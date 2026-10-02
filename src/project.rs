//! On-disk project layout and persistence.
//!
//! ```text
//! my-design/
//!   studio.ron              pages, artboard files, and canvas positions
//!   artboards/*.html        one standalone HTML page per artboard
//!   .gpui-studio/comments.ron
//! ```
//!
//! Every artboard file opens in a browser as-is. Writes are atomic and only
//! touch files whose content changed; edits made on disk by other tools are
//! detected and merged back into the open document.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use anyhow::{Context as _, Result, bail};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::model::html::{ImportOptions, artboard_document, parse_document};
use crate::model::{Artboard, Document, NodeId, Page};

const MANIFEST: &str = "studio.ron";
const ARTBOARDS: &str = "artboards";
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const KEEP_IDS: ImportOptions = ImportOptions { keep_ids: true };

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    version: u16,
    name: String,
    pages: Vec<ManifestPage>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ManifestPage {
    name: String,
    artboards: Vec<ManifestArtboard>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ManifestArtboard {
    file: String,
    x: f32,
    y: f32,
}

/// An open project directory.
pub struct Project {
    root: PathBuf,
    /// Display name.
    pub name: String,
    /// Content last written or read per file, to skip no-op writes and ignore
    /// our own filesystem events.
    known: BTreeMap<String, String>,
    known_manifest: String,
    watcher: Option<(RecommendedWatcher, Receiver<notify::Result<notify::Event>>)>,
}

/// Whether a manifest file name is a plain file inside `artboards/`.
fn valid_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.ends_with(".html")
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    let parent = path.parent().context("file has no parent directory")?;
    fs::create_dir_all(parent)?;
    let mut file = NamedTempFile::new_in(parent)?;
    file.write_all(contents.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path)
        .with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

fn read_bounded(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        bail!("{} is not a regular file", path.display());
    }
    if metadata.len() > MAX_FILE_BYTES {
        bail!("{} is larger than {MAX_FILE_BYTES} bytes", path.display());
    }
    Ok(fs::read_to_string(path)?)
}

impl Project {
    /// Project root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Absolute path of an artboard file.
    #[must_use]
    pub fn artboard_path(&self, file: &str) -> PathBuf {
        self.root.join(ARTBOARDS).join(file)
    }

    /// Directory for editor-owned state.
    #[must_use]
    pub fn studio_dir(&self) -> PathBuf {
        self.root.join(".gpui-studio")
    }

    /// Open a project, scaffolding a starter design when the folder has none.
    pub fn open(root: &Path) -> Result<(Self, Document)> {
        fs::create_dir_all(root).with_context(|| format!("create {}", root.display()))?;
        let root = root.canonicalize()?;
        let manifest_path = root.join(MANIFEST);
        if !manifest_path.exists() {
            let name = root.file_name().map_or_else(
                || "Untitled".to_owned(),
                |n| n.to_string_lossy().into_owned(),
            );
            let mut project = Self {
                root: root.clone(),
                name,
                known: BTreeMap::new(),
                known_manifest: String::new(),
                watcher: None,
            };
            let doc = crate::presets::starter_document();
            project.save(&doc)?;
            return Ok((project, doc));
        }
        let mut project = Self {
            root,
            name: String::new(),
            known: BTreeMap::new(),
            known_manifest: String::new(),
            watcher: None,
        };
        let doc = project.load()?;
        Ok((project, doc))
    }

    /// Read the manifest and every artboard from disk.
    pub fn load(&mut self) -> Result<Document> {
        let manifest_text = read_bounded(&self.root.join(MANIFEST))?;
        let manifest: Manifest = ron::from_str(&manifest_text).context("parse studio.ron")?;
        if manifest.version != 1 {
            bail!("unsupported studio.ron version {}", manifest.version);
        }
        self.name = manifest.name;
        self.known.clear();
        self.known_manifest = manifest_text;
        let mut doc = Document::new();
        doc.pages.clear();
        for page in manifest.pages {
            let mut artboards = Vec::new();
            for entry in page.artboards {
                if !valid_file_name(&entry.file) {
                    bail!("invalid artboard file name {:?}", entry.file);
                }
                let path = self.artboard_path(&entry.file);
                let source = match read_bounded(&path) {
                    Ok(source) => source,
                    Err(error) => {
                        eprintln!("skipping artboard {}: {error:#}", entry.file);
                        continue;
                    }
                };
                let root = parse_document(&mut doc, &source, KEEP_IDS);
                self.known.insert(entry.file.clone(), source);
                artboards.push(Artboard {
                    root,
                    file: entry.file,
                    x: entry.x,
                    y: entry.y,
                });
            }
            doc.pages.push(Page {
                name: page.name,
                artboards,
            });
        }
        if doc.pages.is_empty() {
            doc.pages.push(Page {
                name: "Page 1".to_owned(),
                artboards: Vec::new(),
            });
        }
        Ok(doc)
    }

    fn manifest_text(&self, doc: &Document) -> Result<String> {
        let manifest = Manifest {
            version: 1,
            name: self.name.clone(),
            pages: doc
                .pages
                .iter()
                .map(|page| ManifestPage {
                    name: page.name.clone(),
                    artboards: page
                        .artboards
                        .iter()
                        .map(|a| ManifestArtboard {
                            file: a.file.clone(),
                            x: a.x.round(),
                            y: a.y.round(),
                        })
                        .collect(),
                })
                .collect(),
        };
        Ok(ron::ser::to_string_pretty(&manifest, ron::ser::PrettyConfig::default())? + "\n")
    }

    /// Write changed artboards and the manifest. Returns how many files changed.
    pub fn save(&mut self, doc: &Document) -> Result<usize> {
        let mut written = 0;
        let mut live = BTreeMap::new();
        for artboard in doc.artboards() {
            if !valid_file_name(&artboard.file) {
                bail!("invalid artboard file name {:?}", artboard.file);
            }
            let html = artboard_document(doc, artboard.root);
            if self.known.get(&artboard.file) != Some(&html) {
                atomic_write(&self.artboard_path(&artboard.file), &html)?;
                written += 1;
            }
            live.insert(artboard.file.clone(), html);
        }
        // Remove files of artboards deleted in Studio (only ones Studio owned).
        for file in self.known.keys() {
            if !live.contains_key(file) {
                let path = self.artboard_path(file);
                if path.is_file() {
                    fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
                    written += 1;
                }
            }
        }
        self.known = live;
        let manifest = self.manifest_text(doc)?;
        if manifest != self.known_manifest {
            atomic_write(&self.root.join(MANIFEST), &manifest)?;
            self.known_manifest = manifest;
            written += 1;
        }
        Ok(written)
    }

    /// Start watching the project for edits made by other tools.
    pub fn watch(&mut self) -> Result<()> {
        let (sender, receiver) = channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        })?;
        fs::create_dir_all(self.root.join(ARTBOARDS))?;
        watcher.watch(&self.root.join(ARTBOARDS), RecursiveMode::NonRecursive)?;
        watcher.watch(&self.root, RecursiveMode::NonRecursive)?;
        self.watcher = Some((watcher, receiver));
        Ok(())
    }

    /// Drain watcher events; returns whether anything on disk may have changed.
    pub fn poll_events(&mut self) -> bool {
        let Some((_, receiver)) = &self.watcher else {
            return false;
        };
        let mut any = false;
        while let Ok(event) = receiver.try_recv() {
            if let Ok(event) = event
                && !matches!(event.kind, notify::EventKind::Access(_))
            {
                any = true;
            }
        }
        any
    }

    /// Merge external changes into `doc`. Returns the artboard files that
    /// changed, or `["studio.ron"]` when the whole project reloaded.
    pub fn sync_external(&mut self, doc: &mut Document) -> Result<Vec<String>> {
        let manifest_path = self.root.join(MANIFEST);
        if let Ok(text) = read_bounded(&manifest_path)
            && text != self.known_manifest
        {
            *doc = self.load()?;
            return Ok(vec![MANIFEST.to_owned()]);
        }
        let mut changed = Vec::new();
        let artboards: Vec<Artboard> = doc.artboards().cloned().collect();
        for artboard in artboards {
            let Ok(source) = read_bounded(&self.artboard_path(&artboard.file)) else {
                continue;
            };
            if self.known.get(&artboard.file) == Some(&source) {
                continue;
            }
            let _ = doc.remove_subtree_keep_artboard(artboard.root);
            let root = parse_document(doc, &source, KEEP_IDS);
            doc.replace_artboard_root(artboard.root, root);
            self.known.insert(artboard.file.clone(), source);
            changed.push(artboard.file);
        }
        Ok(changed)
    }
}

impl Document {
    /// Remove an artboard's node tree but keep its slot for replacement.
    pub(crate) fn remove_subtree_keep_artboard(&mut self, root: NodeId) -> Option<()> {
        let slot = self.artboard_index(root)?;
        let entry = self.pages[slot.0].artboards[slot.1].clone();
        self.remove(root).ok()?;
        self.pages[slot.0].artboards.insert(slot.1, entry);
        Some(())
    }

    /// Point an artboard slot at a new root element.
    pub(crate) fn replace_artboard_root(&mut self, old: NodeId, new: NodeId) {
        if let Some(artboard) = self.artboard_mut(old) {
            artboard.root = new;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffolds_saves_reloads_and_merges_external_edits() {
        let dir = tempfile::tempdir().unwrap();
        let (mut project, mut doc) = Project::open(dir.path()).unwrap();
        assert!(dir.path().join("studio.ron").is_file());
        let first = doc.pages[0].artboards[0].clone();
        assert!(project.artboard_path(&first.file).is_file());

        // A no-op save writes nothing.
        assert_eq!(project.save(&doc).unwrap(), 0);

        // Edit in Studio and save.
        let child = doc.new_element("p");
        doc.attach(child, first.root, None).unwrap();
        doc.set_text(child, "Saved from Studio").unwrap();
        assert_eq!(project.save(&doc).unwrap(), 1);

        let (_, reopened) = Project::open(dir.path()).unwrap();
        let root = reopened.pages[0].artboards[0].root;
        assert_eq!(root, first.root);
        assert!(
            reopened
                .text_content(root)
                .unwrap()
                .contains("Saved from Studio")
        );

        // Edit on disk and merge.
        let path = project.artboard_path(&first.file);
        let text = fs::read_to_string(&path)
            .unwrap()
            .replace("Saved from Studio", "Edited on disk");
        fs::write(&path, text).unwrap();
        let changed = project.sync_external(&mut doc).unwrap();
        assert_eq!(changed, vec![first.file.clone()]);
        let root = doc.pages[0].artboards[0].root;
        assert_eq!(root, first.root, "persisted ids survive external edits");
        assert!(doc.text_content(root).unwrap().contains("Edited on disk"));
        assert_eq!(doc.pages[0].artboards[0].x, first.x);

        // Deleting an artboard removes its file.
        let file = doc.pages[0].artboards[0].file.clone();
        doc.remove(root).unwrap();
        project.save(&doc).unwrap();
        assert!(!project.artboard_path(&file).exists());
    }

    #[test]
    fn rejects_escaping_file_names() {
        assert!(valid_file_name("home-2.html"));
        assert!(!valid_file_name("../x.html"));
        assert!(!valid_file_name("a/b.html"));
        assert!(!valid_file_name(".hidden.html"));
    }
}
