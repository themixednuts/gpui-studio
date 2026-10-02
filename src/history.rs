//! Snapshot undo/redo.
//!
//! Documents are small, so each undo step stores a full copy. Continuous
//! gestures (dragging, typing in a field) share a coalescing key so one
//! gesture becomes one undo step.

use std::time::{Duration, Instant};

use crate::model::Document;

const LIMIT: usize = 200;
const COALESCE_WINDOW: Duration = Duration::from_millis(900);

/// Undo/redo stacks of document snapshots.
#[derive(Default)]
pub struct History {
    past: Vec<Document>,
    future: Vec<Document>,
    last_key: Option<(String, Instant)>,
}

impl History {
    /// Record the state *before* an edit. Edits with the same `key` inside the
    /// coalescing window extend the previous step instead of adding one.
    pub fn record(&mut self, before: &Document, key: Option<&str>) {
        let now = Instant::now();
        if let (Some(key), Some((last, at))) = (key, &self.last_key)
            && key == last
            && now.duration_since(*at) < COALESCE_WINDOW
        {
            self.last_key = Some((key.to_owned(), now));
            self.future.clear();
            return;
        }
        self.past.push(before.clone());
        if self.past.len() > LIMIT {
            self.past.remove(0);
        }
        self.future.clear();
        self.last_key = key.map(|key| (key.to_owned(), now));
    }

    /// End any coalescing so the next edit starts a new step.
    pub fn seal(&mut self) {
        self.last_key = None;
    }

    /// Restore the previous state, given the current one.
    pub fn undo(&mut self, current: &Document) -> Option<Document> {
        let previous = self.past.pop()?;
        self.future.push(current.clone());
        self.last_key = None;
        Some(previous)
    }

    /// Re-apply an undone state, given the current one.
    pub fn redo(&mut self, current: &Document) -> Option<Document> {
        let next = self.future.pop()?;
        self.past.push(current.clone());
        self.last_key = None;
        Some(next)
    }

    /// Whether undo is available.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    /// Whether redo is available.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_gestures_and_round_trips() {
        let mut history = History::default();
        let mut doc = Document::new();
        let v0 = doc.clone();
        history.record(&doc, Some("drag"));
        doc.create_artboard(0, "A", (0.0, 0.0), (10.0, 10.0));
        history.record(&doc, Some("drag"));
        doc.pages[0].artboards[0].x = 5.0;
        let v2 = doc.clone();
        let undone = history.undo(&doc).unwrap();
        assert_eq!(undone, v0, "one gesture is one step");
        assert_eq!(history.redo(&undone).unwrap(), v2);
        assert!(!history.can_redo());
    }
}
