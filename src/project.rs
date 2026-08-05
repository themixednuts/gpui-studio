use std::io::Write as _;
use std::path::{Path, PathBuf};

use gpui_mcp::LiveDocumentSource;
use gpui_mcp_html::{ProjectError, ProjectFile, ProjectPaths, ProjectSnapshot};
use tempfile::NamedTempFile;

/// Root-scoped project persistence with external-edit conflict detection.
#[derive(Clone, Debug)]
pub struct ProjectStore {
    paths: ProjectPaths,
    baseline: LiveDocumentSource,
}

impl ProjectStore {
    /// Open and snapshot one standard pure-HTML GPUI project.
    ///
    /// # Errors
    ///
    /// Returns an error for missing, oversized, non-UTF-8, or root-escaping files.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ProjectStoreError> {
        let paths = ProjectPaths::open(root)?;
        let baseline = ProjectSnapshot::load(&paths)?.into_document();
        Ok(Self { paths, baseline })
    }

    /// Canonical project root.
    #[must_use]
    pub fn root(&self) -> &Path {
        self.paths.root()
    }

    /// Validated project paths.
    #[must_use]
    pub const fn paths(&self) -> &ProjectPaths {
        &self.paths
    }

    /// Last complete bundle accepted from disk.
    #[must_use]
    pub const fn baseline(&self) -> &LiveDocumentSource {
        &self.baseline
    }

    /// Read a fresh, bounded disk snapshot without changing the baseline.
    ///
    /// # Errors
    ///
    /// Returns an error if any file is invalid or escaped since the project opened.
    pub fn read_disk(&self) -> Result<LiveDocumentSource, ProjectStoreError> {
        Ok(ProjectSnapshot::load(&self.paths)?.into_document())
    }

    /// Accept a complete source known to have just been read from these paths.
    pub fn adopt_disk(&mut self, source: LiveDocumentSource) {
        self.baseline = source;
    }

    /// Whether an active in-memory preview differs from the accepted disk bundle.
    #[must_use]
    pub fn is_dirty(&self, source: &LiveDocumentSource) -> bool {
        &self.baseline != source
    }

    /// Persist an explicitly approved complete preview.
    ///
    /// Each file is staged and flushed in the canonical `ui` directory before
    /// replacement. Saving is rejected if disk changed since the baseline, so an
    /// MCP or manual preview cannot silently overwrite an external editor.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectStoreError::Conflict`] for external changes or an I/O/path
    /// error without accepting a new baseline.
    pub fn save(&mut self, source: &LiveDocumentSource) -> Result<(), ProjectStoreError> {
        let disk = self.read_disk()?;
        if disk != self.baseline {
            return Err(ProjectStoreError::Conflict);
        }

        let staged = [
            self.stage(ProjectFile::Html, &source.html)?,
            self.stage(ProjectFile::Css, &source.css)?,
            self.stage(ProjectFile::Bindings, &source.bindings_ron)?,
        ];
        for (temporary, destination) in staged {
            temporary
                .persist(&destination)
                .map_err(|source| ProjectStoreError::Persist {
                    path: destination,
                    source,
                })?;
        }

        self.paths = ProjectPaths::open(self.paths.root())?;
        let saved = self.read_disk()?;
        if saved != *source {
            return Err(ProjectStoreError::Verification);
        }
        self.baseline = saved;
        Ok(())
    }

    fn stage(
        &self,
        file: ProjectFile,
        contents: &str,
    ) -> Result<(NamedTempFile, PathBuf), ProjectStoreError> {
        let destination = self.paths.file(file).to_owned();
        let canonical_destination =
            destination
                .canonicalize()
                .map_err(|source| ProjectStoreError::Io {
                    operation: "canonicalize destination",
                    path: destination.clone(),
                    source,
                })?;
        if !canonical_destination.starts_with(self.paths.root()) {
            return Err(ProjectStoreError::OutsideRoot {
                path: canonical_destination,
                root: self.paths.root().to_owned(),
            });
        }
        let mut temporary =
            NamedTempFile::new_in(self.paths.ui_dir()).map_err(|source| ProjectStoreError::Io {
                operation: "create staged project file",
                path: self.paths.ui_dir().to_owned(),
                source,
            })?;
        temporary
            .write_all(contents.as_bytes())
            .and_then(|()| temporary.flush())
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|source| ProjectStoreError::Io {
                operation: "write staged project file",
                path: temporary.path().to_owned(),
                source,
            })?;
        Ok((temporary, destination))
    }
}

/// Failure to read or explicitly persist a Studio project.
#[derive(Debug, thiserror::Error)]
pub enum ProjectStoreError {
    /// Source project validation or reading failed.
    #[error(transparent)]
    Project(#[from] ProjectError),
    /// Disk changed after the active preview baseline was loaded.
    #[error("project files changed externally; reload or merge before saving")]
    Conflict,
    /// A destination escaped the canonical project root.
    #[error("project path `{}` resolves outside root `{}`", path.display(), root.display())]
    OutsideRoot {
        /// Rejected destination.
        path: PathBuf,
        /// Allowed canonical root.
        root: PathBuf,
    },
    /// Staging or flushing a source file failed.
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
    #[error("replace project file `{}`", path.display())]
    Persist {
        /// Destination that could not be replaced.
        path: PathBuf,
        /// Staged-file persistence error.
        #[source]
        source: tempfile::PersistError,
    },
    /// A completed save did not round-trip to the requested bundle.
    #[error("saved project did not verify as the requested complete bundle")]
    Verification,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use gpui_mcp::LiveDocumentSource;
    use gpui_mcp_html::BindingDocument;
    use tempfile::TempDir;

    use super::{ProjectStore, ProjectStoreError};

    fn fixture() -> Result<(TempDir, ProjectStore), Box<dyn std::error::Error>> {
        let root = TempDir::new()?;
        let ui = root.path().join("ui");
        fs::create_dir(&ui)?;
        fs::write(ui.join("app.html"), "<main>one</main>")?;
        fs::write(ui.join("app.css"), "main { color: #111111; }")?;
        fs::write(
            ui.join("app.bindings.ron"),
            BindingDocument::new().to_ron_pretty()?,
        )?;
        let store = ProjectStore::open(root.path())?;
        Ok((root, store))
    }

    fn replacement(store: &ProjectStore) -> LiveDocumentSource {
        let mut source = store.baseline().clone();
        source.html = "<main>two</main>".to_owned();
        source
    }

    #[test]
    fn explicit_save_round_trips_the_complete_bundle() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, mut store) = fixture()?;
        let source = replacement(&store);

        store.save(&source)?;

        assert_eq!(store.read_disk()?, source);
        assert!(!store.is_dirty(&source));
        Ok(())
    }

    #[test]
    fn save_rejects_an_external_edit() -> Result<(), Box<dyn std::error::Error>> {
        let (root, mut store) = fixture()?;
        let source = replacement(&store);
        fs::write(root.path().join("ui/app.css"), "main { color: red; }")?;

        assert!(matches!(
            store.save(&source),
            Err(ProjectStoreError::Conflict)
        ));
        assert_eq!(
            fs::read_to_string(root.path().join("ui/app.html"))?,
            "<main>one</main>"
        );
        Ok(())
    }
}
