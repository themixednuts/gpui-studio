use std::collections::BTreeSet;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::AnnotationStore;

const HANDOFF_DOCUMENT_VERSION: u16 = 1;
const MAX_HANDOFF_DOCUMENT_BYTES: u64 = 256 * 1024;
const MAX_HANDOFF_ANNOTATIONS: usize = 2_048;

/// One explicit user dispatch of unresolved comments to an MCP agent handoff.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnotationHandoff {
    /// Stable project-local batch identifier.
    pub id: String,
    /// Active annotation IDs included in stable document order.
    pub annotation_ids: Vec<String>,
    /// HTML document revision visible when the batch was published.
    pub project_revision: u64,
    /// Semantic tree generation used to capture the batch.
    pub semantic_generation: u64,
}

/// Persisted latest-value outbox read by the Studio MCP context resource.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnotationHandoffStore {
    /// Versioned schema.
    pub version: u16,
    /// Next monotonic project-local batch number.
    pub next_id: u64,
    /// Most recently sent annotation batch.
    pub latest: Option<AnnotationHandoff>,
}

impl Default for AnnotationHandoffStore {
    fn default() -> Self {
        Self {
            version: HANDOFF_DOCUMENT_VERSION,
            next_id: 1,
            latest: None,
        }
    }
}

impl AnnotationHandoffStore {
    /// Load the project outbox, or return an empty outbox when it has not been used.
    ///
    /// # Errors
    ///
    /// Returns an error for oversized, invalid, unreadable, or unsupported data.
    pub fn load(project_root: &Path) -> Result<Self, AnnotationHandoffError> {
        let path = handoff_path(project_root);
        if !path.exists() {
            return Ok(Self::default());
        }
        let metadata = fs::metadata(&path).map_err(|source| AnnotationHandoffError::Io {
            operation: "inspect annotation handoff",
            path: path.clone(),
            source,
        })?;
        if metadata.len() > MAX_HANDOFF_DOCUMENT_BYTES {
            return Err(AnnotationHandoffError::TooLarge {
                found: metadata.len(),
                maximum: MAX_HANDOFF_DOCUMENT_BYTES,
            });
        }
        let source = fs::read_to_string(&path).map_err(|source| AnnotationHandoffError::Io {
            operation: "read annotation handoff",
            path,
            source,
        })?;
        let store: Self = ron::from_str(&source)?;
        store.validate()?;
        Ok(store)
    }

    /// Publish every currently active annotation as one latest-value handoff.
    ///
    /// # Errors
    ///
    /// Returns an error when no unresolved comments exist or batch metadata is invalid.
    pub fn publish(
        &mut self,
        annotations: &AnnotationStore,
        project_revision: u64,
        semantic_generation: u64,
    ) -> Result<&AnnotationHandoff, AnnotationHandoffError> {
        let annotation_ids = annotations
            .sendable()
            .map(|annotation| annotation.id.clone())
            .collect::<Vec<_>>();
        if annotation_ids.is_empty() {
            return Err(AnnotationHandoffError::Empty);
        }
        let handoff = AnnotationHandoff {
            id: format!("handoff-{:06}", self.next_id),
            annotation_ids,
            project_revision,
            semantic_generation,
        };
        validate_handoff(&handoff)?;
        self.next_id = self.next_id.saturating_add(1);
        self.latest = Some(handoff);
        let Some(latest) = self.latest.as_ref() else {
            return Err(AnnotationHandoffError::Invalid(
                "latest annotation handoff was not retained",
            ));
        };
        Ok(latest)
    }

    /// Persist the outbox through a flushed staged file.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid data, serialization, staging, or replacement failure.
    pub fn save(&self, project_root: &Path) -> Result<(), AnnotationHandoffError> {
        self.validate()?;
        let directory = project_root.join(".gpui-studio");
        fs::create_dir_all(&directory).map_err(|source| AnnotationHandoffError::Io {
            operation: "create annotation handoff directory",
            path: directory.clone(),
            source,
        })?;
        let destination = handoff_path(project_root);
        let source = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())?;
        let mut temporary =
            NamedTempFile::new_in(&directory).map_err(|source| AnnotationHandoffError::Io {
                operation: "stage annotation handoff",
                path: directory,
                source,
            })?;
        temporary
            .write_all(source.as_bytes())
            .and_then(|()| temporary.flush())
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|source| AnnotationHandoffError::Io {
                operation: "flush annotation handoff",
                path: temporary.path().to_owned(),
                source,
            })?;
        temporary
            .persist(&destination)
            .map_err(|source| AnnotationHandoffError::Persist {
                path: destination,
                source,
            })?;
        Ok(())
    }

    fn validate(&self) -> Result<(), AnnotationHandoffError> {
        if self.version != HANDOFF_DOCUMENT_VERSION {
            return Err(AnnotationHandoffError::UnsupportedVersion {
                found: self.version,
                supported: HANDOFF_DOCUMENT_VERSION,
            });
        }
        if self.next_id == 0 {
            return Err(AnnotationHandoffError::Invalid(
                "annotation handoff sequence is invalid",
            ));
        }
        if let Some(handoff) = &self.latest {
            validate_handoff(handoff)?;
        }
        Ok(())
    }
}

fn validate_handoff(handoff: &AnnotationHandoff) -> Result<(), AnnotationHandoffError> {
    let mut ids = BTreeSet::new();
    if handoff.id.is_empty()
        || handoff.id.len() > 128
        || handoff.annotation_ids.is_empty()
        || handoff.annotation_ids.len() > MAX_HANDOFF_ANNOTATIONS
        || handoff.project_revision == 0
        || handoff.semantic_generation == 0
        || handoff
            .annotation_ids
            .iter()
            .any(|id| id.is_empty() || id.len() > 128 || !ids.insert(id.as_str()))
    {
        return Err(AnnotationHandoffError::Invalid(
            "annotation handoff metadata is invalid",
        ));
    }
    Ok(())
}

fn handoff_path(project_root: &Path) -> PathBuf {
    project_root
        .join(".gpui-studio")
        .join("annotation-handoff.ron")
}

/// Invalid or unpersistable annotation handoff data.
#[derive(Debug, thiserror::Error)]
pub enum AnnotationHandoffError {
    /// No unresolved comment exists to send.
    #[error("there are no unresolved annotations to send")]
    Empty,
    /// The document exceeds the bounded local input size.
    #[error("annotation handoff is {found} bytes; maximum is {maximum}")]
    TooLarge {
        /// Observed bytes.
        found: u64,
        /// Maximum bytes.
        maximum: u64,
    },
    /// RON parsing failed.
    #[error("parse annotation handoff")]
    Parse(#[from] ron::error::SpannedError),
    /// RON serialization failed.
    #[error("serialize annotation handoff")]
    Serialize(#[from] ron::Error),
    /// The schema version is unsupported.
    #[error("annotation handoff version {found} is unsupported; expected {supported}")]
    UnsupportedVersion {
        /// Parsed version.
        found: u16,
        /// Current version.
        supported: u16,
    },
    /// Handoff data violates a bounded semantic invariant.
    #[error("invalid annotation handoff: {0}")]
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
    #[error("replace annotation handoff `{}`", path.display())]
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
    use tempfile::TempDir;

    use super::AnnotationHandoffStore;
    use crate::{AnnotationStore, ElementSelection, NormalizedAnchor};

    #[test]
    fn latest_handoff_round_trips_with_all_active_annotations()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = TempDir::new()?;
        let mut annotations = AnnotationStore::default();
        annotations.add(
            ElementSelection::new("hero", "canvas--hero", 3, None),
            "Increase contrast",
            NormalizedAnchor::default(),
        )?;
        annotations.add(
            ElementSelection::new("cta", "canvas--cta", 3, None),
            "Use a shorter label",
            NormalizedAnchor::default(),
        )?;
        let mut outbox = AnnotationHandoffStore::default();

        let handoff = outbox.publish(&annotations, 3, 9)?;
        assert_eq!(handoff.annotation_ids.len(), 2);
        outbox.save(root.path())?;

        assert_eq!(AnnotationHandoffStore::load(root.path())?, outbox);
        Ok(())
    }

    #[test]
    fn empty_handoff_is_rejected() {
        let mut outbox = AnnotationHandoffStore::default();
        let error = outbox.publish(&AnnotationStore::default(), 1, 1);
        assert!(error.is_err());
    }
}
