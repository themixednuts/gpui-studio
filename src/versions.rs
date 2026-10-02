//! Version history: named and automatic snapshots of the whole project.
//!
//! A version holds exactly the files a save writes (`studio.ron` and every
//! artboard's HTML), so restoring one is loading a project. Versions live in
//! `.gpui-studio/versions/`, one RON file each plus an index.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

/// Automatic versions kept (named ones are never pruned).
const MAX_AUTO: usize = 50;
const MAX_VERSION_BYTES: u64 = 64 * 1024 * 1024;

/// What a version is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionMeta {
    /// Milliseconds since the epoch; unique id.
    pub id: u64,
    /// Display name.
    pub name: String,
    /// Who made it ("You", or an agent's name).
    pub author: String,
    /// Made automatically (prunable).
    pub auto: bool,
    /// Content hash, to skip identical snapshots.
    hash: u64,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    meta: VersionMeta,
    files: BTreeMap<String, String>,
}

/// The version history of one project.
#[derive(Debug)]
pub struct Versions {
    dir: PathBuf,
    list: Vec<VersionMeta>,
}

fn hash(files: &BTreeMap<String, String>) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    files.hash(&mut hasher);
    hasher.finish()
}

fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let parent = path.parent().context("version path has no parent")?;
    fs::create_dir_all(parent)?;
    let mut file = NamedTempFile::new_in(parent)?;
    file.write_all(text.as_bytes())?;
    file.persist(path)?;
    Ok(())
}

impl Versions {
    /// Open the history in `<studio_dir>/versions`.
    #[must_use]
    pub fn load(studio_dir: &Path) -> Self {
        let dir = studio_dir.join("versions");
        let list = fs::read_to_string(dir.join("index.ron"))
            .ok()
            .and_then(|text| ron::from_str::<Vec<VersionMeta>>(&text).ok())
            .unwrap_or_default();
        Self { dir, list }
    }

    /// Versions, newest first.
    #[must_use]
    pub fn list(&self) -> &[VersionMeta] {
        &self.list
    }

    fn path(&self, id: u64) -> PathBuf {
        self.dir.join(format!("{id}.ron"))
    }

    fn write_index(&self) -> Result<()> {
        let text = ron::ser::to_string_pretty(&self.list, ron::ser::PrettyConfig::default())?;
        write_atomic(&self.dir.join("index.ron"), &text)
    }

    /// Record a version of `files`. Returns `None` when the newest version
    /// already has identical content (unless this one is named by a person).
    pub fn save(
        &mut self,
        files: BTreeMap<String, String>,
        name: &str,
        author: &str,
        auto: bool,
    ) -> Result<Option<VersionMeta>> {
        let content = hash(&files);
        if auto && self.list.first().is_some_and(|v| v.hash == content) {
            return Ok(None);
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let id = self.list.first().map_or(now, |v| now.max(v.id + 1));
        let name = name.trim();
        let meta = VersionMeta {
            id,
            name: if name.is_empty() {
                "Version".to_owned()
            } else {
                name.chars().take(80).collect()
            },
            author: author.to_owned(),
            auto,
            hash: content,
        };
        let stored = Stored {
            meta: meta.clone(),
            files,
        };
        write_atomic(&self.path(id), &ron::to_string(&stored)?)?;
        self.list.insert(0, meta.clone());
        // Keep every named version and the newest automatic ones.
        let mut autos = 0;
        let mut dropped = Vec::new();
        self.list.retain(|v| {
            if !v.auto {
                return true;
            }
            autos += 1;
            if autos > MAX_AUTO {
                dropped.push(v.id);
                false
            } else {
                true
            }
        });
        for id in dropped {
            let _ = fs::remove_file(self.path(id));
        }
        self.write_index()?;
        Ok(Some(meta))
    }

    /// The files of a version.
    pub fn files(&self, id: u64) -> Result<BTreeMap<String, String>> {
        if !self.list.iter().any(|v| v.id == id) {
            bail!("no version {id}");
        }
        let path = self.path(id);
        let size = fs::metadata(&path)?.len();
        if size > MAX_VERSION_BYTES {
            bail!("version {id} is too large");
        }
        let stored: Stored = ron::from_str(&fs::read_to_string(&path)?)
            .with_context(|| format!("read version {id}"))?;
        Ok(stored.files)
    }

    /// Rename a version (naming an automatic version keeps it).
    pub fn rename(&mut self, id: u64, name: &str) -> Result<()> {
        let version = self
            .list
            .iter_mut()
            .find(|v| v.id == id)
            .with_context(|| format!("no version {id}"))?;
        version.name = name.trim().chars().take(80).collect();
        version.auto = false;
        self.write_index()
    }

    /// Delete a version.
    pub fn delete(&mut self, id: u64) -> Result<()> {
        let before = self.list.len();
        self.list.retain(|v| v.id != id);
        if self.list.len() == before {
            bail!("no version {id}");
        }
        let _ = fs::remove_file(self.path(id));
        self.write_index()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(text: &str) -> BTreeMap<String, String> {
        BTreeMap::from([("studio.ron".to_owned(), text.to_owned())])
    }

    #[test]
    fn saves_dedupes_prunes_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let mut versions = Versions::load(dir.path());
        let first = versions
            .save(files("a"), "Start", "You", false)
            .unwrap()
            .unwrap();
        assert!(
            versions
                .save(files("a"), "Auto", "You", true)
                .unwrap()
                .is_none(),
            "identical"
        );
        let second = versions
            .save(files("b"), "Before Claude's edits", "Claude", true)
            .unwrap()
            .unwrap();
        assert!(second.id > first.id);
        assert_eq!(versions.list()[0].name, "Before Claude's edits");
        assert_eq!(versions.files(first.id).unwrap()["studio.ron"], "a");
        for n in 0..(MAX_AUTO + 5) {
            versions
                .save(files(&format!("x{n}")), "Auto", "You", true)
                .unwrap();
        }
        assert_eq!(versions.list().iter().filter(|v| v.auto).count(), MAX_AUTO);
        assert!(
            versions.list().iter().any(|v| v.id == first.id),
            "named versions are kept"
        );
        versions.rename(first.id, "Approved").unwrap();
        let reloaded = Versions::load(dir.path());
        assert_eq!(reloaded.list().len(), versions.list().len());
        assert!(reloaded.list().iter().any(|v| v.name == "Approved"));
        versions.delete(first.id).unwrap();
        assert!(versions.files(first.id).is_err());
    }
}
