use std::time::{Duration, Instant};

use unicode_segmentation::UnicodeSegmentation;

/// Screen-owned, provisional text received from run observations. `visible`
/// indexes its displayed prefix; only this owner advances or resets it. Neither
/// the text nor this display cursor controls execution or session history.
#[derive(Default)]
pub(super) struct Preview {
    text: String,
    visible: usize,
    advanced: Option<Instant>,
}

impl Preview {
    pub(super) fn push(&mut self, text: &str, now: Instant) {
        self.advanced.get_or_insert(now);
        self.text.push_str(text);
    }

    pub(super) fn visible(&self) -> &str {
        &self.text[..self.visible]
    }

    pub(super) fn advance(&mut self, now: Instant) -> bool {
        let Some(previous) = self.advanced else {
            return false;
        };
        if now.duration_since(previous) < Duration::from_millis(40) {
            return false;
        }
        // Limit both segmentation work and per-frame output. Larger bursts
        // catch up faster; missed frames never accumulate animation credit.
        let pending = &self.text[self.visible..];
        let count = pending.graphemes(true).take(512).count();
        let budget = (4 + count / 8).min(64);
        // Keep the final cluster provisional: a later delta may extend it with
        // combining marks or a ZWJ sequence. Commit/failure consumes all text.
        let end = pending
            .grapheme_indices(true)
            .nth(budget.min(count.saturating_sub(1)))
            .map_or(0, |(index, _)| index);
        self.visible += end;
        self.advanced = Some(now);
        end > 0
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(super) fn take(&mut self) -> String {
        std::mem::take(self).text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_is_paced_and_failure_can_take_every_received_character() {
        let now = Instant::now();
        let text = "中文👩‍💻e\u{301}".repeat(100);
        let mut preview = Preview::default();
        preview.push(&text, now);
        assert!(!preview.advance(now + Duration::from_millis(39)));
        assert!(preview.advance(now + Duration::from_millis(40)));
        let first = preview.visible().to_owned();
        assert!(!first.is_empty());
        assert!(first.len() < text.len());
        assert!(text.grapheme_indices(true).any(|(i, _)| i == first.len()));
        assert!(!preview.advance(now + Duration::from_millis(40)));
        assert!(preview.advance(now + Duration::from_millis(80)));
        assert!(preview.visible().len() > first.len());
        assert_eq!(preview.take(), text);
        assert!(preview.visible().is_empty());
        assert!(!preview.advance(now + Duration::from_secs(1)));
    }

    #[test]
    fn split_clusters_remain_whole_and_clear_discards_pending_animation() {
        let now = Instant::now();
        let mut preview = Preview::default();
        preview.push("Ae", now);
        preview.advance(now + Duration::from_millis(40));
        assert_eq!(preview.visible(), "A");
        preview.push("\u{301}👩", now);
        preview.advance(now + Duration::from_millis(80));
        assert_eq!(preview.visible(), "Ae\u{301}");
        preview.push("\u{200d}💻!", now);
        preview.advance(now + Duration::from_millis(120));
        assert_eq!(preview.visible(), "Ae\u{301}👩‍💻");
        preview.clear();
        preview.push("next", now + Duration::from_secs(1));
        assert!(!preview.advance(now + Duration::from_secs(1)));
        assert!(preview.visible().is_empty());
    }
}
