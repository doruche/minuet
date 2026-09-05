//! OSC 8 is emitted only here, never embedded in Ratatui cell symbols. The
//! terminal owns published rows; no application hyperlink registry survives a
//! write. Modern terminals handle activation, with no capability probe.

use std::io::{self, Write};

use crossterm::{
    queue,
    style::{Attribute, Print, ResetColor, SetAttribute, SetStyle},
};
use ratatui::backend::IntoCrossterm;

use crate::tui::render::markdown::Row;

pub(super) const CLOSE_LINK: &str = "\x1b]8;;\x1b\\";
// End any partially written OSC string before sending the closing hyperlink.
pub(super) const RESET_LINK: &str = "\x1b\\\x1b]8;;\x1b\\";

pub(super) fn write_row(output: &mut impl Write, row: &Row, colors: bool) -> io::Result<()> {
    let result = (|| {
        for segment in row {
            queue!(output, SetAttribute(Attribute::Reset), ResetColor)?;
            if colors {
                queue!(output, SetStyle(segment.style.into_crossterm()))?;
            }
            if let Some(link) = &segment.link {
                // Destinations are data. Percent-encode control bytes (including
                // decoded entities) so a URL cannot terminate/inject OSC commands.
                write!(output, "\x1b]8;;")?;
                for c in link.chars() {
                    if c.is_control() {
                        for byte in c.to_string().bytes() {
                            write!(output, "%{byte:02X}")?;
                        }
                    } else {
                        write!(output, "{c}")?;
                    }
                }
                write!(output, "\x1b\\")?;
            }
            queue!(output, Print(&segment.text))?;
            if segment.link.is_some() {
                write!(output, "{CLOSE_LINK}")?;
            }
        }
        Ok(())
    })();
    // Close even on a failed write, before the caller reports failure. Teardown
    // retries this reset as well; link state must never leak into the prompt.
    let reset = write!(output, "{RESET_LINK}")
        .and_then(|()| queue!(output, SetAttribute(Attribute::Reset), ResetColor));
    result.and(reset)?;
    write!(output, "\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::render::markdown::blocks;

    #[test]
    fn links_are_protocol_metadata_and_no_color_preserves_activation() {
        let row = blocks("[**label**](https://example.test/a?x=1&y=2) tail")
            .next()
            .unwrap()
            .unwrap()
            .rows(80)
            .remove(0);
        for colors in [false, true] {
            let mut bytes = Vec::new();
            write_row(&mut bytes, &row, colors).unwrap();
            let output = String::from_utf8(bytes.clone()).unwrap();
            assert!(
                output.contains("\x1b]8;;https://example.test/a?x=1&y=2\x1b\\label\x1b]8;;\x1b\\")
            );
            let mut parser = vt100::Parser::new(4, 80, 0);
            parser.process(&bytes);
            assert_eq!(parser.screen().contents(), "label tail");
            assert_eq!(parser.screen().cell(0, 0).unwrap().bold(), colors);
            assert!(!parser.screen().cell(0, 6).unwrap().bold());
        }
    }

    #[test]
    fn destination_control_characters_cannot_terminate_osc() {
        let row = blocks("[label](https://example.test/&#27;X&#7;)")
            .next()
            .unwrap()
            .unwrap()
            .rows(80)
            .remove(0);
        let mut output = Vec::new();
        write_row(&mut output, &row, true).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("https://example.test/%1BX%07\x1b\\label"));
    }

    struct FailOnce {
        remaining: usize,
        failed: bool,
        bytes: Vec<u8>,
    }
    impl Write for FailOnce {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if !self.failed && self.remaining == 0 {
                self.failed = true;
                return Err(io::Error::other("injected output failure"));
            }
            let n = if self.failed {
                bytes.len()
            } else {
                bytes.len().min(self.remaining)
            };
            self.remaining = self.remaining.saturating_sub(n);
            self.bytes.extend_from_slice(&bytes[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn partial_link_write_returns_error_and_attempts_to_close_the_link() {
        let row = blocks("[label](https://example.test)")
            .next()
            .unwrap()
            .unwrap()
            .rows(80)
            .remove(0);
        let mut complete = Vec::new();
        write_row(&mut complete, &row, true).unwrap();
        // Exercise each failure position up through the label, including a
        // partially emitted OSC opener or destination. Never mask the error.
        let label_end = complete.windows(5).position(|b| b == b"label").unwrap() + 5;
        for position in 0..label_end {
            let mut writer = FailOnce {
                remaining: position,
                failed: false,
                bytes: Vec::new(),
            };
            assert!(write_row(&mut writer, &row, true).is_err());
            assert!(
                writer.bytes[position..]
                    .windows(CLOSE_LINK.len())
                    .any(|b| b == CLOSE_LINK.as_bytes())
            );
        }
    }
}
