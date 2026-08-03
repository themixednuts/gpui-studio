use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use gpui_mcp::{Rect, UiTree};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

const ANNOTATION_DOCUMENT_VERSION: u16 = 2;
const LEGACY_ANNOTATION_DOCUMENT_VERSION: u16 = 1;
const MAX_ANNOTATION_DOCUMENT_BYTES: u64 = 1024 * 1024;
const MAX_ANNOTATIONS: usize = 2_048;
const MAX_COMMENT_BYTES: usize = 16 * 1024;

/// Stable selection context shared by the inspector, annotations, and MCP resources.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElementSelection {
    /// Source-level element ID, independent of runtime embedding.
    pub authored_id: String,
    /// Fully namespaced ID in the GPUI semantic tree.
    pub runtime_id: String,
    /// Source revision when the selection was made.
    pub document_revision: u64,
    /// Last observed window-relative bounds.
    pub captured_rect: Option<Rect>,
}

impl ElementSelection {
    /// Create a selection from one authored and runtime identifier.
    #[must_use]
    pub fn new(
        authored_id: impl Into<String>,
        runtime_id: impl Into<String>,
        document_revision: u64,
        captured_rect: Option<Rect>,
    ) -> Self {
        Self {
            authored_id: authored_id.into(),
            runtime_id: runtime_id.into(),
            document_revision,
            captured_rect,
        }
    }
}

/// Normalized annotation anchor within an element's current rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedAnchor {
    /// Horizontal position from zero through one.
    pub x: f32,
    /// Vertical position from zero through one.
    pub y: f32,
}

impl Default for NormalizedAnchor {
    fn default() -> Self {
        Self { x: 0.5, y: 0.5 }
    }
}

/// Review state for one spatial comment.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum AnnotationStatus {
    /// Task is queued and should inform the next change.
    #[default]
    Open,
    /// Work has started but is not yet complete.
    InProgress,
    /// Work is complete and excluded from the active MCP queue.
    #[serde(alias = "Resolved")]
    Done,
    /// Historical task is intentionally hidden from ordinary history views.
    Archived,
}

impl AnnotationStatus {
    /// Whether this task should appear in the active review queue.
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Open | Self::InProgress)
    }
}

/// One project-local comment attached to stable authored and runtime identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Annotation {
    /// Stable project-local comment ID.
    pub id: String,
    /// Selected element when the comment was created.
    pub target: ElementSelection,
    /// Normalized location that survives element movement and resizing.
    pub anchor: NormalizedAnchor,
    /// Human review comment.
    pub comment: String,
    /// Current review state.
    pub status: AnnotationStatus,
}

/// Versioned offline annotation document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnotationStore {
    /// Versioned schema.
    pub version: u16,
    /// Next monotonic project-local annotation number.
    pub next_id: u64,
    /// Ordered spatial comments.
    pub annotations: Vec<Annotation>,
}

impl Default for AnnotationStore {
    fn default() -> Self {
        Self {
            version: ANNOTATION_DOCUMENT_VERSION,
            next_id: 1,
            annotations: Vec::new(),
        }
    }
}

impl AnnotationStore {
    /// Load project-local annotations, or an empty document when absent.
    ///
    /// # Errors
    ///
    /// Returns an error for oversized, invalid, or unsupported annotation data.
    pub fn load(project_root: &Path) -> Result<Self, AnnotationError> {
        let path = annotation_path(project_root);
        if !path.exists() {
            return Ok(Self::default());
        }
        let metadata = fs::metadata(&path).map_err(|source| AnnotationError::Io {
            operation: "inspect annotation document",
            path: path.clone(),
            source,
        })?;
        if metadata.len() > MAX_ANNOTATION_DOCUMENT_BYTES {
            return Err(AnnotationError::TooLarge {
                found: metadata.len(),
                maximum: MAX_ANNOTATION_DOCUMENT_BYTES,
            });
        }
        let source = fs::read_to_string(&path).map_err(|source| AnnotationError::Io {
            operation: "read annotation document",
            path,
            source,
        })?;
        let mut store: Self = ron::from_str(&source)?;
        store.migrate()?;
        store.validate()?;
        Ok(store)
    }

    /// Attach a new open comment to the exact selected element.
    ///
    /// # Errors
    ///
    /// Rejects empty, oversized, invalid, or over-capacity comments.
    pub fn add(
        &mut self,
        selection: ElementSelection,
        comment: &str,
        anchor: NormalizedAnchor,
    ) -> Result<&Annotation, AnnotationError> {
        if self.annotations.len() >= MAX_ANNOTATIONS {
            return Err(AnnotationError::Invalid(
                "annotation document reached its capacity",
            ));
        }
        validate_selection(&selection)?;
        validate_anchor(anchor)?;
        let comment = comment.trim();
        if comment.is_empty() || comment.len() > MAX_COMMENT_BYTES {
            return Err(AnnotationError::Invalid(
                "annotation comment is empty or exceeds 16 KiB",
            ));
        }
        let id = format!("comment-{:06}", self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        self.annotations.push(Annotation {
            id,
            target: selection,
            anchor,
            comment: comment.to_owned(),
            status: AnnotationStatus::Open,
        });
        Ok(&self.annotations[self.annotations.len() - 1])
    }

    /// Move an existing review task to another lifecycle state.
    ///
    /// # Errors
    ///
    /// Returns an error when the task is missing or the transition is invalid.
    pub fn transition(
        &mut self,
        id: &str,
        next: AnnotationStatus,
    ) -> Result<&Annotation, AnnotationError> {
        let annotation = self
            .annotations
            .iter_mut()
            .find(|annotation| annotation.id == id)
            .ok_or(AnnotationError::NotFound)?;
        let valid = matches!(
            (annotation.status, next),
            (
                AnnotationStatus::Open,
                AnnotationStatus::InProgress | AnnotationStatus::Done
            ) | (
                AnnotationStatus::InProgress,
                AnnotationStatus::Open | AnnotationStatus::Done
            ) | (
                AnnotationStatus::Done,
                AnnotationStatus::Open | AnnotationStatus::Archived
            ) | (AnnotationStatus::Archived, AnnotationStatus::Open)
        ) || annotation.status == next;
        if !valid {
            return Err(AnnotationError::Invalid(
                "annotation task transition is invalid",
            ));
        }
        annotation.status = next;
        Ok(annotation)
    }

    /// Remove one spatial comment entirely (user-initiated delete of their own
    /// annotation), as opposed to the status transitions used for review
    /// accountability.
    ///
    /// # Errors
    ///
    /// Returns an error when no annotation has the given id.
    pub fn remove(&mut self, id: &str) -> Result<(), AnnotationError> {
        let before = self.annotations.len();
        self.annotations.retain(|annotation| annotation.id != id);
        if self.annotations.len() == before {
            return Err(AnnotationError::NotFound);
        }
        Ok(())
    }

    /// Replace the text of an existing spatial comment without changing its status.
    ///
    /// # Errors
    ///
    /// Returns an error when the comment is missing, empty, or oversized.
    pub fn update_comment(
        &mut self,
        id: &str,
        comment: &str,
    ) -> Result<&Annotation, AnnotationError> {
        let comment = comment.trim();
        if comment.is_empty() || comment.len() > MAX_COMMENT_BYTES {
            return Err(AnnotationError::Invalid(
                "annotation comment is empty or exceeds 16 KiB",
            ));
        }
        let annotation = self
            .annotations
            .iter_mut()
            .find(|annotation| annotation.id == id)
            .ok_or(AnnotationError::NotFound)?;
        comment.clone_into(&mut annotation.comment);
        Ok(annotation)
    }

    /// Active tasks in stable document order.
    #[must_use]
    pub fn active(&self) -> impl DoubleEndedIterator<Item = &Annotation> {
        self.annotations
            .iter()
            .filter(|annotation| annotation.status.is_active())
    }

    /// An active, persisted comment eligible for an MCP handoff.
    #[must_use]
    pub fn sendable(&self) -> impl DoubleEndedIterator<Item = &Annotation> {
        self.active()
    }

    /// Whether the annotation document contains anything an MCP handoff can publish.
    #[must_use]
    pub fn has_sendable(&self) -> bool {
        self.sendable().next().is_some()
    }

    /// Most recently created active task for one runtime element.
    #[must_use]
    pub fn latest_active_for_target(&self, runtime_id: &str) -> Option<&Annotation> {
        self.active()
            .rev()
            .find(|annotation| annotation.target.runtime_id == runtime_id)
    }

    /// Count active, completed, and archived tasks.
    #[must_use]
    pub fn counts(&self) -> AnnotationCounts {
        self.annotations
            .iter()
            .fold(AnnotationCounts::default(), |mut counts, annotation| {
                match annotation.status {
                    AnnotationStatus::Open | AnnotationStatus::InProgress => counts.active += 1,
                    AnnotationStatus::Done => counts.done += 1,
                    AnnotationStatus::Archived => counts.archived += 1,
                }
                counts
            })
    }

    /// Persist the complete annotation document through a flushed staged file.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid data, serialization, staging, or replacement failure.
    pub fn save(&self, project_root: &Path) -> Result<(), AnnotationError> {
        self.validate()?;
        let directory = project_root.join(".gpui-studio");
        fs::create_dir_all(&directory).map_err(|source| AnnotationError::Io {
            operation: "create annotation directory",
            path: directory.clone(),
            source,
        })?;
        let destination = annotation_path(project_root);
        let source = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())?;
        let mut temporary =
            NamedTempFile::new_in(&directory).map_err(|source| AnnotationError::Io {
                operation: "stage annotation document",
                path: directory,
                source,
            })?;
        temporary
            .write_all(source.as_bytes())
            .and_then(|()| temporary.flush())
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|source| AnnotationError::Io {
                operation: "flush annotation document",
                path: temporary.path().to_owned(),
                source,
            })?;
        temporary
            .persist(&destination)
            .map_err(|source| AnnotationError::Persist {
                path: destination,
                source,
            })?;
        Ok(())
    }

    /// Resolve annotations against the latest semantic tree for MCP context.
    #[must_use]
    pub fn resolved(&self, tree: &UiTree) -> Vec<ResolvedAnnotation> {
        Self::resolve_annotations(self.annotations.iter(), tree)
    }

    /// Resolve only active work against the latest semantic tree for MCP context.
    #[must_use]
    pub fn resolved_active(&self, tree: &UiTree) -> Vec<ResolvedAnnotation> {
        Self::resolve_annotations(self.active(), tree)
    }

    /// Resolve one ordered annotation-ID batch against the latest semantic tree.
    #[must_use]
    pub fn resolved_ids(&self, ids: &[String], tree: &UiTree) -> Vec<ResolvedAnnotation> {
        Self::resolve_annotations(
            ids.iter().filter_map(|id| {
                self.annotations
                    .iter()
                    .find(|annotation| annotation.id == *id)
            }),
            tree,
        )
    }

    fn resolve_annotations<'a>(
        annotations: impl Iterator<Item = &'a Annotation>,
        tree: &UiTree,
    ) -> Vec<ResolvedAnnotation> {
        annotations
            .cloned()
            .map(|annotation| {
                let current_rect = tree
                    .nodes
                    .get(&annotation.target.runtime_id)
                    .and_then(|node| node.bounds);
                let anchor_point = current_rect.map(|rect| ResolvedPoint {
                    x: rect.x + rect.width * annotation.anchor.x,
                    y: rect.y + rect.height * annotation.anchor.y,
                });
                ResolvedAnnotation {
                    annotation,
                    current_rect,
                    anchor_point,
                    semantic_generation: tree.generation,
                }
            })
            .collect()
    }

    fn migrate(&mut self) -> Result<(), AnnotationError> {
        if self.version == ANNOTATION_DOCUMENT_VERSION {
            return Ok(());
        }
        if self.version != LEGACY_ANNOTATION_DOCUMENT_VERSION {
            return Err(AnnotationError::UnsupportedVersion {
                found: self.version,
                supported: ANNOTATION_DOCUMENT_VERSION,
            });
        }
        self.version = ANNOTATION_DOCUMENT_VERSION;
        Ok(())
    }

    fn validate(&self) -> Result<(), AnnotationError> {
        if self.version != ANNOTATION_DOCUMENT_VERSION {
            return Err(AnnotationError::UnsupportedVersion {
                found: self.version,
                supported: ANNOTATION_DOCUMENT_VERSION,
            });
        }
        if self.annotations.len() > MAX_ANNOTATIONS || self.next_id == 0 {
            return Err(AnnotationError::Invalid(
                "annotation count or sequence is invalid",
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        for annotation in &self.annotations {
            if annotation.id.is_empty()
                || annotation.id.len() > 128
                || !ids.insert(annotation.id.as_str())
                || annotation.comment.is_empty()
                || annotation.comment.len() > MAX_COMMENT_BYTES
            {
                return Err(AnnotationError::Invalid(
                    "annotation identity or comment is invalid",
                ));
            }
            validate_selection(&annotation.target)?;
            validate_anchor(annotation.anchor)?;
        }
        Ok(())
    }
}

/// Review-task counts split by lifecycle bucket.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct AnnotationCounts {
    /// Open or in-progress tasks.
    pub active: usize,
    /// Completed tasks retained as history.
    pub done: usize,
    /// Archived historical tasks.
    pub archived: usize,
}

/// One annotation enriched with current geometry at resource-read time.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ResolvedAnnotation {
    /// Persistent comment and capture context.
    pub annotation: Annotation,
    /// Latest bounds for the stable runtime ID, if it still exists.
    pub current_rect: Option<Rect>,
    /// Latest window-relative anchor point.
    pub anchor_point: Option<ResolvedPoint>,
    /// Semantic tree generation used for resolution.
    pub semantic_generation: u64,
}

/// Window-relative point serialized into Studio MCP resources.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ResolvedPoint {
    /// Horizontal logical coordinate.
    pub x: f32,
    /// Vertical logical coordinate.
    pub y: f32,
}

fn validate_selection(selection: &ElementSelection) -> Result<(), AnnotationError> {
    if selection.authored_id.is_empty()
        || selection.authored_id.len() > 256
        || selection.runtime_id.is_empty()
        || selection.runtime_id.len() > 256
        || selection.document_revision == 0
        || selection.captured_rect.is_some_and(|rect| !rect.is_valid())
    {
        return Err(AnnotationError::Invalid("annotation selection is invalid"));
    }
    Ok(())
}

fn validate_anchor(anchor: NormalizedAnchor) -> Result<(), AnnotationError> {
    if !anchor.x.is_finite()
        || !anchor.y.is_finite()
        || !(0.0..=1.0).contains(&anchor.x)
        || !(0.0..=1.0).contains(&anchor.y)
    {
        return Err(AnnotationError::Invalid("annotation anchor is invalid"));
    }
    Ok(())
}

fn annotation_path(project_root: &Path) -> PathBuf {
    project_root.join(".gpui-studio").join("annotations.ron")
}

/// Invalid or unpersistable annotation data.
#[derive(Debug, thiserror::Error)]
pub enum AnnotationError {
    /// Requested task ID does not exist.
    #[error("annotation task was not found")]
    NotFound,
    /// The document exceeds the bounded local input size.
    #[error("annotation document is {found} bytes; maximum is {maximum}")]
    TooLarge {
        /// Observed bytes.
        found: u64,
        /// Maximum bytes.
        maximum: u64,
    },
    /// RON parsing failed.
    #[error("parse annotation document")]
    Parse(#[from] ron::error::SpannedError),
    /// RON serialization failed.
    #[error("serialize annotation document")]
    Serialize(#[from] ron::Error),
    /// The schema version is unsupported.
    #[error("annotation version {found} is unsupported; expected {supported}")]
    UnsupportedVersion {
        /// Parsed version.
        found: u16,
        /// Current version.
        supported: u16,
    },
    /// Annotation data violates a bounded semantic invariant.
    #[error("invalid annotations: {0}")]
    Invalid(&'static str),
    /// A filesystem operation failed.
    #[error("{operation} `{}`", path.display())]
    Io {
        /// Operation being attempted.
        operation: &'static str,
        /// Affected path.
        path: PathBuf,
        /// Operating-system error.
        #[source]
        source: std::io::Error,
    },
    /// Atomic replacement failed.
    #[error("replace annotation document `{}`", path.display())]
    Persist {
        /// Destination that could not be replaced.
        path: PathBuf,
        /// Staged-file persistence error.
        #[source]
        source: tempfile::PersistError,
    },
}

#[cfg(test)]
mod tests {
    use gpui_mcp::{Rect, UiTree};
    use tempfile::TempDir;

    use super::{AnnotationStatus, AnnotationStore, ElementSelection, NormalizedAnchor};

    #[test]
    fn comments_round_trip_with_stable_spatial_context() -> Result<(), Box<dyn std::error::Error>> {
        let root = TempDir::new()?;
        let mut store = AnnotationStore::default();
        let selection = ElementSelection::new(
            "hero-title",
            "project-canvas/hero-title",
            7,
            Some(Rect {
                x: 10.0,
                y: 20.0,
                width: 200.0,
                height: 40.0,
            }),
        );
        store.add(
            selection,
            "Make this hierarchy clearer",
            NormalizedAnchor::default(),
        )?;

        store.save(root.path())?;

        assert_eq!(AnnotationStore::load(root.path())?, store);
        assert_eq!(store.resolved(&UiTree::default()).len(), 1);
        Ok(())
    }

    #[test]
    fn completed_tasks_leave_active_context_but_remain_in_history()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut store = AnnotationStore::default();
        assert!(!store.has_sendable());
        let task_id = store
            .add(
                ElementSelection::new("hero", "canvas--hero", 1, None),
                "Increase contrast",
                NormalizedAnchor::default(),
            )?
            .id
            .clone();
        assert!(store.has_sendable());

        store.transition(&task_id, AnnotationStatus::InProgress)?;
        store.transition(&task_id, AnnotationStatus::Done)?;

        assert!(!store.has_sendable());
        assert!(store.active().next().is_none());
        assert!(store.resolved_active(&UiTree::default()).is_empty());
        assert_eq!(store.resolved(&UiTree::default()).len(), 1);
        assert_eq!(store.counts().done, 1);
        Ok(())
    }

    #[test]
    fn v1_resolved_comments_migrate_to_completed_tasks() -> Result<(), Box<dyn std::error::Error>> {
        let root = TempDir::new()?;
        let directory = root.path().join(".gpui-studio");
        std::fs::create_dir_all(&directory)?;
        std::fs::write(
            directory.join("annotations.ron"),
            r#"(
                version: 1,
                next_id: 2,
                annotations: [(
                    id: "comment-000001",
                    target: (
                        authored_id: "hero",
                        runtime_id: "canvas--hero",
                        document_revision: 1,
                        captured_rect: None,
                    ),
                    anchor: (x: 0.5, y: 0.5),
                    comment: "Legacy complete task",
                    status: Resolved,
                )],
            )"#,
        )?;

        let store = AnnotationStore::load(root.path())?;

        assert_eq!(store.version, 2);
        assert_eq!(store.annotations[0].status, AnnotationStatus::Done);
        assert!(store.active().next().is_none());
        Ok(())
    }
}
