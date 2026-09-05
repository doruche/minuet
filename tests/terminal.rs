#![cfg(unix)]

// Exercise the actual binary-private interaction layer without exporting UI
// internals or adding a test tool / alternate startup path to the product.
#[path = "../src/cli/mod.rs"]
mod cli;

use std::{
    io::{BufRead, Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use minuet::{
    agent_loop::ReactLoop,
    context::FullContext,
    inference::OpenAiResponsesBackend,
    kernel::{KernelComponents, KernelOptions, start},
    model::{ModelName, ModelSelection, ProviderId},
    session::MemorySessionStore,
    tool::{Tool, ToolDefinition, ToolError, ToolOutput, ToolRegistry},
};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde_json::{Value, json};

const FIXTURE_ENV: &str = "MINUET_TERMINAL_TEST_DIR";

struct GatedTool {
    directory: PathBuf,
    fail: bool,
}

#[async_trait]
impl Tool for GatedTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "stream".into(),
            description: "gated test tool".into(),
            parameters: json!({"type": "object"}),
        }
    }

    async fn invoke(&self, _: Value, output: &dyn ToolOutput) -> Result<Value, ToolError> {
        output.write("执行片段-alpha").await;
        std::fs::write(self.directory.join("waiting"), "").unwrap();
        while !self.directory.join("release").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        output.write("-beta\n").await;
        // Enough output to exercise scrollback and multi-buffer insertions.
        output
            .write(&(0..50).map(|n| format!("row-{n:02}\n")).collect::<String>())
            .await;
        std::fs::write(self.directory.join("executed"), "").unwrap();
        if self.fail {
            Err(ToolError::InvalidArguments("failure-after-stream".into()))
        } else {
            Ok(json!({"result": "final-tool-result"}))
        }
    }
}

fn backend(directory: PathBuf) -> (OpenAiResponsesBackend, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let responses = [
            json!({"status":"completed", "output":[{"type":"function_call", "call_id":"call-stream", "name":"stream", "arguments":"{}"}]}),
            json!({"status":"completed", "output":[{"type":"message", "content":[{"type":"output_text", "text":"assistant-finished"}]}]}),
        ];
        for (index, response) in responses.into_iter().enumerate() {
            let (mut socket, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(socket.try_clone().unwrap());
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            std::fs::write(directory.join(format!("request-{index}.json")), body).unwrap();
            let body = response.to_string();
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    (
        OpenAiResponsesBackend::new(&format!("http://{address}"), "fixture-key").unwrap(),
        server,
    )
}

// Re-executed by the tests below in a PTY or with pipes. This test-only process
// runs the same CLI with a real kernel, adapter and a tool gated by the parent.
#[tokio::test]
async fn terminal_fixture() {
    let Some(directory) = std::env::var_os(FIXTURE_ENV).map(PathBuf::from) else {
        return;
    };
    let (backend, server) = backend(directory.clone());
    let tools = ToolRegistry::new(
        [Arc::new(GatedTool {
            directory,
            fail: std::env::var_os("MINUET_TERMINAL_TEST_FAIL").is_some(),
        }) as Arc<dyn Tool>],
        &["stream".into()],
    )
    .unwrap();
    let running = start(
        KernelComponents {
            backend: Arc::new(backend),
            tools,
            store: Box::new(MemorySessionStore::default()),
            agent_loop: Arc::new(ReactLoop::new(4).unwrap()),
            context: Arc::new(FullContext),
        },
        KernelOptions {
            model: ModelSelection {
                provider: ProviderId::new("fixture").unwrap(),
                model: ModelName::new("fixture").unwrap(),
            },
            default_reasoning_effort: None,
        },
    )
    .unwrap();
    let result = cli::run(running.handle()).await;
    let shutdown = running.shutdown().await;
    result.unwrap();
    shutdown.unwrap();
    // The listener is intentionally process-scoped if no run was submitted.
    if server.is_finished() {
        server.join().unwrap();
    }
}

fn capture(mut reader: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = [0; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if sender.send(buffer[..n].to_vec()).is_err() {
                        break;
                    }
                },
            }
        }
    });
    receiver
}

struct Pty {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    output: mpsc::Receiver<Vec<u8>>,
    parser: vt100::Parser,
    raw: Vec<u8>,
    query_count: usize,
    initial_modes: nix::sys::termios::Termios,
    directory: tempfile::TempDir,
}

impl Pty {
    fn start(fail: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let initial_modes = pair.master.get_termios().unwrap();
        let mut command = CommandBuilder::new(std::env::current_exe().unwrap());
        command.args(["--exact", "terminal_fixture", "--nocapture"]);
        command.env(FIXTURE_ENV, directory.path());
        command.env("TERM", "xterm-256color");
        if fail {
            command.env("MINUET_TERMINAL_TEST_FAIL", "1");
        }
        let child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let output = capture(pair.master.try_clone_reader().unwrap());
        let writer = pair.master.take_writer().unwrap();
        Self {
            master: pair.master,
            child,
            writer,
            output,
            parser: vt100::Parser::new(24, 80, 2000),
            raw: Vec::new(),
            query_count: 0,
            initial_modes,
            directory,
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).unwrap();
        self.writer.flush().unwrap();
    }

    fn pump(&mut self) {
        match self.output.recv_timeout(Duration::from_millis(50)) {
            Ok(bytes) => {
                self.raw.extend_from_slice(&bytes);
                self.parser.process(&bytes);
                // Act as the terminal for cursor-position queries, including
                // queries split across OS reads during setup and resize.
                let queries = self
                    .raw
                    .windows(4)
                    .filter(|bytes| *bytes == b"\x1b[6n")
                    .count();
                for _ in self.query_count..queries {
                    let (row, col) = self.parser.screen().cursor_position();
                    self.send(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
                }
                self.query_count = queries;
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {},
            Err(mpsc::RecvTimeoutError::Disconnected) => {},
        }
    }

    fn wait_for(&mut self, condition: impl Fn(&Self) -> bool, description: &str) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !condition(self) {
            assert!(
                Instant::now() < deadline,
                "timed out: {description}\nscreen:\n{}\nraw: {:?}",
                self.parser.screen().contents(),
                String::from_utf8_lossy(&self.raw)
            );
            self.pump();
        }
    }

    fn ready(&mut self) {
        self.wait_for(
            |pty| pty.parser.screen().contents().contains("Alt-Enter"),
            "input ready",
        );
    }
    fn release(&self) {
        std::fs::write(self.directory.path().join("release"), "").unwrap();
    }

    fn finish(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline, "child did not shut down");
            self.pump();
        }
        while let Ok(bytes) = self.output.try_recv() {
            self.raw.extend_from_slice(&bytes);
            self.parser.process(&bytes);
        }
        assert!(
            self.raw.windows(8).any(|bytes| bytes == b"\x1b[?2004l"),
            "bracketed paste not restored"
        );
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn pty_edits_chinese_pastes_multiline_and_streams_before_tool_return() {
    let mut pty = Pty::start(false);
    pty.ready();
    assert!(!pty.parser.screen().alternate_screen());
    pty.send("中文".as_bytes());
    pty.wait_for(
        |p| p.parser.screen().contents().contains("中文"),
        "Chinese input",
    );
    pty.send(b"\x7f\x7f");
    pty.wait_for(
        |p| !p.parser.screen().contents().contains(['中', '文']),
        "Chinese cells erased",
    );
    pty.send("\x1b[200~  first\n\t第二行\x1b[201~".as_bytes());
    pty.wait_for(
        |p| p.parser.screen().contents().contains("第二行"),
        "multiline paste displayed",
    );
    assert!(
        !pty.directory.path().join("waiting").exists(),
        "paste submitted itself"
    );
    pty.send(b"\r");
    pty.wait_for(
        |p| p.parser.screen().contents().contains("执行片段-alpha"),
        "unterminated live tool fragment",
    );
    assert!(!pty.directory.path().join("executed").exists());
    assert!(
        pty.parser.screen().contents().contains("第二行"),
        "wide output cells must not insert spaces between Chinese characters"
    );
    let request: Value = serde_json::from_slice(
        &std::fs::read(pty.directory.path().join("request-0.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        request["input"][0]["content"][0]["text"],
        "  first\n\t第二行"
    );
    pty.send(b"must-not-queue\r");
    pty.release();
    pty.wait_for(
        |p| p.parser.screen().contents().contains("assistant-finished"),
        "final answer",
    );
    assert!(String::from_utf8_lossy(&pty.raw).contains("final-tool-result"));
    pty.send(b"/exit\r");
    pty.finish();
}

#[test]
fn pty_ctrl_c_restores_terminal_before_waiting_for_existing_shutdown() {
    let mut pty = Pty::start(false);
    pty.ready();
    pty.send(b"go\r");
    pty.wait_for(
        |p| p.parser.screen().contents().contains("执行片段-alpha"),
        "tool waiting",
    );
    pty.send(b"\x03");
    pty.wait_for(
        |p| p.raw.windows(8).any(|bytes| bytes == b"\x1b[?2004l"),
        "terminal restoration",
    );
    let restored = pty.master.get_termios().unwrap();
    assert_eq!(restored, pty.initial_modes);
    assert!(
        pty.child.try_wait().unwrap().is_none(),
        "shutdown must wait for the gated tool"
    );
    pty.release();
    pty.finish();
    assert!(pty.directory.path().join("executed").exists());
}

#[test]
fn pty_failure_keeps_progress_and_resize_keeps_input_usable() {
    let mut pty = Pty::start(true);
    pty.ready();
    pty.master
        .resize(PtySize {
            rows: 16,
            cols: 40,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    pty.parser.screen_mut().set_size(16, 40);
    pty.send("中文\x7f\x7fgo\r".as_bytes());
    pty.wait_for(
        |p| p.parser.screen().contents().contains("执行片段-alpha"),
        "stream after resize",
    );
    pty.release();
    pty.wait_for(
        |p| p.parser.screen().contents().contains("assistant-finished"),
        "answer after tool failure",
    );
    let visible = pty.parser.screen().contents();
    assert!(visible.contains("Failed stream"), "{visible}");
    assert!(
        visible
            .lines()
            .map(str::trim)
            .collect::<String>()
            .contains("failure-after-stream"),
        "{visible}"
    );
    pty.master
        .resize(PtySize {
            rows: 16,
            cols: 30,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    pty.parser.screen_mut().set_size(16, 30);
    pty.send(b"/model info\r");
    pty.wait_for(
        |p| p.parser.screen().contents().contains("provider: fixture"),
        "command after second resize",
    );
    let mut screen = pty.parser.screen().clone();
    let mut retained = false;
    for offset in 0..2000 {
        screen.set_scrollback(offset);
        retained |= screen.contents().contains("assistant-finished");
        if screen.scrollback() != offset {
            break;
        }
    }
    assert!(retained, "horizontal resize erased completed output");
    pty.send(b"\x03");
    pty.finish();
}

// Assertion failures must not strand fixture processes waiting on a gate.
struct PipeChild(std::process::Child);
impl Drop for PipeChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn pipe_fixture(directory: &std::path::Path) -> PipeChild {
    PipeChild(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "terminal_fixture", "--nocapture"])
            .env(FIXTURE_ENV, directory)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    )
}

#[test]
fn pipes_stream_without_ansi_or_prompts_and_accept_following_commands() {
    let directory = tempfile::tempdir().unwrap();
    let mut child = pipe_fixture(directory.path());
    let mut input = child.0.stdin.take().unwrap();
    let output = capture(child.0.stdout.take().unwrap());
    input.write_all(b"go\n").unwrap();
    input.flush().unwrap();
    let mut bytes = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !String::from_utf8_lossy(&bytes).contains("执行片段-alpha") {
        assert!(Instant::now() < deadline, "pipe stream stalled");
        if let Ok(part) = output.recv_timeout(Duration::from_millis(50)) {
            bytes.extend(part);
        }
    }
    assert!(!directory.path().join("executed").exists());
    std::fs::write(directory.path().join("release"), "").unwrap();
    input.write_all(b"/tools list\n/exit\n").unwrap();
    drop(input);
    while child.0.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.0.kill();
            panic!("pipe shutdown stalled");
        }
        if let Ok(part) = output.recv_timeout(Duration::from_millis(50)) {
            bytes.extend(part);
        }
    }
    while let Ok(part) = output.recv_timeout(Duration::from_millis(50)) {
        bytes.extend(part);
    }
    assert!(child.0.wait().unwrap().success());
    let text = String::from_utf8(bytes).unwrap();
    assert!(!text.contains('\x1b'));
    assert!(!text.contains("minuet>"));
    assert!(text.contains("执行片段-alpha-beta\n"));
    assert!(text.contains("stream (enabled)"));
    assert!(text.contains("assistant-finished"));
}

#[test]
fn commands_own_queries_mutations_and_informational_group_help() {
    let directory = tempfile::tempdir().unwrap();
    let mut child = pipe_fixture(directory.path());
    let mut input = child.0.stdin.take().unwrap();
    input.write_all(b"/tools\n/context\n/model\n/tools disable stream\n/tools list\n/tools enable stream\n/tools list\n/model effort 'vendor depth'\n/model info\n/model effort clear\n/model info\n/tools enable\n/exit\n").unwrap();
    drop(input);
    let mut text = String::new();
    child
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert!(child.0.wait().unwrap().success());
    assert!(!text.contains('\x1b'));
    for expected in [
        "Usage: /tools",
        "Usage: /context",
        "Usage: /model",
        "tool `stream` disabled",
        "stream (disabled)",
        "tool `stream` enabled",
        "stream (enabled)",
        "reasoning effort: vendor depth",
        "reasoning effort: upstream default",
        "error:",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
}

#[test]
fn sigint_exits_with_pipe_stdin_still_open() {
    let directory = tempfile::tempdir().unwrap();
    let mut child = pipe_fixture(directory.path());
    let mut input = child.0.stdin.take().unwrap();
    let output = capture(child.0.stdout.take().unwrap());
    input.write_all(b"/tools list\n").unwrap();
    input.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut bytes = Vec::new();
    while !String::from_utf8_lossy(&bytes).contains("stream (enabled)") {
        assert!(Instant::now() < deadline, "fixture did not process command");
        if let Ok(part) = output.recv_timeout(Duration::from_millis(50)) {
            bytes.extend(part);
        }
    }
    // Processing the command proves the interaction loop has installed its
    // signal listener. Keep stdin open: exiting must not require an EOF/read.
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(child.0.id() as i32),
        nix::sys::signal::Signal::SIGINT,
    )
    .unwrap();
    while child.0.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "SIGINT waited for an external stdin read"
        );
        if let Ok(part) = output.recv_timeout(Duration::from_millis(50)) {
            bytes.extend(part);
        }
    }
    while let Ok(part) = output.recv_timeout(Duration::from_millis(50)) {
        bytes.extend(part);
    }
    assert!(child.0.wait().unwrap().success());
    assert!(String::from_utf8_lossy(&bytes).contains("Exiting; waiting for shutdown."));
    drop(input);
}
