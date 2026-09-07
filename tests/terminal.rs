#![cfg(unix)]

// Exercise the actual binary-private interaction layer without exporting UI
// internals or adding a test tool / alternate startup path to the product.
#[path = "../src/tui/mod.rs"]
mod tui;

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
    fn display_arguments(&self, arguments: &Value) -> String {
        arguments.get("n").map(Value::to_string).unwrap_or_default()
    }
    fn display_result(&self, result: &Value) -> String {
        result["result"].as_str().unwrap().to_owned()
    }

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
        let first = std::fs::read(directory.join("first-response.json"))
            .map(|bytes| serde_json::from_slice::<Value>(&bytes).unwrap())
            .unwrap_or_else(|_| json!({"status":"completed", "output":[{"type":"function_call", "call_id":"call-stream", "name":"stream", "arguments":"{}"}]}));
        let responses = [
            first,
            std::fs::read(directory.join("second-response.json"))
                .map(|bytes| serde_json::from_slice::<Value>(&bytes).unwrap())
                .unwrap_or_else(|_| json!({"status":"completed", "output":[{"type":"message", "content":[{"type":"output_text", "text": std::fs::read_to_string(directory.join("answer.md")).unwrap_or_else(|_| "assistant-finished".into())}]}]})),
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
            while directory.join(format!("hold-response-{index}")).exists() {
                std::thread::sleep(Duration::from_millis(5));
            }
            let mut body = String::new();
            if let Some(text) = response
                .pointer("/output/0/content/0/text")
                .and_then(Value::as_str)
            {
                let split = text.floor_char_boundary(text.len() / 2);
                for delta in [&text[..split], &text[split..]] {
                    body.push_str(&format!(
                        "data: {}\n\n",
                        json!({"type":"response.output_text.delta", "delta":delta})
                    ));
                }
            }
            let terminal = if directory.join(format!("fail-completion-{index}")).exists() {
                json!({"type":"response.failed", "response":{"error":{"message":"fixture stream failed"}}})
            } else {
                json!({"type":"response.completed", "response":response})
            };
            let completion = format!("data: {terminal}\n\n");
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len() + completion.len()).unwrap();
            socket.flush().unwrap();
            while directory.join(format!("hold-completion-{index}")).exists() {
                std::thread::sleep(Duration::from_millis(5));
            }
            socket.write_all(completion.as_bytes()).unwrap();
        }
    });
    (
        OpenAiResponsesBackend::new(&format!("http://{address}"), "fixture-key").unwrap(),
        server,
    )
}

// Re-executed by the tests below in a PTY. This test-only process
// runs the same TUI with a real kernel, adapter and a tool gated by the parent.
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
            agent_loop: Arc::new(
                ReactLoop::new(
                    std::env::var("MINUET_TERMINAL_TEST_LIMIT")
                        .map(|limit| limit.parse().unwrap())
                        .unwrap_or(4),
                )
                .unwrap(),
            ),
            context: Arc::new(FullContext),
        },
        KernelOptions {
            model: ModelSelection {
                provider: ProviderId::new("fixture").unwrap(),
                model: ModelName::new("fixture").unwrap(),
            },
            default_reasoning_effort: None,
            default_enabled_tools: vec!["stream".to_owned()],
        },
    )
    .unwrap();
    let result = tui::run(running.handle()).await;
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
    keyboard_query_count: usize,
    keyboard_reply: Option<bool>,
    initial_modes: nix::sys::termios::Termios,
    directory: tempfile::TempDir,
}

impl Pty {
    fn start(fail: bool) -> Self {
        Self::start_with_options(fail, Some(false), true)
    }

    fn start_with_options(fail: bool, keyboard_reply: Option<bool>, colors: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut command = CommandBuilder::new(std::env::current_exe().unwrap());
        command.args(["--exact", "terminal_fixture", "--nocapture"]);
        command.env(FIXTURE_ENV, directory.path());
        if fail {
            command.env("MINUET_TERMINAL_TEST_FAIL", "1");
        }
        Self::start_process(command, directory, keyboard_reply, colors)
    }

    fn start_process(
        mut command: CommandBuilder,
        directory: tempfile::TempDir,
        keyboard_reply: Option<bool>,
        colors: bool,
    ) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let initial_modes = pair.master.get_termios().unwrap();
        command.env("TERM", "xterm-256color");
        command.env("NO_COLOR", if colors { "" } else { "1" });
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
            keyboard_query_count: 0,
            keyboard_reply,
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
                let queries = self
                    .raw
                    .windows(3)
                    .filter(|bytes| *bytes == b"\x1b[c")
                    .count();
                for _ in self.keyboard_query_count..queries {
                    if let Some(enhanced) = self.keyboard_reply {
                        if enhanced {
                            self.send(b"\x1b[?0u");
                        }
                        self.send(b"\x1b[?1;2c");
                    }
                }
                self.keyboard_query_count = queries;
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
            |pty| pty.frame_complete() && pty.parser.screen().contents().contains("Ctrl-O"),
            "input ready",
        );
    }

    fn frame_complete(&self) -> bool {
        self.raw.ends_with(b"\x1b[?2026l")
    }

    fn activity(&self, phase: &str) -> Option<(char, u64)> {
        if !self.frame_complete() {
            return None;
        }
        self.parser.screen().contents().lines().find_map(|line| {
            let (activity, label) = line.split_once(" · ")?;
            if label.trim_end() != phase {
                return None;
            }
            let mut fields = activity.split_whitespace();
            let frame = fields.next()?.chars().next()?;
            let seconds = fields.next()?.strip_suffix('s')?.parse().ok()?;
            Some((frame, seconds))
        })
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

#[test]
fn pty_paces_model_preview_before_commit_and_flushes_at_success_or_failure() {
    for fail in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let text = format!("livepreview-{}\nreceived-tail\nend", "字".repeat(1000));
        let response = json!({"status":"completed", "output":[{"type":"message", "content":[{"type":"output_text", "text":text}]}]});
        std::fs::write(
            directory.path().join("first-response.json"),
            response.to_string(),
        )
        .unwrap();
        std::fs::write(directory.path().join("hold-completion-0"), "").unwrap();
        if fail {
            std::fs::write(directory.path().join("fail-completion-0"), "").unwrap();
        }
        let mut command = CommandBuilder::new(std::env::current_exe().unwrap());
        command.args(["--exact", "terminal_fixture", "--nocapture"]);
        command.env(FIXTURE_ENV, directory.path());
        let mut pty = Pty::start_process(command, directory, Some(false), false);
        pty.ready();
        pty.send(b"show paced output\r");
        pty.wait_for(
            |p| p.frame_complete() && p.parser.screen().contents().contains("livepreview-字"),
            "model preview before terminal event and with no tool output",
        );
        let first = pty.parser.screen().contents().matches('字').count();
        assert!(!pty.parser.screen().contents().contains("received-tail"));
        pty.wait_for(
            |p| p.frame_complete() && p.parser.screen().contents().matches('字').count() > first,
            "preview advances without another upstream write",
        );
        assert!(!pty.parser.screen().contents().contains("received-tail"));
        pty.wait_for(
            |p| p.frame_complete() && p.parser.screen().contents().contains("received-tail"),
            "preview fills the available terminal height before completion",
        );
        let preview = pty.parser.screen().contents();
        assert!(
            preview.lines().filter(|line| line.contains('字')).count() > 6,
            "{preview}"
        );
        assert!(!preview.contains("livepreview-"), "{preview}");
        assert!(preview.contains("Waiting for model"), "{preview}");
        std::fs::remove_file(pty.directory.path().join("hold-completion-0")).unwrap();
        pty.wait_for(
            |p| {
                p.frame_complete()
                    && p.parser.screen().contents().contains("received-tail")
                    && p.parser.screen().contents().contains(if fail {
                        "Failed ·"
                    } else {
                        "Completed ·"
                    })
            },
            "terminal event flushes queued text and restores input",
        );
        assert_eq!(
            pty.parser
                .screen()
                .contents()
                .contains("[response incomplete]"),
            fail
        );
        pty.send(b"/exit\r");
        pty.finish();
    }
}

#[test]
fn pty_status_ticks_without_output_and_keeps_total_time_across_phases() {
    let mut pty = Pty::start_with_options(false, Some(false), false);
    pty.ready();
    for index in 0..2 {
        std::fs::write(
            pty.directory.path().join(format!("hold-response-{index}")),
            "",
        )
        .unwrap();
    }
    pty.send(b"timed run\r");
    pty.wait_for(
        |p| p.activity("Waiting for model…").is_some(),
        "model waiting status",
    );
    let first_frame = pty.activity("Waiting for model…").unwrap().0;
    pty.wait_for(
        |p| {
            p.activity("Waiting for model…")
                .is_some_and(|(frame, _)| frame != first_frame)
        },
        "animation without model output or input",
    );
    pty.wait_for(
        |p| {
            p.activity("Waiting for model…")
                .is_some_and(|(_, seconds)| seconds >= 1)
        },
        "elapsed time without model output or input",
    );
    let model_seconds = pty.activity("Waiting for model…").unwrap().1;
    assert_eq!(
        pty.parser
            .screen()
            .contents()
            .matches("Waiting for model…")
            .count(),
        1
    );
    std::fs::remove_file(pty.directory.path().join("hold-response-0")).unwrap();
    pty.wait_for(|p| p.activity("Running stream…").is_some(), "tool status");
    assert!(pty.activity("Running stream…").unwrap().1 >= model_seconds);
    pty.wait_for(
        |p| {
            p.activity("Running stream…")
                .is_some_and(|(_, seconds)| seconds > model_seconds)
        },
        "elapsed time during a quiet tool",
    );
    let tool_seconds = pty.activity("Running stream…").unwrap().1;
    assert!(pty.parser.screen().contents().contains("执行片段-alpha"));
    pty.release();
    pty.wait_for(
        |p| p.activity("Waiting for model…").is_some(),
        "second model call",
    );
    assert!(pty.activity("Waiting for model…").unwrap().1 >= tool_seconds);
    std::fs::remove_file(pty.directory.path().join("hold-response-1")).unwrap();
    pty.wait_for(
        |p| p.frame_complete() && p.parser.screen().contents().contains("Completed · "),
        "completion time",
    );
    let contents = pty.parser.screen().contents();
    assert!(contents.contains("assistant-finished"));
    assert!(contents.contains("Ctrl-O"));
    assert!(!contents.contains("Waiting for model…"));
    assert_eq!(contents.matches("Completed · ").count(), 1);
    let seconds: u64 = contents
        .lines()
        .find_map(|line| line.strip_prefix("Completed · "))
        .unwrap()
        .trim_end()
        .strip_suffix('s')
        .unwrap()
        .parse()
        .unwrap();
    assert!(seconds >= tool_seconds);
    pty.send(b"/model info\r");
    pty.wait_for(
        |p| p.frame_complete() && p.parser.screen().contents().contains("provider: fixture"),
        "command after timed run",
    );
    assert_eq!(
        pty.parser
            .screen()
            .contents()
            .matches("Completed · ")
            .count(),
        1
    );
    pty.send(b"/exit\r");
    pty.finish();
}

#[test]
fn pty_new_run_resets_time_and_model_failure_retains_elapsed_summary() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("first-response.json"),
        r#"{"status":"completed","output":[]}"#,
    )
    .unwrap();
    std::fs::write(
        directory.path().join("second-response.json"),
        r#"{"status":"failed","output":[]}"#,
    )
    .unwrap();
    for index in 0..2 {
        std::fs::write(directory.path().join(format!("hold-response-{index}")), "").unwrap();
    }
    let mut command = CommandBuilder::new(std::env::current_exe().unwrap());
    command.args(["--exact", "terminal_fixture", "--nocapture"]);
    command.env(FIXTURE_ENV, directory.path());
    let mut pty = Pty::start_process(command, directory, Some(false), true);
    pty.ready();
    pty.send(b"first\r");
    pty.wait_for(
        |p| {
            p.activity("Waiting for model…")
                .is_some_and(|(_, seconds)| seconds >= 1)
        },
        "first run time",
    );
    let first_seconds = pty.activity("Waiting for model…").unwrap().1;
    std::fs::remove_file(pty.directory.path().join("hold-response-0")).unwrap();
    pty.wait_for(
        |p| p.frame_complete() && p.parser.screen().contents().contains("Completed · "),
        "empty response completion",
    );
    assert!(pty.parser.screen().contents().contains("(no text output)"));
    pty.send(b"second\r");
    pty.wait_for(
        |p| p.activity("Waiting for model…").is_some(),
        "next run status",
    );
    assert!(pty.activity("Waiting for model…").unwrap().1 < first_seconds);
    pty.wait_for(
        |p| {
            p.activity("Waiting for model…")
                .is_some_and(|(_, seconds)| seconds >= 1)
        },
        "second run time",
    );
    let failed_seconds = pty.activity("Waiting for model…").unwrap().1;
    std::fs::remove_file(pty.directory.path().join("hold-response-1")).unwrap();
    pty.wait_for(
        |p| p.frame_complete() && p.parser.screen().contents().contains("Failed · "),
        "failure time",
    );
    let contents = pty.parser.screen().contents();
    assert!(contents.contains("error:"));
    assert!(contents.contains("Ctrl-O"));
    assert!(!contents.contains("Waiting for model…"));
    assert_eq!(contents.matches("Completed · ").count(), 1);
    assert_eq!(contents.matches("Failed · ").count(), 1);
    let seconds: u64 = contents
        .lines()
        .find_map(|line| line.strip_prefix("Failed · "))
        .unwrap()
        .trim_end()
        .strip_suffix('s')
        .unwrap()
        .parse()
        .unwrap();
    assert!(seconds >= failed_seconds);
    pty.send(b"/exit\r");
    pty.finish();
}

#[test]
fn pty_markdown_publishes_blocks_links_and_highlighting_then_restores_input() {
    for colors in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let answer = format!(
            "# Heading\n\n{}\n\n```rust\nfn main() {{}}\n```\n\n[label](https://example.test) **bold** ![alt](image.png)\n\n| Long heading | Other |\n| --- | --- |\n| cell | value |\n\nassistant-finished",
            (0..140)
                .map(|n| format!("line-{n:03} **text**\n\n"))
                .collect::<String>()
        );
        std::fs::write(directory.path().join("answer.md"), &answer).unwrap();
        let mut command = CommandBuilder::new(std::env::current_exe().unwrap());
        command.args(["--exact", "terminal_fixture", "--nocapture"]);
        command.env(FIXTURE_ENV, directory.path());
        let mut pty = Pty::start_process(command, directory, Some(false), colors);
        pty.ready();
        pty.send(b"markdown please\r");
        pty.wait_for(
            |p| p.directory.path().join("waiting").exists(),
            "tool running",
        );
        pty.master
            .resize(PtySize {
                rows: 24,
                cols: 30,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        pty.parser.screen_mut().set_size(24, 30);
        pty.release();
        pty.wait_for(
            |p| {
                p.frame_complete()
                    && p.parser.screen().contents().contains("assistant-finished")
                    && p.parser.screen().contents().contains("Completed ·")
            },
            "Markdown complete",
        );
        let screen = pty.parser.screen();
        assert!(screen.contents().contains("label bold [img] alt"));
        assert!(!screen.contents().contains("https://"));
        let code_row = screen
            .contents()
            .lines()
            .position(|line| line.contains("fn main"))
            .unwrap() as u16;
        let code_column = screen
            .contents()
            .lines()
            .nth(code_row as usize)
            .and_then(|line| line.find("fn main"))
            .unwrap() as u16;
        assert_eq!(
            matches!(
                screen.cell(code_row, code_column).unwrap().fgcolor(),
                vt100::Color::Rgb(..)
            ),
            colors
        );
        let raw = String::from_utf8_lossy(&pty.raw);
        assert!(raw.contains("\x1b]8;;https://example.test\x1b\\label\x1b]8;;\x1b\\"));
        pty.send(b"/model info\r");
        pty.wait_for(
            |p| p.frame_complete() && p.parser.screen().contents().contains("provider: fixture"),
            "literal output after Markdown",
        );
        pty.parser.screen_mut().set_scrollback(1000);
        let mut transcript = String::new();
        for _ in 0..1000 {
            transcript.push_str(&pty.parser.screen().contents());
            let offset = pty.parser.screen().scrollback();
            if offset == 0 {
                break;
            }
            pty.parser
                .screen_mut()
                .set_scrollback(offset.saturating_sub(20));
        }
        for n in 0..140 {
            assert!(
                transcript.contains(&format!("line-{n:03} text")),
                "lost block {n}\ntranscript: {transcript}"
            );
        }
        pty.parser.screen_mut().set_scrollback(0);
        pty.send("编辑".as_bytes());
        pty.wait_for(
            |p| p.frame_complete() && p.parser.screen().contents().contains("> 编辑"),
            "input after Markdown",
        );
        let (row, col) = pty.parser.screen().cursor_position();
        assert_eq!(col, 6);
        assert_eq!(
            pty.parser.screen().cell(row, col).unwrap().bgcolor(),
            vt100::Color::Default
        );
        pty.send(b"\x7f\x7f/exit\r");
        pty.finish();
        assert_eq!(pty.master.get_termios().unwrap(), pty.initial_modes);
    }
}

#[test]
fn pty_unclosed_model_code_cannot_capture_the_run_limit_notice() {
    let directory = tempfile::tempdir().unwrap();
    let response = json!({"status":"completed", "output":[
        {"type":"message", "content":[{"type":"output_text", "text":"```rust\nfn partial() {}"}]},
        {"type":"function_call", "call_id":"call-stream", "name":"stream", "arguments":"{}"}
    ]});
    std::fs::write(
        directory.path().join("first-response.json"),
        response.to_string(),
    )
    .unwrap();
    let mut command = CommandBuilder::new(std::env::current_exe().unwrap());
    command.args(["--exact", "terminal_fixture", "--nocapture"]);
    command.env(FIXTURE_ENV, directory.path());
    command.env("MINUET_TERMINAL_TEST_LIMIT", "1");
    let mut pty = Pty::start_process(command, directory, Some(false), true);
    pty.ready();
    pty.send(b"limited\r");
    pty.wait_for(
        |p| p.frame_complete() && p.parser.screen().contents().contains("Stopped · "),
        "limit notice",
    );
    let contents = pty.parser.screen().contents();
    assert!(!contents.contains("Completed · "));
    assert!(!contents.contains("Waiting for model…"));
    assert_eq!(
        contents.matches("fn partial() {}").count(),
        1,
        "{contents:?}"
    );
    assert!(
        contents.find("fn partial() {}") < contents.find("Skipped stream()"),
        "{contents:?}"
    );
    assert!(
        contents.find("Skipped stream()") < contents.find("run stopped:"),
        "{contents:?}"
    );
    let row = contents
        .lines()
        .position(|line| line.starts_with("run stopped:"))
        .unwrap() as u16;
    let cell = pty.parser.screen().cell(row, 0).unwrap();
    assert_eq!(cell.fgcolor(), vt100::Color::Idx(1));
    assert!(cell.bold());
    assert!(!pty.directory.path().join("waiting").exists());
    pty.send(b"/exit\r");
    pty.finish();
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn pty_backspace_clears_wide_character_styles() {
    let mut pty = Pty::start(false);
    pty.ready();
    let mut text = "中文测试输入".to_owned();
    pty.send(text.as_bytes());
    pty.wait_for(
        |p| p.parser.screen().contents().contains(&text),
        "wide text",
    );
    while text.pop().is_some() {
        pty.send(b"\x7f");
        pty.wait_for(
            |p| {
                let screen = p.parser.screen().contents();
                p.frame_complete()
                    && screen
                        .lines()
                        .any(|line| line.trim_end() == format!("> {text}").trim_end())
            },
            "deleted wide character",
        );
        let screen = pty.parser.screen();
        let (height, width) = screen.size();
        let reversed = (0..height)
            .flat_map(|row| (0..width).map(move |col| (row, col)))
            .filter(|&(row, col)| screen.cell(row, col).unwrap().inverse())
            .collect::<Vec<_>>();
        assert_eq!(
            reversed.len(),
            0,
            "cursor residue after deleting to {text:?}: {reversed:?}"
        );
        let (row, col) = screen.cursor_position();
        assert_eq!(col, 2 + text.chars().count() as u16 * 2);
        assert!(!screen.hide_cursor());
        for col in col..width {
            let cell = screen.cell(row, col).unwrap();
            assert_eq!(cell.bgcolor(), vt100::Color::Default);
            assert!(cell.contents().trim().is_empty());
        }
    }
    pty.send(b"/exit\r");
    pty.finish();
}

#[test]
fn pty_shift_enter_and_ctrl_o_preserve_manual_newlines_and_restore_keyboard_mode() {
    for enhanced in [true, false] {
        let mut pty = Pty::start_with_options(false, Some(enhanced), true);
        pty.ready();
        assert_eq!(
            pty.parser.screen().contents().contains("Shift-Enter"),
            enhanced
        );
        assert_eq!(pty.raw.windows(5).any(|b| b == b"\x1b[>1u"), enhanced);
        pty.send("第一行".as_bytes());
        pty.send(if enhanced { b"\x1b[13;2u" } else { b"\x0f" });
        pty.send("第二行".as_bytes());
        pty.send(b"\x0fend");
        pty.wait_for(
            |p| p.frame_complete() && p.parser.screen().contents().contains("  end"),
            "manual multiline input",
        );
        assert!(!pty.directory.path().join("waiting").exists());
        pty.send(b"\r");
        pty.wait_for(
            |p| p.directory.path().join("request-0.json").exists(),
            "submitted multiline input",
        );
        let request: Value = serde_json::from_slice(
            &std::fs::read(pty.directory.path().join("request-0.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            request["input"][0]["content"][0]["text"],
            "第一行\n第二行\nend"
        );
        pty.release();
        pty.wait_for(
            |p| {
                p.frame_complete()
                    && p.parser.screen().contents().contains("assistant-finished")
                    && p.parser.screen().contents().contains("Completed ·")
            },
            "run complete",
        );
        pty.send(b"/exit\r");
        pty.finish();
        assert_eq!(
            pty.raw.windows(5).filter(|b| *b == b"\x1b[<1u").count(),
            usize::from(enhanced)
        );
        assert_eq!(pty.master.get_termios().unwrap(), pty.initial_modes);
    }
}

#[test]
fn pty_compact_prompt_wraps_and_shrinks_without_cursor_or_style_residue() {
    let mut pty = Pty::start(false);
    pty.ready();
    let screen = pty.parser.screen();
    let (row, col) = screen.cursor_position();
    assert_eq!(col, 2);
    assert_eq!(screen.cell(row, 0).unwrap().contents(), ">");
    assert_eq!(
        screen.cell(row, 0).unwrap().fgcolor(),
        vt100::Color::Idx(14)
    );
    assert!(screen.cell(row, 0).unwrap().bold());
    assert_eq!(screen.cell(row - 1, 0).unwrap().contents(), "M");
    assert_eq!(screen.cell(row + 1, 0).unwrap().contents(), "E");
    pty.master
        .resize(PtySize {
            rows: 24,
            cols: 12,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    pty.parser.screen_mut().set_size(24, 12);
    pty.send("甲乙丙丁戊Z".as_bytes());
    pty.wait_for(
        |p| p.frame_complete() && p.parser.screen().contents().contains("  Z"),
        "wrapped input",
    );
    let (row, col) = pty.parser.screen().cursor_position();
    assert_eq!(col, 3);
    assert_eq!(
        pty.parser.screen().cell(row - 1, 0).unwrap().contents(),
        ">"
    );
    let text_cell = pty.parser.screen().cell(row, 2).unwrap();
    assert_eq!(text_cell.fgcolor(), vt100::Color::Default);
    assert!(!text_cell.bold());
    pty.send(b"\x7f\x7f");
    pty.wait_for(
        |p| p.frame_complete() && !p.parser.screen().contents().contains(['戊', 'Z']),
        "shrunk input",
    );
    let (row, col) = pty.parser.screen().cursor_position();
    assert_eq!(col, 10);
    assert_eq!(
        pty.parser.screen().cell(row + 1, 0).unwrap().contents(),
        "E"
    );
    pty.send(b"\x7f\x7f\x7f\x7f/exit\r");
    pty.finish();
}

#[test]
fn pty_legacy_probe_timeout_keeps_newline_and_no_color_editing_available() {
    let mut pty = Pty::start_with_options(false, None, false);
    pty.ready();
    assert!(!pty.parser.screen().contents().contains("Shift-Enter"));
    let (row, _) = pty.parser.screen().cursor_position();
    assert_eq!(
        pty.parser.screen().cell(row, 0).unwrap().fgcolor(),
        vt100::Color::Default
    );
    pty.send(b"a\x0fb");
    pty.wait_for(
        |p| p.frame_complete() && p.parser.screen().contents().contains("  b"),
        "fallback newline",
    );
    pty.send(b"\x7f\x7f\x7f/exit\r");
    pty.finish();
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
        "wide output cells must not insert spaces between Chinese characters: {:?}",
        pty.parser.screen().contents()
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
        |p| {
            p.frame_complete()
                && p.parser.screen().contents().contains("assistant-finished")
                && p.parser.screen().contents().contains("Completed ·")
        },
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
        |p| {
            p.frame_complete()
                && p.parser.screen().contents().contains("assistant-finished")
                && p.parser.screen().contents().contains("Completed ·")
        },
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
    assert!(
        retained,
        "horizontal resize erased completed output\nscreen: {}\nraw: {:?}",
        pty.parser.screen().contents(),
        String::from_utf8_lossy(&pty.raw)
    );
    pty.send(b"\x03");
    pty.finish();
}

#[test]
fn commands_own_queries_mutations_and_informational_group_help() {
    let mut pty = Pty::start(false);
    pty.ready();
    for (command, expected) in [
        ("/tools", "Usage: /tools"),
        ("/context", "Usage: /context"),
        ("/model", "Usage: /model"),
        ("/tools disable stream", "tool `stream` disabled"),
        ("/tools list", "stream (disabled)"),
        ("/tools enable stream", "tool `stream` enabled"),
        ("/tools list", "stream (enabled)"),
        (
            "/model effort 'vendor depth'",
            "reasoning effort set to `vendor depth`",
        ),
        ("/model info", "reasoning effort: vendor depth"),
        ("/model effort clear", "reasoning effort cleared"),
        ("/model info", "reasoning effort: upstream default"),
        ("/tools enable", "error:"),
    ] {
        let start = pty.raw.len();
        pty.send(format!("{command}\r").as_bytes());
        pty.wait_for(
            |p| {
                // Decode only this command's output so a previous query cannot
                // satisfy the assertion, even when its result text is identical.
                let mut response = vt100::Parser::new(24, 80, 0);
                response.process(&p.raw[start..]);
                p.frame_complete() && response.screen().contents().contains(expected)
            },
            expected,
        );
    }
    pty.send(b"/exit\r");
    pty.finish();
}

#[test]
fn binary_default_and_explicit_tui_open_and_restore_the_terminal() {
    for args in [vec![], vec!["tui"]] {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("config.toml"),
            r#"
model = "fixture"
model_provider = "fixture"
[model_providers.fixture]
protocol = "openai-responses"
base_url = "http://127.0.0.1:9"
api_key_env = "MINUET_CLI_TEST_KEY"
"#,
        )
        .unwrap();
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_minuet"));
        command.args(args);
        command.env("MINUET_HOME", directory.path());
        command.env("MINUET_CLI_TEST_KEY", "fixture-key");
        let mut pty = Pty::start_process(command, directory, Some(false), true);
        pty.ready();
        pty.send(b"/model info\r");
        pty.wait_for(
            |p| p.frame_complete() && p.parser.screen().contents().contains("provider: fixture"),
            "binary model query",
        );
        pty.send(b"/exit\r");
        pty.finish();
        assert_eq!(pty.master.get_termios().unwrap(), pty.initial_modes);
    }
}

#[test]
fn pty_session_replay_distinguishes_display_clear_from_session_clear() {
    for fail in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("first-response.json"),
            json!({"status":"completed","output":[
                {"type":"message","content":[{"type":"output_text","text":"**before-tool**"}]},
                {"type":"function_call","call_id":"call-stream","name":"stream","arguments":"{}"}
            ]})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            directory.path().join("second-response.json"),
            json!({"status":"completed","output":[{"type":"message","content":[
                {"type":"output_text","text":"**replay-answer** with [link](https://example.com)\n\n中文"}
            ]}]})
            .to_string(),
        )
        .unwrap();
        let mut command = CommandBuilder::new(std::env::current_exe().unwrap());
        command.args(["--exact", "terminal_fixture", "--nocapture"]);
        command.env(FIXTURE_ENV, directory.path());
        if fail {
            command.env("MINUET_TERMINAL_TEST_FAIL", "1");
        }
        let mut pty = Pty::start_process(command, directory, Some(false), false);
        pty.ready();
        pty.send(b"/session info\r");
        pty.wait_for(
            |p| p.frame_complete() && p.parser.screen().contents().contains(": 0 items"),
            "initial session identity",
        );
        let contents = pty.parser.screen().contents();
        let id = contents
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("session ")
                    .and_then(|rest| rest.split_once(':'))
                    .map(|(id, _)| id.to_owned())
            })
            .unwrap();
        pty.send(b"replay-question\r");
        pty.wait_for(
            |p| p.directory.path().join("waiting").exists(),
            "tool waiting",
        );
        pty.release();
        pty.wait_for(
            |p| {
                p.frame_complete()
                    && p.parser.screen().contents().contains("Completed ·")
                    && p.parser.screen().contents().contains("replay-answer")
            },
            "completed original run",
        );
        let executed = std::fs::metadata(pty.directory.path().join("executed"))
            .unwrap()
            .modified()
            .unwrap();
        let requests: Vec<_> = (0..2)
            .map(|i| std::fs::read(pty.directory.path().join(format!("request-{i}.json"))).unwrap())
            .collect();
        let start = pty.raw.len();
        pty.send(b"/session new\r");
        pty.wait_for(
            |p| p.frame_complete() && p.parser.screen().contents().contains("started session"),
            "new session clears view",
        );
        let cleared = pty.parser.screen().contents();
        assert!(!cleared.contains("replay-answer"), "{cleared}");
        assert!(!cleared.contains("replay-question"), "{cleared}");
        // vt100 0.16 does not emulate ED3. Check the explicit purge request
        // separately from the emulator's visible-screen replacement evidence.
        assert!(pty.raw[start..].windows(4).any(|bytes| bytes == b"\x1b[3J"));
        pty.send(format!("/session switch {id}\r").as_bytes());
        pty.wait_for(
            |p| p.frame_complete() && p.parser.screen().contents().contains("switched to session"),
            "replay selected session",
        );
        let replay = pty.parser.screen().contents();
        for text in [
            "replay-question",
            "before-tool",
            "stream()",
            "replay-answer",
            "中文",
        ] {
            assert!(replay.contains(text), "missing {text}: {replay}");
        }
        assert!(!replay.contains("**"), "Markdown should render: {replay}");
        assert!(
            replay.contains(if fail {
                "Failed stream()"
            } else {
                "Ran stream()"
            }),
            "{replay}"
        );
        if fail {
            // The terminal may wrap inside the failure marker.
            assert!(replay.contains("invalid arguments"), "{replay}");
            assert!(replay.contains("er-stream"), "{replay}");
        } else {
            assert!(replay.contains("final-tool-result"), "{replay}");
        }
        assert!(replay.find("replay-question") < replay.find("before-tool"));
        assert!(replay.find("before-tool") < replay.find("stream()"));
        assert!(replay.find("stream()") < replay.find("replay-answer"));
        assert!(
            !replay.contains("row-49"),
            "provisional tool fragments are not committed results"
        );
        assert_eq!(
            std::fs::metadata(pty.directory.path().join("executed"))
                .unwrap()
                .modified()
                .unwrap(),
            executed
        );
        assert!(
            pty.raw[start..]
                .windows("https://example.com".len())
                .any(|bytes| bytes == b"https://example.com")
        );
        for (i, request) in requests.iter().enumerate() {
            assert_eq!(
                &std::fs::read(pty.directory.path().join(format!("request-{i}.json"))).unwrap(),
                request
            );
        }
        pty.send(b"/session switch invalid-id\r");
        pty.wait_for(
            |p| p.frame_complete() && p.parser.screen().contents().contains("invalid session ID"),
            "invalid switch is visible",
        );
        assert!(pty.parser.screen().contents().contains("replay-answer"));
        let start = pty.raw.len();
        pty.send(b"/clear\r");
        pty.wait_for(
            |p| {
                p.frame_complete()
                    && p.parser
                        .screen()
                        .contents()
                        .contains("Cleared display; session context preserved.")
            },
            "clear resets view",
        );
        assert!(!pty.parser.screen().contents().contains("replay-answer"));
        assert!(pty.raw[start..].windows(4).any(|bytes| bytes == b"\x1b[3J"));
        pty.send(format!("/session switch {id}\r").as_bytes());
        pty.wait_for(
            |p| p.frame_complete() && p.parser.screen().contents().contains("switched to session"),
            "display clear preserves the session transcript",
        );
        assert!(pty.parser.screen().contents().contains("replay-question"));
        assert!(pty.parser.screen().contents().contains("replay-answer"));
        pty.send(b"/session clear\r");
        pty.wait_for(
            |p| {
                p.frame_complete()
                    && p.parser
                        .screen()
                        .contents()
                        .contains("cleared conversation history")
            },
            "session clear resets view and context",
        );
        assert!(!pty.parser.screen().contents().contains("replay-answer"));
        pty.send(b"/session info\r");
        pty.wait_for(
            |p| {
                let contents = p.parser.screen().contents();
                p.frame_complete()
                    && contents.contains(": 0 items")
                    && contents.contains("usage_total=")
                    && contents.lines().any(|line| line.trim() == "0")
            },
            "session clear resets context",
        );
        let start = pty.raw.len();
        pty.send(format!("/session switch {id}\r").as_bytes());
        pty.wait_for(
            |p| {
                p.frame_complete()
                    && p.raw.len() > start
                    && p.parser.screen().contents().contains("switched to session")
            },
            "cleared session remains empty after switching",
        );
        assert!(!pty.parser.screen().contents().contains("replay-question"));
        pty.send(b"/exit\r");
        pty.finish();
    }
}

#[test]
fn pty_execution_timeline_and_ordered_replay_keep_independent_markdown_documents() {
    let directory = tempfile::tempdir().unwrap();
    let message =
        |text: &str| json!({"type":"message","content":[{"type":"output_text","text":text}]});
    std::fs::write(
        directory.path().join("first-response.json"),
        json!({"status":"completed","output":[
            message("[cross][ref]\n\n"),
            message("[ref]: https://example.test/cross\n\n**separate-message**"),
            {"type":"function_call","call_id":"x","name":"stream","arguments":"{\"n\":1}"},
            message("```rust\nfn partial() {}"),
            {"type":"function_call","call_id":"y","name":"stream","arguments":"{\"n\":2}"},
            message("**after-fence**"),
        ]})
        .to_string(),
    )
    .unwrap();
    let mut command = CommandBuilder::new(std::env::current_exe().unwrap());
    command.args(["--exact", "terminal_fixture", "--nocapture"]);
    command.env(FIXTURE_ENV, directory.path());
    let mut pty = Pty::start_process(command, directory, Some(false), true);
    pty.ready();
    pty.send(b"/session info\r");
    pty.wait_for(
        |p| p.frame_complete() && p.parser.screen().contents().contains(": 0 items"),
        "session identity",
    );
    let id = pty
        .parser
        .screen()
        .contents()
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("session ")
                .and_then(|rest| rest.split_once(':'))
                .map(|(id, _)| id.to_owned())
        })
        .unwrap();
    // Keep the short semantic transcript visible while testing both live and
    // replay. The fixture's streamed rows still exercise native scrollback.
    pty.master
        .resize(PtySize {
            rows: 40,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    pty.parser.screen_mut().set_size(40, 100);
    let run_start = pty.raw.len();
    pty.send(b"ordered question\r");
    pty.wait_for(
        |p| p.frame_complete() && p.parser.screen().contents().contains("执行片段-alpha"),
        "first tool's live output",
    );
    let live = pty.parser.screen().contents();
    let assert_order = |contents: &str, expected: &[&str]| {
        let mut previous = None;
        for &text in expected {
            assert_eq!(contents.matches(text).count(), 1, "{text}: {contents}");
            let position = contents.find(text).unwrap();
            if let Some(previous) = previous {
                assert!(previous < position, "out of order: {contents}");
            }
            previous = Some(position);
        }
    };
    assert_order(
        &live,
        &[
            "[cross][ref]",
            "separate-message",
            "fn partial() {}",
            "after-fence",
            "Running stream(1)",
        ],
    );
    assert!(!live.contains("Tool:"));
    assert!(!live.contains("No committed result"));
    assert!(!live.contains("stream(2)"));
    let assert_markdown = |screen: &vt100::Screen| {
        let contents = screen.contents();
        for text in ["separate-message", "after-fence"] {
            let (row, line) = contents
                .lines()
                .enumerate()
                .find(|(_, line)| line.contains(text))
                .unwrap();
            let col = line.find(text).unwrap();
            assert!(
                screen.cell(row as u16, col as u16).unwrap().bold(),
                "message lost its Markdown document: {text}"
            );
        }
    };
    assert_markdown(pty.parser.screen());
    assert!(live.find("after-fence") < live.find("Running stream"));
    assert!(
        !live.contains("Result:"),
        "no execution result before release"
    );
    assert!(
        !String::from_utf8_lossy(&pty.raw[run_start..])
            .contains("\x1b]8;;https://example.test/cross"),
        "separate documents incorrectly resolved a reference"
    );
    pty.release();
    pty.wait_for(
        |p| p.frame_complete() && p.parser.screen().contents().contains("Completed ·"),
        "run completion",
    );
    // Final results appear once per executed invocation, not again at commit.
    assert_eq!(
        String::from_utf8_lossy(&pty.raw[run_start..])
            .matches("final-tool-result")
            .count(),
        2
    );
    let request: Value = serde_json::from_slice(
        &std::fs::read(pty.directory.path().join("request-1.json")).unwrap(),
    )
    .unwrap();
    let outputs: Vec<_> = request["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "function_call_output")
        .collect();
    assert_eq!(outputs.len(), 2);
    assert_eq!(outputs[0]["call_id"], "x");
    assert_eq!(outputs[1]["call_id"], "y");
    let executed = std::fs::metadata(pty.directory.path().join("executed"))
        .unwrap()
        .modified()
        .unwrap();
    let replay_start = pty.raw.len();
    pty.send(format!("/session switch {id}\r").as_bytes());
    pty.wait_for(
        |p| p.frame_complete() && p.parser.screen().contents().contains("switched to session"),
        "ordered replay",
    );
    let replay = pty.parser.screen().contents();
    assert_order(
        &replay,
        &[
            "[cross][ref]",
            "separate-message",
            "Ran stream(1)",
            "fn partial() {}",
            "Ran stream(2)",
            "after-fence",
        ],
    );
    assert_markdown(pty.parser.screen());
    assert!(!replay.contains("row-49"));
    assert_eq!(replay.matches("final-tool-result").count(), 2);
    assert!(
        !String::from_utf8_lossy(&pty.raw[replay_start..])
            .contains("\x1b]8;;https://example.test/cross")
    );
    assert_eq!(
        std::fs::metadata(pty.directory.path().join("executed"))
            .unwrap()
            .modified()
            .unwrap(),
        executed
    );
    pty.send(b"/exit\r");
    pty.finish();
}
