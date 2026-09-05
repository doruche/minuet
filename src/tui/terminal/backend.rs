use std::io::{self, Write};

use ratatui::{
    backend::{Backend, ClearType, CrosstermBackend, WindowSize},
    buffer::Cell,
    layout::{Position, Size},
};
use unicode_width::UnicodeWidthStr;

/// Main-screen output must survive ratatui's full clear on horizontal shrink.
/// Enforce that at the destructive operation, including automatic resizes inside
/// draw/insert_before. Checking size before those calls leaves a resize race.
/// Remove this adapter when ratatui's inline resize preserves completed output.
/// No input/session state depends on the archived display; I/O errors propagate.
pub(super) struct InlineBackend(CrosstermBackend<io::Stdout>);

impl InlineBackend {
    pub fn new() -> Self {
        Self(CrosstermBackend::new(io::stdout()))
    }
}

impl Write for InlineBackend {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        Write::flush(&mut self.0)
    }
}

impl Backend for InlineBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        // Non-region insert_before emits every buffer cell. Writing a wide
        // glyph's continuation cells again inserts spaces in Crossterm output.
        // Remove this filter when Ratatui's draw_lines skips those cells, as its
        // ordinary buffer diff already does. This only adapts display cells.
        let mut covered = None;
        self.0.draw(content.filter(|(x, y, cell)| {
            if covered.is_some_and(|(row, start, end)| *y == row && *x > start && *x < end) {
                return false;
            }
            let width = cell.symbol().width().min(u16::MAX as usize) as u16;
            covered = Some((*y, *x, x.saturating_add(width)));
            true
        }))
    }
    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.0.append_lines(n)
    }
    fn hide_cursor(&mut self) -> io::Result<()> {
        self.0.hide_cursor()
    }
    fn show_cursor(&mut self) -> io::Result<()> {
        self.0.show_cursor()
    }
    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.0.get_cursor_position()
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.0.set_cursor_position(position)
    }
    fn clear(&mut self) -> io::Result<()> {
        self.clear_region(ClearType::All)
    }
    fn clear_region(&mut self, region: ClearType) -> io::Result<()> {
        if region == ClearType::All {
            let size = self.0.size()?;
            self.0
                .set_cursor_position((0, size.height.saturating_sub(1)))?;
            self.0.append_lines(size.height)?;
        }
        self.0.clear_region(region)
    }
    fn size(&self) -> io::Result<Size> {
        self.0.size()
    }
    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.0.window_size()
    }
    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.0)
    }
}
