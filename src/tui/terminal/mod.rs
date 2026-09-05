mod backend;

use backend::InlineBackend;

use std::io;

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
    text::Text,
    widgets::{Paragraph, Widget},
};

use super::{
    input::Input,
    render::{self, OutputTail, Tone},
};

pub struct Screen {
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
        let mode = TerminalMode::enter()?;
        let terminal = Terminal::with_options(
            InlineBackend::new(),
            TerminalOptions {
                viewport: Viewport::Inline(2),
            },
        )?;
        let colors = std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty());
        Ok(Self {
            terminal,
            mode,
            tail: OutputTail::default(),
            colors,
        })
    }

    pub fn fragment(&mut self, text: &str, tone: Tone) -> io::Result<()> {
        let text = render::safe_text(text);
        if self.tail.tone != tone && !self.tail.text.is_empty() {
            self.append("\n")?;
        }
        self.tail.tone = tone;
        self.append(&text)
    }

    pub fn line(&mut self, text: &str, tone: Tone) -> io::Result<()> {
        self.end_line()?;
        self.fragment(text, tone)?;
        if !text.ends_with('\n') {
            self.fragment("\n", tone)?;
        }
        Ok(())
    }

    /// Render a complete model answer as Markdown after its block structure is
    /// known. Tool and status output deliberately continue through `line` so
    /// incidental punctuation in diagnostics cannot become formatting.
    pub fn markdown(&mut self, markdown: &str) -> io::Result<()> {
        self.end_line()?;
        let rendered = tui_markdown::from_str(markdown);
        self.terminal.autoresize()?;
        let lines = rendered.lines.len().max(1) as u16;
        self.terminal.insert_before(lines, |buffer| {
            Text::from(rendered.clone()).render(buffer.area, buffer);
        })?;
        Ok(())
    }

    fn end_line(&mut self) -> io::Result<()> {
        if !self.tail.text.is_empty() {
            self.fragment("\n", self.tail.tone)?;
        }
        Ok(())
    }

    pub fn draw(&mut self, input: &Input, status: &str, busy: bool) -> io::Result<()> {
        execute!(self.terminal.backend_mut(), BeginSynchronizedUpdate)?;
        let draw = self.draw_frame(input, status, busy);
        let end = execute!(self.terminal.backend_mut(), EndSynchronizedUpdate);
        draw.and(end)
    }

    pub fn finish(&mut self) -> io::Result<()> {
        // Restore modes even if flushing or clearing fails. The guard retries
        // failed restoration on drop, while the explicit error stays visible.
        let flush = self.end_line();
        let area = self.terminal.get_frame().area();
        let clear = self.terminal.clear();
        let cursor = execute!(self.terminal.backend_mut(), MoveTo(0, area.y));
        let restore = self.mode.restore();
        flush.and(clear).and(cursor).and(restore)
    }

    fn draw_frame(&mut self, input: &Input, status: &str, busy: bool) -> io::Result<()> {
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
            // modes and unfinished output remain owned by this Screen. Replace
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
