mod backend;

use backend::InlineBackend;

use std::io::{self, IsTerminal, Write};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    style::{Attribute, ResetColor, SetAttribute},
    terminal::{
        BeginSynchronizedUpdate, EndSynchronizedUpdate, disable_raw_mode, enable_raw_mode,
        supports_keyboard_enhancement,
    },
};
use ratatui::{
    Terminal, TerminalOptions, Viewport,
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
    terminal: Terminal<InlineBackend>,
    mode: TerminalMode,
    tail: OutputTail,
    colors: bool,
}

// Owns only terminal modes. It exists before fallible viewport setup, so both
// partial initialization failures and unwinding restore the terminal. Kernel
// shutdown runs after TUI teardown and does not own these resources.
struct TerminalMode {
    active: bool,
    keyboard_enhanced: bool,
}

impl TerminalMode {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut mode = Self {
            active: true,
            keyboard_enhanced: false,
        };
        execute!(io::stdout(), EnableBracketedPaste, Hide)?;
        // Capability discovery is optional. A timeout/unrecognized reply leaves
        // Ctrl-O as the advertised newline key; it must not disable editing.
        // Windows' native console events already carry Shift separately.
        if supports_keyboard_enhancement().unwrap_or(false) {
            mode.keyboard_enhanced = true;
            execute!(
                io::stdout(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )?;
        }
        Ok(mode)
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        let modes = disable_raw_mode();
        let keyboard = if self.keyboard_enhanced {
            let result = execute!(io::stdout(), PopKeyboardEnhancementFlags);
            if result.is_ok() {
                self.keyboard_enhanced = false;
            }
            result
        } else {
            Ok(())
        };
        let display = execute!(
            io::stdout(),
            DisableBracketedPaste,
            ResetColor,
            SetAttribute(Attribute::Reset),
            EndSynchronizedUpdate,
            Show
        );
        self.active = modes.is_err() || keyboard.is_err() || display.is_err();
        modes.and(keyboard).and(display)
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
            InlineBackend::new(),
            TerminalOptions {
                viewport: Viewport::Inline(2),
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
            execute!(inline.terminal.backend_mut(), BeginSynchronizedUpdate)?;
            let draw = inline.draw(input, status, busy);
            let end = execute!(inline.terminal.backend_mut(), EndSynchronizedUpdate);
            draw.and(end)?;
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
    fn draw(&mut self, input: &Input, status: &str, busy: bool) -> io::Result<()> {
        self.append("")?;
        let size = self.terminal.size()?;
        let layout = render::InputLayout::new(input, size.width);
        let height = (if busy { 1 } else { layout.height() }
            + 1
            + u16::from(!self.tail.text.is_empty())
            + u16::from(!status.is_empty()))
        .min(size.height)
        .max(1);
        if height != self.terminal.get_frame().area().height {
            // Ratatui 0.30 has no API to change an inline viewport's requested
            // height. Recreate only its display buffers at the same origin;
            // modes and unfinished output remain owned by this Inline. Replace
            // this with a height setter when ratatui provides one.
            let origin = self.terminal.get_frame().area().y;
            self.terminal.clear()?;
            self.terminal.set_cursor_position((0, origin))?;
            self.terminal = Terminal::with_options(
                InlineBackend::new(),
                TerminalOptions {
                    viewport: Viewport::Inline(height),
                },
            )?;
        }
        let shift_enter = cfg!(windows) || self.mode.keyboard_enhanced;
        self.terminal.draw(|frame| {
            render::draw(
                frame,
                &layout,
                status,
                &self.tail,
                busy,
                self.colors,
                shift_enter,
            )
        })?;
        Ok(())
    }

    fn append(&mut self, text: &str) -> io::Result<()> {
        self.terminal.autoresize()?;
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
