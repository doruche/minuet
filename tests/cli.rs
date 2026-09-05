use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(8);

fn command(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_minuet"));
    command.args(args).env_remove("MINUET_HOME");
    command
}

fn capture(mut reader: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = [0; 8192];
        loop {
            let n = reader.read(&mut buffer).unwrap();
            if n == 0 || sender.send(buffer[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    receiver
}

// Keep subprocesses bounded and reclaim them even when an assertion fails.
struct Process {
    child: Child,
    stdout: mpsc::Receiver<Vec<u8>>,
    stderr: mpsc::Receiver<Vec<u8>>,
    out: Vec<u8>,
    err: Vec<u8>,
}

impl Process {
    fn start(mut command: Command, close_stdout: bool) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let stdout = if close_stdout {
            drop(stdout);
            let (_, receiver) = mpsc::channel();
            receiver
        } else {
            capture(stdout)
        };
        let stderr = capture(child.stderr.take().unwrap());
        Self {
            child,
            stdout,
            stderr,
            out: Vec::new(),
            err: Vec::new(),
        }
    }

    fn input(&mut self, text: &[u8]) {
        self.child.stdin.take().unwrap().write_all(text).unwrap();
    }

    #[cfg(unix)]
    fn wait_for_stderr(&mut self, expected: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while !String::from_utf8_lossy(&self.err).contains(expected) {
            let bytes = self
                .stderr
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            self.err.extend(bytes);
        }
    }

    fn finish(mut self) -> (ExitStatus, String, String) {
        let deadline = Instant::now() + TIMEOUT;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "CLI did not exit");
            std::thread::sleep(Duration::from_millis(5));
        };
        for (receiver, bytes) in [(&self.stdout, &mut self.out), (&self.stderr, &mut self.err)] {
            loop {
                match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(part) => bytes.extend(part),
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => panic!("output reader did not finish"),
                }
            }
        }
        (
            status,
            String::from_utf8(self.out.clone()).unwrap(),
            String::from_utf8(self.err.clone()).unwrap(),
        )
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Provider {
    listener: TcpListener,
    home: tempfile::TempDir,
}

impl Provider {
    fn new(max_steps: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            format!(
                r#"
model = "fixture"
model_provider = "fixture"
[loop]
max_steps = {max_steps}
[tools]
enabled = ["echo"]
[model_providers.fixture]
protocol = "openai-responses"
base_url = "http://{}"
api_key_env = "MINUET_CLI_TEST_KEY"
"#,
                listener.local_addr().unwrap()
            ),
        )
        .unwrap();
        Self { listener, home }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = command(args);
        command
            .env("MINUET_HOME", self.home.path())
            .env("MINUET_CLI_TEST_KEY", "fixture-key");
        command
    }

    fn request(&self) -> (TcpStream, Value) {
        let deadline = Instant::now() + TIMEOUT;
        let socket = loop {
            match self.listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "no provider request");
                    std::thread::sleep(Duration::from_millis(5));
                },
                Err(error) => panic!("{error}"),
            }
        };
        socket.set_read_timeout(Some(TIMEOUT)).unwrap();
        socket.set_write_timeout(Some(TIMEOUT)).unwrap();
        let mut reader = BufReader::new(socket.try_clone().unwrap());
        let mut first = String::new();
        reader.read_line(&mut first).unwrap();
        assert_eq!(first, "POST /responses HTTP/1.1\r\n");
        let mut length = 0;
        loop {
            let mut line = String::new();
            assert_ne!(reader.read_line(&mut line).unwrap(), 0);
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = value.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        (socket, serde_json::from_slice(&body).unwrap())
    }
}

fn respond(mut socket: TcpStream, status: &str, body: Value) {
    let body = if status.starts_with('2') {
        format!(
            "data: {}\n\n",
            json!({"type":"response.completed", "response":body})
        )
    } else {
        body.to_string()
    };
    let content_type = if status.starts_with('2') {
        "text/event-stream"
    } else {
        "application/json"
    };
    write!(socket, "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
}

fn answer(text: &str) -> Value {
    json!({"status":"completed", "output":[{"type":"message", "content":[{"type":"output_text", "text":text}]}]})
}

fn tool_call() -> Value {
    json!({"status":"completed", "output":[{"type":"function_call", "call_id":"echo-call", "name":"echo", "arguments":"{\"text\":\"tool-result\"}"}]})
}

#[test]
fn version_and_help_do_not_require_configuration_or_stdin_eof() {
    for (args, expected) in [
        (
            vec!["--version"],
            format!("minuet {}\n", env!("CARGO_PKG_VERSION")),
        ),
        (
            vec!["-V"],
            format!("minuet {}\n", env!("CARGO_PKG_VERSION")),
        ),
        (vec!["--help"], "chat".into()),
        (vec!["chat", "--help"], "<PROMPT>".into()),
        (vec!["tui", "--help"], "terminal".into()),
    ] {
        let (status, out, err) = Process::start(command(&args), false).finish();
        assert!(status.success(), "{err}");
        assert!(out.contains(&expected), "{out}");
        assert!(err.is_empty(), "{err}");
    }
}

#[test]
fn invalid_arguments_and_nonterminal_tui_fail_before_configuration() {
    for args in [
        vec![],
        vec!["tui"],
        vec!["chat"],
        vec!["unknown"],
        vec!["chat", "a", "b"],
        vec!["chat", "  \n"],
    ] {
        let (status, out, err) = Process::start(command(&args), false).finish();
        assert_eq!(status.code(), Some(2), "{args:?}: {err}");
        assert!(out.is_empty(), "{out}");
        assert!(!err.contains("MINUET_HOME"), "{err}");
        if args.is_empty() || args == ["tui"] {
            assert!(err.contains("minuet chat -"), "{err}");
        }
    }
}

#[test]
fn startup_failure_is_stderr_and_nonzero() {
    let (status, out, err) = Process::start(command(&["chat", "hello"]), false).finish();
    assert_eq!(status.code(), Some(1));
    assert!(out.is_empty());
    assert!(err.contains("MINUET_HOME"), "{err}");
}

#[test]
fn chat_preserves_literal_prompts_and_final_text_without_reading_stdin() {
    for (prompt, text, expected) in [
        ("  /help\n中文  ", "answer\t中文\n", "answer\t中文\n"),
        ("--help", "done", "done\n"),
        ("hello", "", ""),
    ] {
        let provider = Provider::new(4);
        let process = Process::start(provider.command(&["chat", "--", prompt]), false);
        let (socket, request) = provider.request();
        assert_eq!(request["input"].as_array().unwrap().len(), 1);
        assert_eq!(request["input"][0]["content"][0]["text"], prompt);
        respond(socket, "200 OK", answer(text));
        let (status, out, err) = process.finish();
        assert!(status.success(), "{err}");
        assert_eq!(out, expected);
        assert!(err.is_empty(), "{err}");
    }
}

#[test]
fn stdin_is_one_multiline_prompt_and_rejects_empty_or_invalid_utf8() {
    let provider = Provider::new(4);
    let mut process = Process::start(provider.command(&["chat", "-"]), false);
    let prompt = "  first\n\t中文\n/exit\n";
    process.input(prompt.as_bytes());
    let (socket, request) = provider.request();
    assert_eq!(request["input"][0]["content"][0]["text"], prompt);
    respond(socket, "200 OK", answer("done"));
    let (status, out, err) = process.finish();
    assert!(status.success(), "{err}");
    assert_eq!(out, "done\n");
    assert!(err.is_empty());
    for (input, expected_status) in [
        (b" \n".as_slice(), 2),
        (b"".as_slice(), 2),
        (b"\xff".as_slice(), 1),
    ] {
        let mut process = Process::start(command(&["chat", "-"]), false);
        process.input(input);
        let (status, out, err) = process.finish();
        assert_eq!(status.code(), Some(expected_status), "{err}");
        assert!(out.is_empty());
        assert!(!err.contains("MINUET_HOME"), "{err}");
    }
}

#[test]
fn chat_runs_tools_and_returns_only_the_final_answer() {
    let provider = Provider::new(4);
    let process = Process::start(provider.command(&["chat", "use echo"]), false);
    let (socket, _) = provider.request();
    respond(socket, "200 OK", tool_call());
    let (socket, request) = provider.request();
    let tool_result = request["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output")
        .unwrap();
    assert!(
        tool_result["output"]
            .as_str()
            .unwrap()
            .contains("tool-result")
    );
    respond(socket, "200 OK", answer("final answer"));
    let (status, out, err) = process.finish();
    assert!(status.success(), "{err}");
    assert_eq!(out, "final answer\n");
    assert!(err.is_empty(), "{err}");
}

#[test]
fn step_limit_preserves_partial_text_but_is_not_success() {
    let provider = Provider::new(1);
    let process = Process::start(provider.command(&["chat", "use echo"]), false);
    let (socket, _) = provider.request();
    let mut response = tool_call();
    response["output"]
        .as_array_mut()
        .unwrap()
        .push(answer("partial")["output"][0].clone());
    respond(socket, "200 OK", response);
    let (status, out, err) = process.finish();
    assert_eq!(status.code(), Some(1));
    assert_eq!(out, "partial\n");
    assert!(err.contains("model-turn limit reached"), "{err}");
}

#[test]
fn provider_and_stdout_failures_are_not_success() {
    let provider = Provider::new(4);
    let process = Process::start(provider.command(&["chat", "hello"]), false);
    let (socket, _) = provider.request();
    respond(
        socket,
        "500 Internal Server Error",
        json!({"error":{"message":"fixture failure"}}),
    );
    let (status, out, err) = process.finish();
    assert_eq!(status.code(), Some(1));
    assert!(out.is_empty());
    assert!(err.contains("500"), "{err}");

    let process = Process::start(provider.command(&["chat", "hello"]), true);
    let (socket, _) = provider.request();
    respond(socket, "200 OK", answer("cannot write this"));
    let (status, out, err) = process.finish();
    assert_eq!(status.code(), Some(1));
    assert!(out.is_empty());
    assert!(!err.is_empty());
}

#[cfg(unix)]
#[test]
fn sigint_waits_for_accepted_execution_then_exits_130() {
    let provider = Provider::new(4);
    let mut process = Process::start(provider.command(&["chat", "use echo"]), false);
    let (socket, _) = provider.request();
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(process.child.id() as i32),
        nix::sys::signal::Signal::SIGINT,
    )
    .unwrap();
    process.wait_for_stderr("waiting for shutdown");
    assert!(process.child.try_wait().unwrap().is_none());
    respond(socket, "200 OK", tool_call());
    let (socket, request) = provider.request();
    assert!(
        request["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["type"] == "function_call_output")
    );
    respond(socket, "200 OK", answer("completed after interrupt"));
    let (status, out, err) = process.finish();
    assert_eq!(status.code(), Some(130), "{err}");
    assert!(out.is_empty());
}
