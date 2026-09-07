mod backend;
mod markdown;
mod preview;

use backend::InlineBackend;

use std::time::Instant;
use std::{
    io::{self, Write},
    sync::Arc,
};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    style::{Attribute, Print, ResetColor, SetAttribute},
    terminal::{
        BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate, disable_raw_mode,
        enable_raw_mode, supports_keyboard_enhancement,
    },
};
use preview::Preview;
use ratatui::{
    Terminal, TerminalOptions, Viewport,
    widgets::{Paragraph, Widget},
};

use super::{
    input::Input,
    render::{self, CONTENT_PREFIX, OutputTail, Tone},
};
use minuet::session::{ToolExecution, TranscriptEntry};

pub struct Screen {
    terminal: Terminal<InlineBackend>,
    mode: TerminalMode,
    tail: OutputTail,
    preview: Preview,
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
        let link = execute!(io::stdout(), Print(markdown::RESET_LINK));
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
        self.active = link.is_err() || modes.is_err() || keyboard.is_err() || display.is_err();
        link.and(modes).and(keyboard).and(display)
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
            preview: Preview::default(),
            colors,
        })
    }

    fn styled_fragment(&mut self, text: &str, tone: Tone, indent: u16) -> io::Result<()> {
        let text = render::safe_text(text);
        if (self.tail.tone != tone || self.tail.indent != indent) && !self.tail.text.is_empty() {
            self.append("\n")?;
        }
        self.tail.tone = tone;
        self.tail.indent = indent;
        self.append(&text)
    }

    pub fn tool_fragment(&mut self, text: &str) -> io::Result<()> {
        self.styled_fragment(text, Tone::Meta, CONTENT_PREFIX)
    }

    pub fn tool_body(&mut self, text: &str) -> io::Result<()> {
        self.end_line()?;
        self.tool_fragment(text)?;
        if !text.ends_with('\n') {
            self.append("\n")?;
        }
        Ok(())
    }

    pub fn line(&mut self, text: &str, tone: Tone) -> io::Result<()> {
        self.end_line()?;
        self.styled_fragment(text, tone, 0)?;
        if !text.ends_with('\n') {
            self.append("\n")?;
        }
        Ok(())
    }

    fn markdown(&mut self, source: &str) -> io::Result<()> {
        self.preview.clear();
        self.end_line()?;
        self.terminal.autoresize()?;
        let area = self.terminal.get_frame().area();
        self.terminal.clear()?;
        self.terminal.set_cursor_position((0, area.y))?;
        // Ratatui 0.30 cells cannot carry hyperlink destinations. Withdraw its
        // viewport before writing completed rich blocks, then reanchor fresh
        // buffers at the actual cursor (including native scrollback movement).
        // Screen alone owns this handoff, including restoration on write error.
        // Use insert_before for this path when Ratatui can preserve OSC 8 links.
        let output = (|| {
            for (index, block) in render::markdown::blocks(source).enumerate() {
                let block = block.map_err(io::Error::other)?;
                let width = self.terminal.size()?.width.saturating_sub(CONTENT_PREFIX);
                if index > 0 {
                    write!(
                        self.terminal.backend_mut(),
                        "{}",
                        " ".repeat(CONTENT_PREFIX as usize)
                    )?;
                    markdown::write_row(self.terminal.backend_mut(), &Vec::new(), self.colors)?;
                }
                for row in block.rows(width) {
                    write!(
                        self.terminal.backend_mut(),
                        "{}",
                        " ".repeat(CONTENT_PREFIX as usize)
                    )?;
                    markdown::write_row(self.terminal.backend_mut(), &row, self.colors)?;
                }
                Write::flush(self.terminal.backend_mut())?;
            }
            Ok(())
        })();
        let anchor = Terminal::with_options(
            InlineBackend::new(),
            TerminalOptions {
                viewport: Viewport::Inline(area.height),
            },
        )
        .map(|terminal| self.terminal = terminal);
        output.and(anchor)
    }

    fn end_line(&mut self) -> io::Result<()> {
        if !self.tail.text.is_empty() {
            self.append("\n")?;
        }
        Ok(())
    }

    pub fn draw(&mut self, input: &Input, status: Option<render::Status<'_>>) -> io::Result<()> {
        execute!(self.terminal.backend_mut(), BeginSynchronizedUpdate)?;
        let draw = self.draw_frame(input, status);
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

    pub fn restore_modes(&mut self) -> io::Result<()> {
        self.mode.restore()
    }

    /// Replace the current inline view with the selected session's semantic
    /// transcript. Replay only publishes stored text; it never invokes work.
    pub fn replay(&mut self, entries: &[Arc<TranscriptEntry>]) -> io::Result<()> {
        self.clear_display()?;
        self.publish_entries(entries)
    }

    /// Clear terminal publication only; session history remains owned by the
    /// repository and a later session switch can replay it in full.
    pub fn clear_display(&mut self) -> io::Result<()> {
        self.preview.clear();
        self.tail = OutputTail::default();
        let area = self.terminal.get_frame().area();
        // Inline viewports normally preserve terminal scrollback. Explicit
        // display clearing must withdraw that publication too, otherwise old
        // messages remain visible above the fresh view. ED3 is supported
        // by terminals that implement native scrollback erasure; it is sent
        // explicitly alongside ED2 and a cursor home. Errors stay observable.
        execute!(
            self.terminal.backend_mut(),
            Clear(ClearType::All),
            Clear(ClearType::Purge),
            MoveTo(0, 0)
        )?;
        self.terminal = Terminal::with_options(
            InlineBackend::new(),
            TerminalOptions {
                viewport: Viewport::Inline(area.height),
            },
        )?;
        Ok(())
    }

    /// Live messages retain document boundaries but calls appear only when
    /// actually executed or explicitly skipped, not as an advance pending list.
    pub fn publish_model_messages(&mut self, entries: &[Arc<TranscriptEntry>]) -> io::Result<()> {
        self.preview.clear();
        // Withdraw the preview's height before publication; a full-height
        // anchor would push the completed answer entirely into scrollback.
        // Do this even when the committed turn contains only tool calls.
        self.resize_viewport(2)?;
        for entry in entries {
            if let TranscriptEntry::ModelMessage { text } = entry.as_ref() {
                self.markdown(text)?;
            }
        }
        Ok(())
    }

    /// Replay consumes stored semantic order and display snapshots only.
    fn publish_entries(&mut self, entries: &[Arc<TranscriptEntry>]) -> io::Result<()> {
        self.preview.clear();
        for entry in entries {
            match entry.as_ref() {
                TranscriptEntry::UserMessage { text } => {
                    self.line(&format!("> {text}"), Tone::User)?
                },
                // The normal Markdown publication path also withdraws and
                // reanchors Ratatui's inline viewport. Without that handoff,
                // the following replay entry can overwrite this model row.
                TranscriptEntry::ModelMessage { text } => self.markdown(text)?,
                TranscriptEntry::ToolInvocation {
                    name, execution, ..
                } => self.tool_execution(name, execution)?,
            }
        }
        Ok(())
    }

    pub fn tool_execution(&mut self, name: &str, execution: &ToolExecution) -> io::Result<()> {
        match execution {
            ToolExecution::Pending => self.line(
                &format!("{name}: result was not committed; execution may have occurred"),
                Tone::Error,
            ),
            ToolExecution::Completed(display) => {
                self.line(&format!("Ran {}", display.call), Tone::Meta)?;
                self.line("Result:", Tone::Meta)?;
                self.tool_body(&display.output)
            },
            ToolExecution::Failed(display) => {
                self.line(&format!("Failed {}", display.call), Tone::Error)?;
                self.line("Result:", Tone::Meta)?;
                self.tool_body(&display.output)
            },
            ToolExecution::Skipped(display) => {
                self.line(&format!("Skipped {}", display.call), Tone::Meta)?;
                self.tool_body(&display.output)
            },
        }
    }

    fn draw_frame(&mut self, input: &Input, status: Option<render::Status<'_>>) -> io::Result<()> {
        self.append("")?;
        let size = self.terminal.size()?;
        let layout = render::InputLayout::new(input, size.width);
        let reserved = 2 + u16::from(status.is_some()) + u16::from(!self.tail.text.is_empty());
        let preview = render::preview_lines(
            self.preview.visible(),
            size.width,
            size.height.saturating_sub(reserved),
        );
        let height = (preview.len() as u16
            + if status.is_some() { 1 } else { layout.height() }
            + 1
            + u16::from(!self.tail.text.is_empty())
            + u16::from(status.is_some()))
        .min(size.height)
        .max(1);
        self.resize_viewport(height)?;
        let shift_enter = cfg!(windows) || self.mode.keyboard_enhanced;
        self.terminal.draw(|frame| {
            render::draw(
                frame,
                &layout,
                status,
                &self.tail,
                &preview,
                self.colors,
                shift_enter,
            )
        })?;
        Ok(())
    }

    fn resize_viewport(&mut self, height: u16) -> io::Result<()> {
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
        Ok(())
    }

    pub fn model_delta(&mut self, text: &str) {
        self.preview.push(&render::safe_text(text), Instant::now());
    }

    pub fn advance_model_preview(&mut self) -> bool {
        self.preview.advance(Instant::now())
    }

    pub fn model_failed(&mut self) -> io::Result<()> {
        // Failure ends pacing immediately. Publish the entire received draft
        // as literal text so it survives the next request and terminal exit.
        let draft = self.preview.take();
        if !draft.is_empty() {
            // As with committed Markdown, release the preview's height before
            // inserting the draft so the failure remains on the visible screen.
            self.resize_viewport(2)?;
            self.end_line()?;
            self.styled_fragment(&draft, Tone::Text, CONTENT_PREFIX)?;
            self.end_line()?;
            self.line("[response incomplete]", Tone::Error)?;
        }
        Ok(())
    }

    fn append(&mut self, text: &str) -> io::Result<()> {
        self.terminal.autoresize()?;
        let width = self.terminal.size()?.width;
        let prefix = self.tail.indent;
        let rows = self.tail.push(text, width.saturating_sub(prefix));
        // Bound insertion buffers even when a final result contains many lines.
        for batch in rows.chunks(128) {
            self.terminal.insert_before(batch.len() as u16, |buffer| {
                let lines: Vec<_> = batch
                    .iter()
                    .map(|text| {
                        render::line(
                            format!("{}{}", " ".repeat(prefix as usize), text),
                            self.tail.tone,
                            self.colors,
                        )
                    })
                    .collect();
                Paragraph::new(lines).render(buffer.area, buffer);
            })?;
        }
        Ok(())
    }
}
