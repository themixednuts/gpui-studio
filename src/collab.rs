//! Person ↔ agent collaboration: chat, agent presence, and an activity feed.
//!
//! Studio has no built-in model. The person writes (or dictates) messages in
//! the chat panel; their own agent reads them over MCP (`read_messages`),
//! replies (`send_message`), says what it is working on (`set_status`), and
//! edits the design through the same commands as always. Each agent keeps its
//! own presence — selection and status — separate from the person's
//! selection, so both can work at once. The person can pause agent edits.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::model::NodeId;

const MAX_MESSAGES: usize = 500;
const MAX_ACTIVITY: usize = 300;
const MAX_TEXT: usize = 16 * 1024;
/// Presence older than this is shown as idle.
pub const PRESENCE_IDLE_MS: u64 = 120_000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Who did something.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Actor {
    /// The person at the canvas.
    Person,
    /// An MCP agent, by the name it gives.
    Agent {
        /// Display name.
        name: String,
    },
}

impl Actor {
    /// Display name.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Person => "You",
            Self::Agent { name } => name,
        }
    }

    /// An agent actor with a sanitized name (default "Agent").
    #[must_use]
    pub fn agent(name: Option<&str>) -> Self {
        let name = name
            .map(|n| {
                n.trim()
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(40)
                    .collect::<String>()
            })
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Agent".to_owned());
        Self::Agent { name }
    }
}

/// One chat message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Increasing id.
    pub id: u64,
    /// Author.
    pub author: Actor,
    /// Text.
    pub text: String,
    /// Layers the message is about (the person's selection when sent).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<NodeId>,
    /// Message this replies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<u64>,
    /// Milliseconds since the Unix epoch.
    pub at: u64,
}

/// One entry of the activity feed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    /// Increasing id.
    pub id: u64,
    /// Who acted.
    pub actor: Actor,
    /// What happened ("update_styles on Hero").
    pub summary: String,
    /// Layers involved.
    pub nodes: Vec<NodeId>,
    /// Document revision after the action.
    pub revision: u64,
    /// Milliseconds since the Unix epoch.
    pub at: u64,
}

/// What an agent is doing right now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presence {
    /// The agent's own selection.
    pub nodes: Vec<NodeId>,
    /// Short status ("Restyling the hero").
    pub status: String,
    /// Last update, ms since the epoch.
    pub at: u64,
}

/// A labelled mark an agent puts on layers (follows them as layout changes).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Annotation {
    /// Agent-chosen key (re-using it replaces the annotation).
    pub key: String,
    /// Marked layers.
    pub nodes: Vec<NodeId>,
    /// Short label.
    pub label: String,
    /// CSS color.
    pub color: String,
    /// Who added it.
    pub agent: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    messages: Vec<Message>,
    next_id: u64,
    agent_read: u64,
}

/// Collaboration state for one project.
#[derive(Debug, Default)]
pub struct Collab {
    messages: Vec<Message>,
    activity: Vec<Activity>,
    presence: BTreeMap<String, Presence>,
    annotations: Vec<Annotation>,
    next_id: u64,
    /// Highest person-message id an agent has read.
    agent_read: u64,
    /// While set, agent edits are refused.
    pub paused: bool,
    /// While set, the person's selection follows the agent's.
    pub follow: bool,
    /// Name of the agent currently talking to Studio.
    agent: String,
    path: Option<PathBuf>,
    dirty: bool,
}

impl Collab {
    /// The current agent's name ("Agent" until one introduces itself).
    #[must_use]
    pub fn agent_name(&self) -> &str {
        if self.agent.is_empty() {
            "Agent"
        } else {
            &self.agent
        }
    }

    /// Remember the name the agent gave.
    pub fn introduce(&mut self, name: Option<&str>) {
        if let Actor::Agent { name } = Actor::agent(name.or(Some(self.agent_name()))) {
            self.agent = name;
        }
    }

    /// The current agent as an actor.
    #[must_use]
    pub fn agent_actor(&self) -> Actor {
        Actor::Agent {
            name: self.agent_name().to_owned(),
        }
    }

    /// Load the chat history from `.gpui-studio/chat.ron` (missing is fine).
    #[must_use]
    pub fn load(studio_dir: &Path) -> Self {
        let path = studio_dir.join("chat.ron");
        let stored: Stored = fs::read_to_string(&path)
            .ok()
            .and_then(|text| ron::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            next_id: stored
                .next_id
                .max(stored.messages.iter().map(|m| m.id).max().unwrap_or(0)),
            messages: stored.messages,
            agent_read: stored.agent_read,
            path: Some(path),
            ..Self::default()
        }
    }

    /// Write the chat history if it changed.
    pub fn save(&mut self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if !self.dirty {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let stored = Stored {
            messages: self.messages.clone(),
            next_id: self.next_id,
            agent_read: self.agent_read,
        };
        let text = ron::ser::to_string_pretty(&stored, ron::ser::PrettyConfig::default())?;
        let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
        std::io::Write::write_all(&mut file, text.as_bytes())?;
        file.persist(path)?;
        self.dirty = false;
        Ok(())
    }

    fn next(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Post a message. Returns its id, or `None` when the text is empty.
    pub fn post(
        &mut self,
        author: Actor,
        text: &str,
        nodes: Vec<NodeId>,
        reply_to: Option<u64>,
    ) -> Option<u64> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let text: String = text.chars().take(MAX_TEXT).collect();
        let id = self.next();
        self.messages.push(Message {
            id,
            author,
            text,
            nodes,
            reply_to,
            at: now_ms(),
        });
        if self.messages.len() > MAX_MESSAGES {
            let excess = self.messages.len() - MAX_MESSAGES;
            self.messages.drain(..excess);
        }
        self.dirty = true;
        Some(id)
    }

    /// Every message, oldest first.
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Person messages an agent has not read yet.
    #[must_use]
    pub fn unread_for_agent(&self) -> usize {
        self.messages
            .iter()
            .filter(|m| m.author == Actor::Person && m.id > self.agent_read)
            .count()
    }

    /// Messages after `since` (default: after the last one an agent read),
    /// marking person messages read.
    pub fn read_for_agent(&mut self, since: Option<u64>) -> Vec<Message> {
        let since = since.unwrap_or(self.agent_read);
        let out: Vec<Message> = self
            .messages
            .iter()
            .filter(|m| m.id > since)
            .cloned()
            .collect();
        if let Some(last) = out
            .iter()
            .filter(|m| m.author == Actor::Person)
            .map(|m| m.id)
            .max()
            && last > self.agent_read
        {
            self.agent_read = last;
            self.dirty = true;
        }
        out
    }

    /// Record an action in the activity feed.
    pub fn log(&mut self, actor: Actor, summary: String, nodes: Vec<NodeId>, revision: u64) {
        let id = self.next();
        self.activity.push(Activity {
            id,
            actor,
            summary,
            nodes,
            revision,
            at: now_ms(),
        });
        if self.activity.len() > MAX_ACTIVITY {
            let excess = self.activity.len() - MAX_ACTIVITY;
            self.activity.drain(..excess);
        }
    }

    /// The activity feed, oldest first.
    #[must_use]
    pub fn activity(&self) -> &[Activity] {
        &self.activity
    }

    /// Update an agent's presence.
    pub fn set_presence(
        &mut self,
        agent: &str,
        nodes: Option<Vec<NodeId>>,
        status: Option<String>,
    ) {
        let entry = self.presence.entry(agent.to_owned()).or_insert(Presence {
            nodes: Vec::new(),
            status: String::new(),
            at: 0,
        });
        if let Some(nodes) = nodes {
            entry.nodes = nodes;
        }
        if let Some(status) = status {
            entry.status = status
                .chars()
                .filter(|c| !c.is_control())
                .take(120)
                .collect();
        }
        entry.at = now_ms();
    }

    /// Add or replace (by key) an annotation. At most 64 are kept.
    pub fn annotate(&mut self, annotation: Annotation) {
        self.annotations.retain(|a| a.key != annotation.key);
        self.annotations.push(annotation);
        if self.annotations.len() > 64 {
            self.annotations.remove(0);
        }
    }

    /// Remove annotations by key, or all of them.
    pub fn clear_annotations(&mut self, key: Option<&str>) -> usize {
        let before = self.annotations.len();
        self.annotations.retain(|a| key.is_some_and(|k| a.key != k));
        before - self.annotations.len()
    }

    /// Current annotations.
    #[must_use]
    pub fn annotations(&self) -> &[Annotation] {
        &self.annotations
    }

    /// Remove an agent's presence.
    pub fn clear_presence(&mut self, agent: &str) {
        self.presence.remove(agent);
    }

    /// Agents and what they are doing.
    #[must_use]
    pub fn presence(&self) -> &BTreeMap<String, Presence> {
        &self.presence
    }

    /// Whether an agent's presence is recent.
    #[must_use]
    pub fn is_active(presence: &Presence) -> bool {
        now_ms().saturating_sub(presence.at) < PRESENCE_IDLE_MS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_flow_between_person_and_agent_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let mut collab = Collab::load(dir.path());
        assert_eq!(collab.post(Actor::Person, "   ", Vec::new(), None), None);
        let ask = collab
            .post(
                Actor::Person,
                "Make the hero bolder",
                vec![NodeId(21)],
                None,
            )
            .unwrap();
        assert_eq!(collab.unread_for_agent(), 1);
        let read = collab.read_for_agent(None);
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].nodes, vec![NodeId(21)]);
        assert_eq!(collab.unread_for_agent(), 0);
        assert!(collab.read_for_agent(None).is_empty(), "already read");
        let agent = Actor::agent(Some("Claude"));
        collab.post(agent.clone(), "On it", Vec::new(), Some(ask));
        assert_eq!(
            collab.read_for_agent(Some(0)).len(),
            2,
            "explicit since re-reads"
        );
        collab.set_presence("Claude", Some(vec![NodeId(21)]), Some("Restyling".into()));
        assert!(Collab::is_active(&collab.presence()["Claude"]));
        collab.log(agent, "update_styles on Hero".into(), vec![NodeId(21)], 7);
        assert_eq!(collab.activity()[0].revision, 7);
        collab.save().unwrap();
        let reloaded = Collab::load(dir.path());
        assert_eq!(reloaded.messages().len(), 2);
        assert_eq!(reloaded.unread_for_agent(), 0);
        assert!(reloaded.presence().is_empty(), "presence is not persisted");
        assert_eq!(Actor::agent(Some("  ")).name(), "Agent");
    }
}
