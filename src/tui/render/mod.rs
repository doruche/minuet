mod input;
pub(super) mod markdown;

pub(super) use input::InputLayout;

use minuet::agent_loop::{ToolActivity, ToolActivityStatus};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::Paragraph,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Keep model output aligned with the user's two-column `> ` input prefix.
pub(super) const CONTENT_PREFIX: u16 = 2;

/// An immutable view of the pending request, borrowed for one redraw.
#[derive(Clone, Copy)]
pub struct Status<'a> {
    pub label: &'a str,
    pub elapsed: std::time::Duration,
}

impl Status<'_> {
    fn text(self) -> String {
        // Animation signals an outstanding request, not remote progress. Derive
        // its frame from elapsed time so there is no second mutable clock.
        let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let frame = frames[(self.elapsed.as_millis() / 100 % frames.len() as u128) as usize];
        // Keep waiting time visible when the terminal clips a long phase label.
        format!(
            "{frame} {} · {}",
            elapsed(self.elapsed),
            safe_text(self.label)
        )
    }
}

pub fn elapsed(duration: std::time::Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!(
            "{}h {:02}m {:02}s",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Tone {
    #[default]
    Text,
    Command,
    Meta,
    Error,
    User,
}

impl Tone {
    pub fn style(self, colors: bool) -> Style {
        if !colors {
            return Style::default();
        }
        match self {
            Self::Text => Style::default(),
            Self::Command => Style::default().fg(Color::DarkGray),
            Self::Meta => Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
            Self::Error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            Self::User => Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        }
    }
}

/// Only the renderer emits terminal controls. Tool/model text and command
/// errors may contain arbitrary controls; show them literally, including CR,
/// instead of letting them move the cursor or modify the terminal state.
pub fn safe_text(text: &str) -> String {
    let mut safe = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' => safe.push(c),
            '\t' => safe.push_str("    "),
            c if c.is_control() => safe.extend(c.escape_default()),
            c => safe.push(c),
        }
    }
    safe
}

/// The unfinished visual line is a render buffer, not conversation history.
/// Its last grapheme stays mutable so a following fragment can extend a
/// combining sequence. Completed rows leave this owner for terminal scrollback.
#[derive(Default)]
pub struct OutputTail {
    pub text: String,
    pub tone: Tone,
    pub indent: u16,
}

impl OutputTail {
    pub fn push(&mut self, text: &str, width: u16) -> Vec<String> {
        self.text.push_str(text);
        let width = usize::from(width.max(1));
        let mut ready = Vec::new();
        let mut line = String::new();
        let mut columns = 0;
        for grapheme in self.text.graphemes(true) {
            if grapheme == "\n" {
                ready.push(std::mem::take(&mut line));
                columns = 0;
                continue;
            }
            let size = grapheme.width();
            if size > width {
                // A one-column viewport cannot render a wide glyph. Escaping
                // preserves its identity instead of silently clipping it away.
                for c in grapheme.chars().flat_map(char::escape_unicode) {
                    if columns == width {
                        ready.push(std::mem::take(&mut line));
                        columns = 0;
                    }
                    line.push(c);
                    columns += 1;
                }
                continue;
            }
            if columns + size > width {
                ready.push(std::mem::take(&mut line));
                columns = 0;
            }
            line.push_str(grapheme);
            columns += size;
        }
        self.text = line;
        ready
    }
}

/// A bounded view of the latest preview rows. Rewrap only the temporary
/// display; completed Markdown retains its existing scrollback owner.
pub fn preview_lines(text: &str, width: u16, available_rows: u16) -> Vec<String> {
    if text.is_empty() || available_rows == 0 {
        return Vec::new();
    }
    let mut tail = OutputTail::default();
    let mut rows = tail.push(text, width.saturating_sub(CONTENT_PREFIX));
    rows.push(tail.text);
    let height = usize::from(available_rows.min(6));
    if rows.len() > height {
        rows.drain(..rows.len() - height);
    }
    rows
}

pub fn draw(
    frame: &mut Frame,
    input: &InputLayout,
    status: Option<Status<'_>>,
    tail: &OutputTail,
    preview: &[String],
    colors: bool,
    shift_enter: bool,
) {
    let areas = Layout::vertical([
        Constraint::Length(preview.len() as u16),
        Constraint::Length(u16::from(!tail.text.is_empty())),
        Constraint::Length(u16::from(status.is_some())),
        Constraint::Min(1),
        Constraint::Length(u16::from(frame.area().height > 1)),
    ])
    .split(frame.area());
    frame.render_widget(
        Paragraph::new(
            preview
                .iter()
                .map(|row| Line::raw(format!("  {row}")))
                .collect::<Vec<_>>(),
        )
        .style(Tone::Text.style(colors)),
        areas[0],
    );
    let prefix = tail.indent;
    let output_area = ratatui::layout::Rect::new(
        areas[1].x + prefix.min(areas[1].width),
        areas[1].y,
        areas[1].width.saturating_sub(prefix),
        areas[1].height,
    );
    frame.render_widget(
        Paragraph::new(tail.text.as_str()).style(tail.tone.style(colors)),
        output_area,
    );
    if let Some(status) = status {
        frame.render_widget(
            Paragraph::new(status.text()).style(Tone::Meta.style(colors)),
            areas[2],
        );
    } else {
        input.draw(frame, areas[3], colors);
    }
    let hint = if status.is_some() {
        "Ctrl-C: exit and wait for shutdown"
    } else if shift_enter {
        "Enter: send · Shift-Enter / Ctrl-O: newline · Ctrl-C: exit"
    } else {
        "Enter: send · Ctrl-O: newline · Ctrl-C: exit"
    };
    frame.render_widget(
        Paragraph::new(hint).style(Tone::Meta.style(colors)),
        areas[4],
    );
}

pub fn tool_result(activity: &ToolActivity, elapsed: std::time::Duration) -> (String, Tone) {
    let (verb, tone) = match activity.status {
        ToolActivityStatus::Completed => ("Ran", Tone::Meta),
        ToolActivityStatus::Error => ("Failed", Tone::Error),
        ToolActivityStatus::Skipped => ("Skipped", Tone::Meta),
    };
    let duration = if activity.status == ToolActivityStatus::Skipped {
        String::new()
    } else {
        format!(" · {:.2}s", elapsed.as_secs_f64())
    };
    (format!("{verb} {}{duration}", activity.display.call), tone)
}

pub fn line(text: String, tone: Tone, colors: bool) -> Line<'static> {
    Line::styled(text, tone.style(colors))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_has_its_own_rows_and_keeps_the_latest_text_after_resize() {
        let text = "old\n中文e\u{301}\nlatest";
        let preview = preview_lines(text, 20, 2);
        assert_eq!(preview, ["中文e\u{301}", "latest"]);
        assert_eq!(preview_lines(text, 20, 1), ["latest"]);
        assert!(preview_lines(text, 20, 0).is_empty());
        for tail_text in ["", "tool tail"] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(20, 6)).unwrap();
            let input = InputLayout::new(&crate::tui::input::Input::default(), 20);
            let tail = OutputTail {
                text: tail_text.into(),
                tone: Tone::Meta,
                indent: 0,
            };
            terminal
                .draw(|frame| {
                    draw(
                        frame,
                        &input,
                        Some(Status {
                            label: "Waiting",
                            elapsed: std::time::Duration::ZERO,
                        }),
                        &tail,
                        &preview,
                        true,
                        false,
                    )
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let row: String = (0..20).map(|x| buffer[(x, 1)].symbol()).collect();
            assert_eq!(row.trim_end(), "  latest");
            assert_eq!(buffer[(2, 1)].fg, Color::Reset);
            if !tail_text.is_empty() {
                let row: String = (0..20).map(|x| buffer[(x, 2)].symbol()).collect();
                assert_eq!(row.trim_end(), tail_text);
            }
        }
    }

    #[test]
    fn narrow_status_keeps_elapsed_time_beside_unfinished_output() {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(20, 4)).unwrap();
        let input = InputLayout::new(&crate::tui::input::Input::default(), 20);
        let tail = OutputTail {
            text: "partial output".into(),
            tone: Tone::Text,
            indent: CONTENT_PREFIX,
        };
        for (seconds, time) in [
            (59, "59s"),
            (60, "1m 00s"),
            (3599, "59m 59s"),
            (3600, "1h 00m 00s"),
            (3661, "1h 01m 01s"),
        ] {
            terminal
                .draw(|frame| {
                    draw(
                        frame,
                        &input,
                        Some(Status {
                            label: "Waiting for model…",
                            elapsed: std::time::Duration::from_secs(seconds),
                        }),
                        &tail,
                        &[],
                        false,
                        false,
                    );
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let output: String = (0..20).map(|x| buffer[(x, 0)].symbol()).collect();
            let status: String = (0..20).map(|x| buffer[(x, 1)].symbol()).collect();
            assert_eq!(output.trim_end(), "  partial output");
            assert!(status.starts_with(&format!("⠋ {time} · ")), "{status}");
        }
    }

    #[test]
    fn displays_unterminated_fragments_and_preserves_combining_boundaries() {
        let mut tail = OutputTail::default();
        assert_eq!(tail.push("开始e", 5), Vec::<String>::new());
        assert_eq!(tail.text, "开始e");
        assert_eq!(tail.push("\u{301}后\n", 5), vec!["开始e\u{301}", "后"]);
        assert_eq!(tail.text, "");
    }

    #[test]
    fn rewraps_pending_output_after_resize_without_losing_text() {
        let mut tail = OutputTail::default();
        tail.push("中文abc", 12);
        assert_eq!(tail.push("", 4), vec!["中文"]);
        assert_eq!(tail.text, "abc");
    }

    #[test]
    fn renders_controls_as_text() {
        assert_eq!(safe_text("a\x1b[2J\r\nb\t"), "a\\u{1b}[2J\\r\nb    ");
    }
}
