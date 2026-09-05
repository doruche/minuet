use ratatui::{Frame, layout::Rect, style::Modifier, text::Line, widgets::Paragraph};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{super::input::Input, Tone};

/// Immutable projection of Input's buffer and cursor at one terminal width.
/// Rebuilt for each draw, never edited or retained across input changes. Height,
/// wrapping and hardware cursor placement all use this same projection.
pub(in crate::cli) struct InputLayout {
    lines: Vec<String>,
    cursor: (usize, u16),
    prefix: u16,
}

impl InputLayout {
    pub fn new(input: &Input, width: u16) -> Self {
        // Keep at least one content column even on an extremely narrow terminal.
        let prefix = 2.min(width.saturating_sub(1));
        let width = usize::from(width.saturating_sub(prefix).max(1));
        let mut lines = vec![String::new()];
        let mut cursor = (0, 0);
        let position = input.cursor();
        for (row, text) in input.lines().iter().enumerate() {
            if row > 0 {
                lines.push(String::new());
            }
            let mut column = 0;
            let mut scalar = 0;
            for grapheme in text.graphemes(true) {
                let displayed = if grapheme == "\t" {
                    " ".repeat(4 - column % 4)
                } else if grapheme.width() > width {
                    // Match output's narrow-viewport policy: retain the identity
                    // of a glyph that cannot fit instead of silently losing it.
                    grapheme.chars().flat_map(char::escape_unicode).collect()
                } else {
                    grapheme.to_owned()
                };
                for (part, glyph) in displayed.graphemes(true).enumerate() {
                    let size = glyph.width();
                    if column + size > width {
                        lines.push(String::new());
                        column = 0;
                    }
                    if position == (row, scalar) && part == 0 {
                        cursor = (lines.len() - 1, column as u16);
                    }
                    lines.last_mut().unwrap().push_str(glyph);
                    column += size;
                }
                scalar += grapheme.chars().count();
            }
            if position == (row, scalar) {
                if column == width {
                    lines.push(String::new());
                    column = 0;
                }
                cursor = (lines.len() - 1, column as u16);
            }
        }
        Self {
            lines,
            cursor,
            prefix,
        }
    }

    pub fn height(&self) -> u16 {
        self.lines.len().min(8) as u16
    }

    pub fn draw(&self, frame: &mut Frame, area: Rect, colors: bool) {
        if area.is_empty() {
            return;
        }
        let top = self.cursor.0.saturating_sub(usize::from(area.height) - 1);
        if self.prefix > 0 {
            frame.render_widget(
                Paragraph::new("> ").style(if colors {
                    Tone::User.style(colors).add_modifier(Modifier::BOLD)
                } else {
                    Tone::User.style(colors)
                }),
                Rect::new(area.x, area.y, self.prefix, 1),
            );
        }
        let content = Rect::new(
            area.x + self.prefix,
            area.y,
            area.width.saturating_sub(self.prefix),
            area.height,
        );
        let lines: Vec<_> = self
            .lines
            .iter()
            .skip(top)
            .take(usize::from(area.height))
            .map(|line| Line::raw(line.as_str()))
            .collect();
        frame.render_widget(Paragraph::new(lines), content);
        // The terminal owns cursor appearance. Painting an inverse space over
        // half a wide glyph can also erase its other half with inverse style,
        // leaving a white cell invisible to the renderer's buffer diff.
        frame.set_cursor_position((
            content.x + self.cursor.1,
            content.y + (self.cursor.0 - top) as u16,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn wraps_graphemes_and_places_the_cursor_after_full_width_text() {
        let mut input = Input::default();
        input.handle(Event::Paste("中e\u{301}👩‍💻".into()), false);
        let layout = InputLayout::new(&input, 7);
        assert_eq!(layout.lines, ["中e\u{301}👩‍💻", ""]);
        assert_eq!(layout.cursor, (1, 0));
        input.handle(
            Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            false,
        );
        let layout = InputLayout::new(&input, 7);
        assert_eq!(layout.cursor, (0, 3));
    }

    #[test]
    fn manual_newlines_tabs_and_narrow_width_preserve_content() {
        let mut input = Input::default();
        input.handle(Event::Paste("中\tX\nlast".into()), false);
        let layout = InputLayout::new(&input, 10);
        assert_eq!(layout.lines, ["中  X", "last"]);
        assert_eq!(layout.cursor, (1, 4));
        let narrow = InputLayout::new(&input, 1);
        assert!(narrow.lines.concat().starts_with("\\u{4e2d}"));
        assert_eq!(narrow.cursor.1, 0);
    }
}
