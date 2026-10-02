//! Comments pinned to design nodes: a review queue for people and agents.
//!
//! Each comment targets a node by its persisted id plus a normalized anchor
//! inside that node, so pins follow the node as the design changes. Comments
//! are stored offline in `.gpui-studio/comments.ron` and exposed to MCP as the
//! active task queue.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

const VERSION: u16 = 1;
const MAX_BYTES: u64 = 2 * 1024 * 1024;
const MAX_COMMENTS: usize = 4096;
const MAX_BODY: usize = 16 * 1024;

/// Lifecycle of a comment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentStatus {
    /// Needs attention.
    #[default]
    Open,
    /// Someone (often an agent) is working on it.
    InProgress,
    /// Resolved; kept in history.
    Done,
}

impl CommentStatus {
    /// Whether the comment is in the active queue.
    #[must_use]
    pub fn is_active(self) -> bool {
        !matches!(self, Self::Done)
    }

    /// Parse the wire name.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "in_progress" => Some(Self::InProgress),
            "done" => Some(Self::Done),
            _ => None,
        }
    }

    /// Short label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::InProgress => "In progress",
            Self::Done => "Done",
        }
    }
}

/// One comment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    /// Monotonic project-local number.
    pub id: u64,
    /// Target node (`n42`).
    pub node: String,
    /// Horizontal anchor within the node, 0..=1.
    pub x: f32,
    /// Vertical anchor within the node, 0..=1.
    pub y: f32,
    /// Text.
    pub body: String,
    /// Who wrote it (`you` or an agent name).
    pub author: String,
    /// Lifecycle.
    pub status: CommentStatus,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Stored {
    version: u16,
    next_id: u64,
    comments: Vec<Comment>,
}

/// The project's comments, persisted on every change.
pub struct Comments {
    path: PathBuf,
    next_id: u64,
    /// All comments in creation order.
    pub items: Vec<Comment>,
}

impl Comments {
    /// Load (or start empty) from `<studio_dir>/comments.ron`.
    pub fn load(studio_dir: &Path) -> Result<Self> {
        let path = studio_dir.join("comments.ron");
        if !path.exists() {
            return Ok(Self {
                path,
                next_id: 1,
                items: Vec::new(),
            });
        }
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.len() > MAX_BYTES {
            bail!("{} is not a bounded regular file", path.display());
        }
        let stored: Stored = ron::from_str(&fs::read_to_string(&path)?)
            .with_context(|| format!("parse {}", path.display()))?;
        if stored.version != VERSION {
            bail!("unsupported comments version {}", stored.version);
        }
        let next_id = stored
            .comments
            .iter()
            .map(|c| c.id + 1)
            .max()
            .unwrap_or(1)
            .max(stored.next_id);
        Ok(Self {
            path,
            next_id,
            items: stored.comments,
        })
    }

    fn persist(&self) -> Result<()> {
        let stored = Stored {
            version: VERSION,
            next_id: self.next_id,
            comments: self.items.clone(),
        };
        let text = ron::ser::to_string_pretty(&stored, ron::ser::PrettyConfig::default())?;
        let parent = self.path.parent().context("comments path has no parent")?;
        fs::create_dir_all(parent)?;
        let mut file = NamedTempFile::new_in(parent)?;
        file.write_all(text.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(&self.path)?;
        Ok(())
    }

    /// Add a comment and persist. Returns its id.
    pub fn add(&mut self, node: &str, (x, y): (f32, f32), body: &str, author: &str) -> Result<u64> {
        if self.items.len() >= MAX_COMMENTS {
            bail!("comment limit reached");
        }
        let id = self.next_id;
        self.next_id += 1;
        self.items.push(Comment {
            id,
            node: node.to_owned(),
            x: x.clamp(0.0, 1.0),
            y: y.clamp(0.0, 1.0),
            body: truncate(body),
            author: author.to_owned(),
            status: CommentStatus::Open,
        });
        if let Err(error) = self.persist() {
            self.items.pop();
            return Err(error);
        }
        Ok(id)
    }

    /// Lookup.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&Comment> {
        self.items.iter().find(|c| c.id == id)
    }

    /// Edit body and/or status, persisting; the in-memory state only changes
    /// if the write succeeds.
    pub fn update(
        &mut self,
        id: u64,
        body: Option<&str>,
        status: Option<CommentStatus>,
    ) -> Result<()> {
        let index = self
            .items
            .iter()
            .position(|c| c.id == id)
            .with_context(|| format!("comment {id} does not exist"))?;
        let before = self.items[index].clone();
        if let Some(body) = body {
            self.items[index].body = truncate(body);
        }
        if let Some(status) = status {
            self.items[index].status = status;
        }
        if let Err(error) = self.persist() {
            self.items[index] = before;
            return Err(error);
        }
        Ok(())
    }

    /// Delete a comment.
    pub fn remove(&mut self, id: u64) -> Result<()> {
        let index = self
            .items
            .iter()
            .position(|c| c.id == id)
            .with_context(|| format!("comment {id} does not exist"))?;
        let removed = self.items.remove(index);
        if let Err(error) = self.persist() {
            self.items.insert(index, removed);
            return Err(error);
        }
        Ok(())
    }

    /// Active (not done) comments.
    pub fn active(&self) -> impl Iterator<Item = &Comment> {
        self.items.iter().filter(|c| c.status.is_active())
    }
}

fn truncate(body: &str) -> String {
    let mut body = body.to_owned();
    if body.len() > MAX_BODY {
        let mut cut = MAX_BODY;
        while !body.is_char_boundary(cut) {
            cut -= 1;
        }
        body.truncate(cut);
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let mut comments = Comments::load(dir.path()).unwrap();
        let id = comments
            .add("n3", (0.2, 2.0), "Make this bolder", "you")
            .unwrap();
        comments
            .update(id, None, Some(CommentStatus::InProgress))
            .unwrap();
        let reloaded = Comments::load(dir.path()).unwrap();
        let comment = reloaded.get(id).unwrap();
        assert_eq!(comment.status, CommentStatus::InProgress);
        assert_eq!(comment.y, 1.0);
        assert_eq!(reloaded.active().count(), 1);
        let mut reloaded = reloaded;
        reloaded
            .update(id, None, Some(CommentStatus::Done))
            .unwrap();
        assert_eq!(reloaded.active().count(), 0);
        let next = reloaded.add("n4", (0.5, 0.5), "x", "agent").unwrap();
        assert!(next > id);
    }
}
