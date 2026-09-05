mod input;

pub(super) use input::InputLayout;

use minuet::agent_loop::{RunOutcome, RunStopReason, ToolActivity, ToolActivityStatus};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::Paragraph,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Tone {
    #[default]
    Text,
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

pub fn draw(
    frame: &mut Frame,
    input: &InputLayout,
    status: &str,
    tail: &OutputTail,
    busy: bool,
    colors: bool,
    shift_enter: bool,
) {
    let areas = Layout::vertical([
        Constraint::Length(u16::from(!tail.text.is_empty())),
        Constraint::Length(u16::from(!status.is_empty())),
        Constraint::Min(1),
        Constraint::Length(u16::from(frame.area().height > 1)),
    ])
    .split(frame.area());
    frame.render_widget(
        Paragraph::new(tail.text.as_str()).style(tail.tone.style(colors)),
        areas[0],
    );
    frame.render_widget(
        Paragraph::new(safe_text(status)).style(Tone::Meta.style(colors)),
        areas[1],
    );
    if !busy {
        input.draw(frame, areas[2], colors);
    }
    let hint = if busy {
        "Ctrl-C: exit and wait for shutdown"
    } else if shift_enter {
        "Enter: send · Shift-Enter / Ctrl-O: newline · Ctrl-C: exit"
    } else {
        "Enter: send · Ctrl-O: newline · Ctrl-C: exit"
    };
    frame.render_widget(
        Paragraph::new(hint).style(Tone::Meta.style(colors)),
        areas[3],
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
    (format!("{verb} {}{duration}", activity.name), tone)
}

pub fn outcome(outcome: &RunOutcome) -> String {
    let text = if outcome.text.is_empty() {
        "(no text output)"
    } else {
        &outcome.text
    };
    if outcome.stop_reason == RunStopReason::StepLimit {
        format!(
            "{text}\nrun stopped: model-turn limit reached; pending tool calls were not executed"
        )
    } else {
        text.to_owned()
    }
}

pub fn line(text: String, tone: Tone, colors: bool) -> Line<'static> {
    Line::styled(text, tone.style(colors))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn markdown_produces_styled_blocks() {
        let text = tui_markdown::from_str("# Title\n\n**bold** and `code`");
        assert!(text.lines.len() >= 2);
        let rendered = text
            .lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("Title"));
        assert!(rendered.contains("bold"));
        assert!(rendered.contains("code"));
    }
}
