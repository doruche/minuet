use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui_textarea::{CursorMove, TextArea};
use unicode_segmentation::UnicodeSegmentation;

pub enum Action {
    Edit,
    Submit(String),
    Exit,
}

pub struct Input {
    editor: TextArea<'static>,
}

impl Default for Input {
    fn default() -> Self {
        let mut editor = TextArea::default();
        editor.set_max_histories(0);
        Self { editor }
    }
}

impl Input {
    pub fn lines(&self) -> &[String] {
        self.editor.lines()
    }

    pub fn cursor(&self) -> (usize, usize) {
        let cursor = self.editor.cursor();
        (cursor.0, cursor.1)
    }

    pub fn handle(&mut self, event: Event, busy: bool) -> Action {
        if let Event::Key(key) = &event {
            if key.kind == KeyEventKind::Release {
                return Action::Edit;
            }
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Action::Exit;
            }
        }
        if busy {
            return Action::Edit;
        }
        match event {
            Event::Paste(text) => {
                // Bracketed paste is one edit, never a sequence of submit keys.
                let text = text.replace("\r\n", "\n").replace('\r', "\n");
                let mut safe = String::with_capacity(text.len());
                for c in text.chars() {
                    if c.is_control() && c != '\n' && c != '\t' {
                        safe.extend(c.escape_default());
                    } else {
                        safe.push(c);
                    }
                }
                self.editor.insert_str(safe);
            },
            Event::Key(key) => match (key.code, key.modifiers) {
                (KeyCode::Enter, KeyModifiers::NONE) => {
                    let text = self.editor.lines().join("\n");
                    *self = Self::default();
                    return Action::Submit(text);
                },
                (KeyCode::Enter, KeyModifiers::SHIFT)
                | (KeyCode::Char('o'), KeyModifiers::CONTROL) => self.editor.insert_newline(),
                (KeyCode::Char(c), modifiers)
                    if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.editor.insert_char(c);
                },
                (KeyCode::Tab, KeyModifiers::NONE) => {
                    self.editor.insert_char('\t');
                },
                (KeyCode::Backspace, KeyModifiers::NONE) => self.delete_grapheme(true),
                (KeyCode::Delete, KeyModifiers::NONE) => self.delete_grapheme(false),
                (KeyCode::Left, KeyModifiers::NONE) => self.move_grapheme(true),
                (KeyCode::Right, KeyModifiers::NONE) => self.move_grapheme(false),
                (KeyCode::Up, KeyModifiers::NONE) => self.move_vertical(CursorMove::Up),
                (KeyCode::Down, KeyModifiers::NONE) => self.move_vertical(CursorMove::Down),
                (KeyCode::Home, KeyModifiers::NONE) => self.move_to_column(0),
                (KeyCode::End, KeyModifiers::NONE) => {
                    let row = self.editor.cursor().0;
                    self.move_to_column(self.editor.lines()[row].chars().count());
                },
                _ => {},
            },
            _ => {},
        }
        Action::Edit
    }

    // The editing component owns the only buffer and uses scalar-value cursor offsets.
    // Adapt operations to grapheme boundaries through its public editing API;
    // retaining a second editable string would split input ownership.
    fn boundaries(&self) -> Vec<usize> {
        let line = &self.editor.lines()[self.editor.cursor().0];
        let mut count = 0;
        let mut boundaries = vec![0];
        for grapheme in line.graphemes(true) {
            count += grapheme.chars().count();
            boundaries.push(count);
        }
        boundaries
    }

    fn move_to_column(&mut self, target: usize) {
        while self.editor.cursor().1 > target {
            self.editor.move_cursor(CursorMove::Back);
        }
        while self.editor.cursor().1 < target {
            self.editor.move_cursor(CursorMove::Forward);
        }
    }

    fn move_grapheme(&mut self, back: bool) {
        let col = self.editor.cursor().1;
        let boundaries = self.boundaries();
        let target = if back {
            boundaries.into_iter().rev().find(|&p| p < col)
        } else {
            boundaries.into_iter().find(|&p| p > col)
        };
        match target {
            Some(target) => self.move_to_column(target),
            None => self.editor.move_cursor(if back {
                CursorMove::Back
            } else {
                CursorMove::Forward
            }),
        }
    }

    fn move_vertical(&mut self, movement: CursorMove) {
        self.editor.move_cursor(movement);
        let col = self.editor.cursor().1;
        let target = self
            .boundaries()
            .into_iter()
            .rev()
            .find(|&p| p <= col)
            .unwrap_or(0);
        self.move_to_column(target);
    }

    fn delete_grapheme(&mut self, back: bool) {
        let col = self.editor.cursor().1;
        let boundaries = self.boundaries();
        let range = boundaries.windows(2).find(|pair| {
            if back {
                pair[0] < col && col <= pair[1]
            } else {
                pair[0] <= col && col < pair[1]
            }
        });
        if let Some(&[start, end]) = range {
            self.move_to_column(end);
            for _ in start..end {
                self.editor.delete_char();
            }
        } else if back {
            self.editor.delete_char();
        } else {
            self.editor.delete_next_char();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn deletes_chinese_and_whole_combining_graphemes() {
        let mut input = Input::default();
        input.handle(Event::Paste("a中e\u{301}👩‍💻".into()), false);
        for expected in ["a中e\u{301}", "a中", "a", ""] {
            input.handle(key(KeyCode::Backspace), false);
            assert_eq!(input.editor.lines(), &[expected]);
        }
    }

    #[test]
    fn moves_and_deletes_in_the_middle_of_mixed_text() {
        let mut input = Input::default();
        input.handle(Event::Paste("中👩‍💻文".into()), false);
        input.handle(key(KeyCode::Left), false);
        input.handle(key(KeyCode::Left), false);
        input.handle(key(KeyCode::Delete), false);
        assert_eq!(input.editor.lines(), &["中文"]);
    }

    #[test]
    fn paste_preserves_lines_and_only_enter_submits() {
        let mut input = Input::default();
        assert!(matches!(
            input.handle(Event::Paste("  中文\r\n\tnext\r".into()), false),
            Action::Edit
        ));
        assert!(
            matches!(input.handle(key(KeyCode::Enter), false), Action::Submit(text) if text == "  中文\n\tnext\n")
        );
        input.handle(
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
            false,
        );
        assert_eq!(input.editor.lines(), &["", ""]);
    }

    #[test]
    fn busy_input_is_discarded_but_ctrl_c_requests_exit() {
        let mut input = Input::default();
        input.handle(Event::Paste("must not queue".into()), true);
        input.handle(key(KeyCode::Char('x')), true);
        input.handle(key(KeyCode::Enter), true);
        assert_eq!(input.editor.lines(), &[""]);
        assert!(matches!(
            input.handle(
                Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
                true
            ),
            Action::Exit
        ));
    }

    #[test]
    fn fallback_newline_and_key_release_do_not_submit() {
        let mut input = Input::default();
        input.handle(key(KeyCode::Char('a')), false);
        input.handle(
            Event::Key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)),
            false,
        );
        assert_eq!(input.lines(), &["a", ""]);
        assert!(matches!(
            input.handle(
                Event::Key(KeyEvent::new_with_kind(
                    KeyCode::Enter,
                    KeyModifiers::NONE,
                    KeyEventKind::Release
                )),
                false
            ),
            Action::Edit
        ));
        assert_eq!(input.lines(), &["a", ""]);
    }
}
