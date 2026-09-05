use minuet::{
    agent_loop::{RunOutcome, RunStopReason, ToolActivity, ToolActivityStatus},
    session::UsageSummary,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::Paragraph,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{command, handler::Effect, input::Input};

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
            Self::Meta => Style::default().fg(Color::DarkGray),
            Self::Error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            Self::User => Style::default().fg(Color::Cyan),
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
    input: &Input,
    status: &str,
    tail: &OutputTail,
    busy: bool,
    colors: bool,
) {
    let areas = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
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
    if busy {
        frame.render_widget(
            Paragraph::new("minuet> (busy)").style(Tone::Meta.style(colors)),
            areas[2],
        );
    } else {
        frame.render_widget(input.widget(), areas[2]);
    }
    let hint = if busy {
        "Ctrl-C: exit and wait for shutdown"
    } else {
        "Enter: send · Alt-Enter: newline · Ctrl-C: exit"
    };
    frame.render_widget(
        Paragraph::new(hint).style(Tone::Meta.style(colors)),
        areas[3],
    );
}

pub fn effect(effect: Effect) -> String {
    match effect {
        Effect::Exit => unreachable!("exit is handled by the interaction owner"),
        Effect::Help => command::help(),
        Effect::NewSession(id) => format!("started memory session {id}"),
        Effect::Model(info) => format!(
            "provider: {}\nmodel: {}\nreasoning effort: {}",
            info.provider,
            info.model,
            info.reasoning_effort
                .as_deref()
                .unwrap_or("upstream default")
        ),
        Effect::ReasoningEffortSet(Some(effort)) => format!("reasoning effort set to `{effort}`"),
        Effect::ReasoningEffortSet(None) => {
            "reasoning effort cleared; the upstream default will be used".into()
        },
        Effect::Tools(tools) => tools
            .into_iter()
            .map(|tool| {
                format!(
                    "{} ({}) — {}",
                    tool.name,
                    if tool.enabled { "enabled" } else { "disabled" },
                    tool.description
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Effect::ToolEnabled { name, enabled } => format!(
            "tool `{name}` {}",
            if enabled { "enabled" } else { "disabled" }
        ),
        Effect::Context(info) => {
            let tokens = match info.committed_input_tokens {
                minuet::kernel::InputTokenCount::Available(tokens) => {
                    format!("{tokens} (upstream count)")
                },
                minuet::kernel::InputTokenCount::Unavailable(error) => {
                    format!("unavailable ({error})")
                },
            };
            format!(
                "session: {}\ncommitted input tokens: {tokens}\n{}",
                info.session_id,
                usage(&info.usage)
            )
        },
    }
}

fn usage(usage: &UsageSummary) -> String {
    let latest = match usage.latest {
        Some(latest) => format!(
            "input {}, output {}, total {}",
            latest.input_tokens, latest.output_tokens, latest.total_tokens
        ),
        None if usage.reported_calls + usage.unreported_calls == 0 => "no model calls yet".into(),
        None => "unavailable for the latest model call".into(),
    };
    let qualifier = if usage.unreported_calls == 0 {
        ""
    } else {
        " (partial: at least one call omitted usage)"
    };
    format!(
        "last reported usage: {latest}\nsession reported usage: input {}, output {}, total {}{qualifier}",
        usage.input_tokens, usage.output_tokens, usage.total_tokens
    )
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
    fn renders_controls_as_text_in_both_output_modes() {
        assert_eq!(safe_text("a\x1b[2J\r\nb\t"), "a\\u{1b}[2J\\r\nb    ");
    }
}
