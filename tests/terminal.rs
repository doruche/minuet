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
            json!({"status":"completed", "output":[{"type":"message", "content":[{"type":"output_text", "text": std::fs::read_to_string(directory.join("answer.md")).unwrap_or_else(|_| "assistant-finished".into())}]}]}),
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
            |p| p.frame_complete() && p.parser.screen().contents().contains("assistant-finished"),
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
        assert_eq!(
            matches!(
                screen.cell(code_row, 0).unwrap().fgcolor(),
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
        |p| p.frame_complete() && p.parser.screen().contents().contains("run stopped:"),
        "limit notice",
    );
    let contents = pty.parser.screen().contents();
    assert!(
        contents.contains("fn partial() {}\nrun stopped:"),
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
            |p| p.frame_complete() && p.parser.screen().contents().contains("assistant-finished"),
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
