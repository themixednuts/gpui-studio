use gpui_mcp::LiveDocumentSource;

const MAX_SNAPSHOTS: usize = 100;

/// Producer responsible for one observed document revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangeOrigin {
    /// A visual or source edit initiated inside Studio.
    Manual,
    /// A complete bundle loaded from the watched project directory.
    File,
    /// An in-memory preview submitted through MCP.
    Mcp,
    /// A history move to an older snapshot.
    Undo,
    /// A history move to a newer snapshot.
    Redo,
}

/// Bounded source-snapshot history synchronized to runtime revisions.
#[derive(Clone, Debug)]
pub struct RevisionHistory {
    snapshots: Vec<LiveDocumentSource>,
    cursor: usize,
    observed_revision: u64,
    last_origin: ChangeOrigin,
}

impl RevisionHistory {
    /// Start a history at one active runtime revision.
    #[must_use]
    pub fn new(revision: u64, source: LiveDocumentSource) -> Self {
        Self {
            snapshots: vec![source],
            cursor: 0,
            observed_revision: revision,
            last_origin: ChangeOrigin::File,
        }
    }

    /// Record a source replacement that has already been accepted by the runtime.
    ///
    /// Returns `true` when the source created a new history snapshot.
    pub fn observe(
        &mut self,
        revision: u64,
        source: LiveDocumentSource,
        origin: ChangeOrigin,
    ) -> bool {
        if revision == self.observed_revision {
            return false;
        }
        self.observed_revision = revision;
        self.last_origin = origin;
        if self.snapshots[self.cursor] == source {
            return false;
        }
        self.snapshots.truncate(self.cursor + 1);
        self.snapshots.push(source);
        self.cursor += 1;
        if self.snapshots.len() > MAX_SNAPSHOTS {
            self.snapshots.remove(0);
            self.cursor -= 1;
        }
        true
    }

    /// Candidate complete bundle for an undo operation.
    #[must_use]
    pub fn undo_source(&self) -> Option<&LiveDocumentSource> {
        self.cursor
            .checked_sub(1)
            .and_then(|index| self.snapshots.get(index))
    }

    /// Candidate complete bundle for a redo operation.
    #[must_use]
    pub fn redo_source(&self) -> Option<&LiveDocumentSource> {
        self.snapshots.get(self.cursor + 1)
    }

    /// Commit a runtime revision that successfully applied the undo candidate.
    pub fn commit_undo(&mut self, revision: u64) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.observed_revision = revision;
            self.last_origin = ChangeOrigin::Undo;
        }
    }

    /// Commit a runtime revision that successfully applied the redo candidate.
    pub fn commit_redo(&mut self, revision: u64) {
        if self.cursor + 1 < self.snapshots.len() {
            self.cursor += 1;
            self.observed_revision = revision;
            self.last_origin = ChangeOrigin::Redo;
        }
    }

    /// Most recently observed runtime revision.
    #[must_use]
    pub const fn observed_revision(&self) -> u64 {
        self.observed_revision
    }

    /// Producer responsible for the current history position.
    #[must_use]
    pub const fn last_origin(&self) -> ChangeOrigin {
        self.last_origin
    }

    /// Whether an older snapshot is available.
    #[must_use]
    pub const fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    /// Whether a newer snapshot is available.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.cursor + 1 < self.snapshots.len()
    }
}

#[cfg(test)]
mod tests {
    use gpui_mcp::LiveDocumentSource;

    use super::{ChangeOrigin, RevisionHistory};

    fn source(html: &str) -> LiveDocumentSource {
        LiveDocumentSource {
            html: html.to_owned(),
            css: String::new(),
            bindings_ron: "(version:1,bindings:[])".to_owned(),
        }
    }

    #[test]
    fn undo_and_redo_move_only_after_runtime_acceptance() {
        let mut history = RevisionHistory::new(1, source("<p>one</p>"));
        assert!(history.observe(2, source("<p>two</p>"), ChangeOrigin::Mcp));
        assert_eq!(history.undo_source(), Some(&source("<p>one</p>")));

        history.commit_undo(3);
        assert_eq!(history.redo_source(), Some(&source("<p>two</p>")));
        assert_eq!(history.last_origin(), ChangeOrigin::Undo);

        history.commit_redo(4);
        assert!(history.can_undo());
        assert!(!history.can_redo());
        assert_eq!(history.observed_revision(), 4);
    }

    #[test]
    fn a_new_edit_after_undo_discards_redo_history() {
        let mut history = RevisionHistory::new(1, source("one"));
        history.observe(2, source("two"), ChangeOrigin::Mcp);
        history.commit_undo(3);

        assert!(history.observe(4, source("branch"), ChangeOrigin::Manual));
        assert!(!history.can_redo());
    }
}
