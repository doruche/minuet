use std::io::{self, IsTerminal, Write};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    style::{Attribute, ResetColor, SetAttribute},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal, TerminalOptions, Viewport,
    backend::{Backend, CrosstermBackend},
    widgets::{Paragraph, Widget},
};

use super::{
    input::Input,
    render::{self, OutputTail, Tone},
};

pub enum Screen {
    Inline(Box<Inline>),
    Plain { at_line_start: bool },
}

pub struct Inline {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    mode: TerminalMode,
    tail: OutputTail,
    colors: bool,
}

// Owns only terminal modes. It exists before fallible viewport setup, so both
// partial initialization failures and unwinding restore the terminal. Kernel
// shutdown runs after CLI teardown and does not own these resources.
struct TerminalMode {
    active: bool,
}

impl TerminalMode {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mode = Self { active: true };
        execute!(io::stdout(), EnableBracketedPaste, Hide)?;
        Ok(mode)
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        let modes = disable_raw_mode();
        let display = execute!(
            io::stdout(),
            DisableBracketedPaste,
            ResetColor,
            SetAttribute(Attribute::Reset),
            Show
        );
        self.active = modes.is_err() || display.is_err();
        modes.and(display)
    }
}

impl Drop for TerminalMode {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

impl Screen {
    pub fn open() -> io::Result<Self> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Ok(Self::Plain {
                at_line_start: true,
            });
        }
        let mode = TerminalMode::enter()?;
        let terminal = Terminal::with_options(
            CrosstermBackend::new(io::stdout()),
            TerminalOptions {
                viewport: Viewport::Inline(8),
            },
        )?;
        let colors = std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty());
        Ok(Self::Inline(Box::new(Inline {
            terminal,
            mode,
            tail: OutputTail::default(),
            colors,
        })))
    }

    pub fn interactive(&self) -> bool {
        matches!(self, Self::Inline(_))
    }

    pub fn fragment(&mut self, text: &str, tone: Tone) -> io::Result<()> {
        let text = render::safe_text(text);
        match self {
            Self::Plain { at_line_start } => {
                let mut stdout = io::stdout().lock();
                stdout.write_all(text.as_bytes())?;
                stdout.flush()?;
                if !text.is_empty() {
                    *at_line_start = text.ends_with('\n');
                }
                Ok(())
            },
            Self::Inline(inline) => {
                if inline.tail.tone != tone && !inline.tail.text.is_empty() {
                    inline.append("\n")?;
                }
                inline.tail.tone = tone;
                inline.append(&text)
            },
        }
    }

    pub fn line(&mut self, text: &str, tone: Tone) -> io::Result<()> {
        self.end_line()?;
        self.fragment(text, tone)?;
        if !text.ends_with('\n') {
            self.fragment("\n", tone)?;
        }
        Ok(())
    }

    fn end_line(&mut self) -> io::Result<()> {
        let pending = match self {
            Self::Plain { at_line_start } => !*at_line_start,
            Self::Inline(inline) => !inline.tail.text.is_empty(),
        };
        if pending {
            let tone = match self {
                Self::Inline(inline) => inline.tail.tone,
                _ => Tone::Text,
            };
            self.fragment("\n", tone)?;
        }
        Ok(())
    }

    pub fn draw(&mut self, input: &Input, status: &str, busy: bool) -> io::Result<()> {
        if let Self::Inline(inline) = self {
            inline.resize()?;
            inline.append("")?;
            inline.terminal.draw(|frame| {
                render::draw(frame, input, status, &inline.tail, busy, inline.colors)
            })?;
        }
        Ok(())
    }

    pub fn finish(&mut self) -> io::Result<()> {
        // Restore modes even if flushing or clearing fails. The guard retries
        // failed restoration on drop, while the explicit error stays visible.
        let flush = self.end_line();
        if let Self::Inline(inline) = self {
            let area = inline.terminal.get_frame().area();
            let clear = inline.terminal.clear();
            let cursor = execute!(inline.terminal.backend_mut(), MoveTo(0, area.y));
            let restore = inline.mode.restore();
            flush.and(clear).and(cursor).and(restore)
        } else {
            flush
        }
    }
}

impl Inline {
    fn resize(&mut self) -> io::Result<()> {
        let size = self.terminal.size()?;
        if size.width < self.terminal.get_frame().area().width {
            // Ratatui 0.30 clears the entire visible screen on horizontal shrink.
            // Archive it to terminal scrollback first so completed output is not
            // erased. This may retain an old input/status snapshot, never session
            // state. Remove this bridge when inline resize preserves output;
            // the PTY resize test guards that boundary. I/O failures propagate.
            self.terminal
                .set_cursor_position((0, size.height.saturating_sub(1)))?;
            self.terminal.backend_mut().append_lines(size.height)?;
            self.terminal.set_cursor_position((0, 0))?;
        }
        self.terminal.autoresize()
    }

    fn append(&mut self, text: &str) -> io::Result<()> {
        self.resize()?;
        let width = self.terminal.size()?.width;
        let rows = self.tail.push(text, width);
        // Bound insertion buffers even when a final result contains many lines.
        for batch in rows.chunks(128) {
            self.terminal.insert_before(batch.len() as u16, |buffer| {
                let lines: Vec<_> = batch
                    .iter()
                    .map(|text| render::line(text.clone(), self.tail.tone, self.colors))
                    .collect();
                Paragraph::new(lines).render(buffer.area, buffer);
            })?;
        }
        Ok(())
    }
}
