//! Completed Markdown blocks are display data, not a second conversation store.
//! Parse the whole answer so reference links resolve across blocks; retain only
//! one top-level block's formatted lines while publishing to terminal scrollback.

use std::sync::{Arc, LazyLock};

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, ThemeSet},
    parsing::SyntaxSet,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::safe_text;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEMES: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::tui) struct Segment {
    pub text: String,
    pub style: Style,
    pub link: Option<Arc<str>>,
}

pub(in crate::tui) type Row = Vec<Segment>;

#[derive(Default)]
struct LogicalLine {
    spans: Row,
    continuation: String,
    clip: bool,
}

pub(in crate::tui) struct Block(Vec<LogicalLine>);

impl Block {
    /// Layout happens immediately before publication. Only tables are clipped;
    /// prose and code preserve every grapheme, including on one-column screens.
    pub fn rows(&self, width: u16) -> Vec<Row> {
        let width = usize::from(width.max(1));
        let mut rows = Vec::new();
        for line in &self.0 {
            let mut row = Vec::new();
            let mut columns = 0;
            // Segment boundaries may split a combining sequence. Segment the
            // entire line, assigning each grapheme the style of its first byte.
            let text: String = line.spans.iter().map(|span| span.text.as_str()).collect();
            let mut spans = line.spans.iter();
            let mut span = spans.next();
            let mut end = span.map_or(0, |span| span.text.len());
            'graphemes: for (offset, grapheme) in text.grapheme_indices(true) {
                while offset >= end {
                    span = spans.next();
                    end += span.map_or(0, |span| span.text.len());
                }
                let span = span.expect("nonempty grapheme belongs to a segment");
                let escaped;
                let units: Vec<&str> = if grapheme.width() > width && !line.clip {
                    escaped = grapheme
                        .chars()
                        .flat_map(char::escape_unicode)
                        .collect::<String>();
                    escaped.graphemes(true).collect()
                } else {
                    vec![grapheme]
                };
                for unit in units {
                    if columns + unit.width() > width {
                        if line.clip {
                            break 'graphemes;
                        }
                        rows.push(std::mem::take(&mut row));
                        // A prefix must leave room for the next glyph; deeply
                        // nested lists in narrow terminals still make progress.
                        let prefix = line
                            .continuation
                            .graphemes(true)
                            .scan(0, |size, g| {
                                *size += g.width();
                                (*size + unit.width() <= width).then_some(g)
                            })
                            .collect::<String>();
                        columns = prefix.width();
                        append(
                            &mut row,
                            Segment {
                                text: prefix,
                                ..Segment::default()
                            },
                        );
                    }
                    columns += unit.width();
                    append(
                        &mut row,
                        Segment {
                            text: unit.into(),
                            style: span.style,
                            link: span.link.clone(),
                        },
                    );
                }
            }
            rows.push(row);
        }
        rows
    }
}

fn append(row: &mut Row, span: Segment) {
    if span.text.is_empty() {
        return;
    }
    if let Some(last) = row.last_mut()
        && last.style == span.style
        && last.link == span.link
    {
        last.text.push_str(&span.text);
    } else {
        row.push(span);
    }
}

pub(in crate::tui) fn blocks(
    source: &str,
) -> impl Iterator<Item = Result<Block, syntect::Error>> + '_ {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut events = Parser::new_ext(source, options);
    std::iter::from_fn(move || {
        let first = events.next()?;
        let mut depth = usize::from(matches!(first, Event::Start(_)));
        let mut block = vec![first];
        while depth > 0 {
            let event = events.next().expect("parser balances block events");
            match event {
                Event::Start(_) => depth += 1,
                Event::End(_) => depth -= 1,
                _ => {},
            }
            block.push(event);
        }
        Some(Writer::default().render(block.into_iter()))
    })
}

#[derive(Default)]
struct Writer {
    lines: Vec<LogicalLine>,
    current: Row,
    styles: Vec<Style>,
    link: Option<Arc<str>>,
    // Prefix authority lives here: list item markers change to hanging indents
    // after their first line. Quote markers remain on continuation lines.
    prefixes: Vec<String>,
    lists: Vec<Option<u64>>,
}

impl Writer {
    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn styled(&mut self, style: Style) {
        self.styles.push(self.style().patch(style));
    }

    fn push(&mut self, text: &str) {
        let text = safe_text(text);
        for (index, part) in text.split('\n').enumerate() {
            if index > 0 {
                self.newline(false);
            }
            if self.current.is_empty() {
                let prefix = self.prefixes.concat();
                append(
                    &mut self.current,
                    Segment {
                        text: prefix,
                        ..Segment::default()
                    },
                );
            }
            let style = self.style();
            append(
                &mut self.current,
                Segment {
                    text: part.into(),
                    style,
                    link: self.link.clone(),
                },
            );
        }
    }

    fn newline(&mut self, blank: bool) {
        if !self.current.is_empty() || blank {
            self.lines.push(LogicalLine {
                spans: std::mem::take(&mut self.current),
                continuation: self.prefixes.concat(),
                clip: false,
            });
        }
    }

    fn render<'a>(
        mut self,
        mut events: impl Iterator<Item = Event<'a>>,
    ) -> Result<Block, syntect::Error> {
        while let Some(event) = events.next() {
            match event {
                Event::Start(Tag::Paragraph) => {},
                Event::End(TagEnd::Paragraph) => {
                    self.newline(false);
                },
                Event::Start(Tag::Heading { .. }) => {
                    self.newline(false);
                    self.styled(
                        Style::new()
                            .fg(Color::LightCyan)
                            .add_modifier(Modifier::BOLD),
                    );
                },
                Event::End(TagEnd::Heading(_)) => {
                    self.newline(false);
                    self.styles.pop();
                },
                Event::Start(Tag::Emphasis) => {
                    self.styled(Style::new().add_modifier(Modifier::ITALIC))
                },
                Event::Start(Tag::Strong) => self.styled(Style::new().add_modifier(Modifier::BOLD)),
                Event::Start(Tag::Strikethrough) => {
                    self.styled(Style::new().add_modifier(Modifier::CROSSED_OUT))
                },
                Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough) => {
                    self.styles.pop();
                },
                Event::Start(Tag::Link { dest_url, .. }) => {
                    self.link = Some(Arc::from(dest_url.as_ref()));
                    self.styled(
                        Style::new()
                            .fg(Color::LightBlue)
                            .add_modifier(Modifier::UNDERLINED),
                    );
                },
                Event::End(TagEnd::Link) => {
                    self.link = None;
                    self.styles.pop();
                },
                Event::Start(Tag::Image { .. }) => {
                    self.styled(Style::new().add_modifier(Modifier::DIM));
                    self.push("[img] ");
                },
                Event::End(TagEnd::Image) => {
                    self.styles.pop();
                },
                Event::Start(Tag::BlockQuote(_)) => {
                    self.newline(false);
                    self.prefixes.push("│ ".into());
                },
                Event::End(TagEnd::BlockQuote(_)) => {
                    self.newline(false);
                    self.prefixes.pop();
                },
                Event::Start(Tag::List(start)) => {
                    self.newline(false);
                    self.lists.push(start);
                },
                Event::End(TagEnd::List(_)) => {
                    self.newline(false);
                    self.lists.pop();
                },
                Event::Start(Tag::Item) => {
                    self.newline(false);
                    let marker = match self.lists.last_mut().expect("item belongs to a list") {
                        Some(index) => {
                            let marker = format!("{index}. ");
                            *index = index.saturating_add(1);
                            marker
                        },
                        None => "• ".into(),
                    };
                    self.push(&marker);
                    self.prefixes.push(" ".repeat(marker.width()));
                },
                Event::End(TagEnd::Item) => {
                    self.newline(false);
                    self.prefixes.pop();
                },
                Event::TaskListMarker(checked) => self.push(if checked { "[x] " } else { "[ ] " }),
                Event::Start(Tag::CodeBlock(kind)) => {
                    self.newline(false);
                    let mut code = String::new();
                    for event in events.by_ref() {
                        match event {
                            Event::End(TagEnd::CodeBlock) => break,
                            Event::Text(text) => code.push_str(&text),
                            _ => {},
                        }
                    }
                    self.code(kind, &code)?;
                },
                Event::Start(Tag::Table(alignments)) => {
                    self.newline(false);
                    self.table(&mut events, &alignments)?;
                },
                Event::Code(text) => {
                    self.styled(Style::new().fg(Color::LightYellow));
                    self.push(&text);
                    self.styles.pop();
                },
                Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => self.push(&text),
                Event::SoftBreak => self.push(" "),
                Event::HardBreak => self.newline(true),
                Event::Rule => {
                    self.newline(false);
                    self.push("────");
                    self.newline(false);
                },
                // Extensions outside the enabled Markdown dialect are never
                // produced here. Keep literal content for parser text events.
                Event::InlineMath(text)
                | Event::DisplayMath(text)
                | Event::FootnoteReference(text) => self.push(&text),
                Event::Start(_) | Event::End(_) => {},
            }
        }
        self.newline(false);
        Ok(Block(self.lines))
    }

    fn code(&mut self, kind: CodeBlockKind<'_>, code: &str) -> Result<(), syntect::Error> {
        let language = match kind {
            CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_owned(),
            CodeBlockKind::Indented => String::new(),
        };
        self.styled(Style::new().fg(Color::DarkGray));
        self.push(&format!("```{language}"));
        self.newline(false);
        self.styles.pop();
        let syntax = SYNTAXES.find_syntax_by_token(&language);
        let mut highlighter =
            syntax.map(|syntax| HighlightLines::new(syntax, &THEMES.themes["base16-ocean.dark"]));
        for line in code.split_inclusive('\n') {
            if let Some(highlighter) = &mut highlighter {
                for (highlight, text) in highlighter.highlight_line(line, &SYNTAXES)? {
                    let color = highlight.foreground;
                    let mut style = Style::new().fg(Color::Rgb(color.r, color.g, color.b));
                    for (flag, modifier) in [
                        (FontStyle::BOLD, Modifier::BOLD),
                        (FontStyle::ITALIC, Modifier::ITALIC),
                        (FontStyle::UNDERLINE, Modifier::UNDERLINED),
                    ] {
                        if highlight.font_style.contains(flag) {
                            style = style.add_modifier(modifier);
                        }
                    }
                    self.styled(style);
                    self.push(text.trim_end_matches('\n'));
                    self.styles.pop();
                }
            } else {
                // No language (or an unknown one) is ordinary code, not an
                // inference about the model's intent or a hidden parse failure.
                self.push(line.trim_end_matches('\n'));
            }
            self.newline(true);
        }
        self.styled(Style::new().fg(Color::DarkGray));
        self.push("```");
        self.newline(false);
        self.styles.pop();
        Ok(())
    }

    fn table<'a>(
        &mut self,
        events: &mut impl Iterator<Item = Event<'a>>,
        alignments: &[Alignment],
    ) -> Result<(), syntect::Error> {
        let mut rows = Vec::<Vec<Row>>::new();
        let mut row = Vec::new();
        while let Some(event) = events.next() {
            match event {
                Event::Start(Tag::TableCell) => {
                    let mut cell = Vec::new();
                    for event in events.by_ref() {
                        if matches!(event, Event::End(TagEnd::TableCell)) {
                            break;
                        }
                        cell.push(event);
                    }
                    let rendered = Writer::default().render(cell.into_iter())?;
                    row.push(rendered.0.into_iter().flat_map(|line| line.spans).collect());
                },
                Event::End(TagEnd::TableHead | TagEnd::TableRow) => {
                    rows.push(std::mem::take(&mut row))
                },
                Event::End(TagEnd::Table) => break,
                _ => {},
            }
        }
        let widths: Vec<_> = (0..alignments.len())
            .map(|column| {
                rows.iter()
                    .map(|row| {
                        row.get(column)
                            .map_or(0, |spans| spans.iter().map(|s| s.text.width()).sum())
                    })
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        for (index, cells) in rows.into_iter().enumerate() {
            let mut spans = vec![Segment {
                text: format!("{}│", self.prefixes.concat()),
                ..Segment::default()
            }];
            for (column, cell) in cells.into_iter().enumerate() {
                let padding =
                    widths[column].saturating_sub(cell.iter().map(|s| s.text.width()).sum());
                let left = match alignments[column] {
                    Alignment::Right => padding,
                    Alignment::Center => padding / 2,
                    _ => 0,
                };
                append(
                    &mut spans,
                    Segment {
                        text: " ".repeat(left + 1),
                        ..Segment::default()
                    },
                );
                for mut span in cell {
                    if index == 0 {
                        span.style = span.style.add_modifier(Modifier::BOLD);
                    }
                    append(&mut spans, span);
                }
                append(
                    &mut spans,
                    Segment {
                        text: format!("{}│", " ".repeat(padding - left + 1)),
                        ..Segment::default()
                    },
                );
            }
            self.lines.push(LogicalLine {
                spans,
                clip: true,
                ..LogicalLine::default()
            });
            if index == 0 {
                let text = format!(
                    "{}├{}┤",
                    self.prefixes.concat(),
                    widths
                        .iter()
                        .map(|w| "─".repeat(w + 2))
                        .collect::<Vec<_>>()
                        .join("┼")
                );
                self.lines.push(LogicalLine {
                    spans: vec![Segment {
                        text,
                        ..Segment::default()
                    }],
                    clip: true,
                    ..LogicalLine::default()
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
