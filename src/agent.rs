//! MCP tools, resources, and the live document an agent uses to design.
//!
//! These map one-to-one onto [`Editor`] operations, so an agent's edits are
//! undoable, autosaved, and visible on the canvas immediately, exactly like a
//! person's. Node ids are the `data-id` values (`n42`) that appear in every
//! HTML snippet the tools return.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::comments::CommentStatus;
use crate::editor::{Editor, InsertTarget};
use crate::export::{CodeFormat, export};
use crate::model::html::{ImportOptions, artboard_document, parse_document};
use crate::model::{NodeId, NodeKind};
use crate::presets::{COMPONENTS, starter_document};

/// One command's metadata.
pub struct CommandSpec {
    /// Name.
    pub name: &'static str,
    /// Title.
    pub title: &'static str,
    /// What it does.
    pub description: &'static str,
    /// JSON schema of the arguments.
    pub schema: fn() -> Value,
    /// Whether it changes the design.
    pub mutating: bool,
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

const NODE: &str = "Node id such as \"n42\" (the data-id attribute in returned HTML).";

/// Every command, in discovery order.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "get_document",
        title: "Get document",
        description: "Pages and artboards (id, name, canvas position, size), the selection, and the revision.",
        schema: || object(json!({}), &[]),
        mutating: false,
    },
    CommandSpec {
        name: "get_selection",
        title: "Get selection",
        description: "Selected node ids with their HTML (including data-id attributes for targeting children).",
        schema: || object(json!({}), &[]),
        mutating: false,
    },
    CommandSpec {
        name: "get_node",
        title: "Get node HTML",
        description: "HTML of one node's subtree with inline styles and data-id attributes.",
        schema: || {
            object(
                json!({ "node_id": { "type": "string", "description": NODE } }),
                &["node_id"],
            )
        },
        mutating: false,
    },
    CommandSpec {
        name: "get_tree",
        title: "Get layer tree",
        description: "Compact layer tree (id, name, tag, text) under a node, or for the current page.",
        schema: || {
            object(
                json!({
                    "node_id": { "type": "string", "description": NODE },
                    "depth": { "type": "integer", "minimum": 1, "maximum": 32 }
                }),
                &[],
            )
        },
        mutating: false,
    },
    CommandSpec {
        name: "select_nodes",
        title: "Select nodes",
        description: "Replace the canvas selection so the person sees what you are working on.",
        schema: || {
            object(
                json!({ "node_ids": { "type": "array", "items": { "type": "string" } } }),
                &["node_ids"],
            )
        },
        mutating: false,
    },
    CommandSpec {
        name: "create_artboard",
        title: "Create artboard",
        description: "Create an artboard (top-level frame) on the current page, optionally filled with HTML.",
        schema: || {
            object(
                json!({
                    "name": { "type": "string" },
                    "width": { "type": "number", "minimum": 1 },
                    "height": { "type": "number", "minimum": 1 },
                    "x": { "type": "number" },
                    "y": { "type": "number" },
                    "html": { "type": "string", "description": "Optional content inserted inside the artboard." }
                }),
                &["name", "width", "height"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "write_html",
        title: "Write HTML",
        description: "Insert HTML into a parent node (or as new artboards when parent_id is omitted). Use inline styles; <style> class rules are inlined; scripts are dropped. Returns new node ids.",
        schema: || {
            object(
                json!({
                    "html": { "type": "string" },
                    "parent_id": { "type": "string", "description": NODE },
                    "index": { "type": "integer", "minimum": 0 }
                }),
                &["html"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "replace_html",
        title: "Replace node with HTML",
        description: "Replace a node (or a whole artboard) with new HTML, keeping its position. data-id values in the HTML are preserved when free.",
        schema: || {
            object(
                json!({ "node_id": { "type": "string", "description": NODE }, "html": { "type": "string" } }),
                &["node_id", "html"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "update_styles",
        title: "Update styles",
        description: "Set CSS properties on nodes; a null value removes the property.",
        schema: || {
            object(
                json!({
                    "node_ids": { "type": "array", "items": { "type": "string" } },
                    "styles": { "type": "object", "additionalProperties": { "type": ["string", "number", "null"] } }
                }),
                &["node_ids", "styles"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "set_text",
        title: "Set text",
        description: "Replace a node's content with plain text.",
        schema: || {
            object(
                json!({ "node_id": { "type": "string", "description": NODE }, "text": { "type": "string" } }),
                &["node_id", "text"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "set_attributes",
        title: "Set attributes",
        description: "Set HTML attributes (src, alt, href, role, aria-*...); null removes. Use rename for layer names.",
        schema: || {
            object(
                json!({
                    "node_id": { "type": "string", "description": NODE },
                    "attributes": { "type": "object", "additionalProperties": { "type": ["string", "null"] } }
                }),
                &["node_id", "attributes"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "rename",
        title: "Rename layer",
        description: "Set a node's layer name.",
        schema: || {
            object(
                json!({ "node_id": { "type": "string", "description": NODE }, "name": { "type": "string" } }),
                &["node_id", "name"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "delete_nodes",
        title: "Delete nodes",
        description: "Delete nodes or artboards.",
        schema: || {
            object(
                json!({ "node_ids": { "type": "array", "items": { "type": "string" } } }),
                &["node_ids"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "duplicate_nodes",
        title: "Duplicate nodes",
        description: "Duplicate nodes next to the originals; returns the copies' ids.",
        schema: || {
            object(
                json!({ "node_ids": { "type": "array", "items": { "type": "string" } } }),
                &["node_ids"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "move_node",
        title: "Move node",
        description: "Move a node into a parent at an index (append when omitted).",
        schema: || {
            object(
                json!({
                    "node_id": { "type": "string", "description": NODE },
                    "parent_id": { "type": "string", "description": NODE },
                    "index": { "type": "integer", "minimum": 0 }
                }),
                &["node_id", "parent_id"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "list_components",
        title: "List components",
        description: "Built-in component library keys and descriptions.",
        schema: || object(json!({}), &[]),
        mutating: false,
    },
    CommandSpec {
        name: "insert_component",
        title: "Insert component",
        description: "Insert a library component (see list_components) into a parent, or as an artboard.",
        schema: || {
            object(
                json!({
                    "component": { "type": "string" },
                    "parent_id": { "type": "string", "description": NODE },
                    "index": { "type": "integer", "minimum": 0 }
                }),
                &["component"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "export_code",
        title: "Export code",
        description: "Export a node as clean HTML, HTML + CSS classes, or GPUI Rust builder code.",
        schema: || {
            object(
                json!({
                    "node_id": { "type": "string", "description": NODE },
                    "format": { "type": "string", "enum": ["html", "html_css", "gpui"] }
                }),
                &["node_id", "format"],
            )
        },
        mutating: false,
    },
    CommandSpec {
        name: "list_comments",
        title: "List comments",
        description: "Review comments pinned to nodes. Open and in-progress comments are the work queue.",
        schema: || object(json!({ "include_done": { "type": "boolean" } }), &[]),
        mutating: false,
    },
    CommandSpec {
        name: "add_comment",
        title: "Add comment",
        description: "Pin a comment to a node.",
        schema: || {
            object(
                json!({ "node_id": { "type": "string", "description": NODE }, "body": { "type": "string" } }),
                &["node_id", "body"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "update_comment",
        title: "Update comment",
        description: "Change a comment's status (open, in_progress, done) or body. Mark it done after addressing it.",
        schema: || {
            object(
                json!({
                    "comment_id": { "type": "integer" },
                    "status": { "type": "string", "enum": ["open", "in_progress", "done"] },
                    "body": { "type": "string" }
                }),
                &["comment_id"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "undo",
        title: "Undo",
        description: "Undo the last edit.",
        schema: || object(json!({}), &[]),
        mutating: true,
    },
    CommandSpec {
        name: "redo",
        title: "Redo",
        description: "Redo the last undone edit.",
        schema: || object(json!({}), &[]),
        mutating: true,
    },
    CommandSpec {
        name: "save",
        title: "Save",
        description: "Write pending changes to the project files now (Studio also autosaves).",
        schema: || object(json!({}), &[]),
        mutating: true,
    },
];

/// A command failure with a user-facing message.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// Bad or missing arguments.
    #[error("{0}")]
    Invalid(String),
    /// The command does not exist.
    #[error("unknown command {0:?}")]
    Unknown(String),
    /// The edit failed.
    #[error("{0:#}")]
    Failed(anyhow::Error),
}

impl From<anyhow::Error> for AgentError {
    fn from(error: anyhow::Error) -> Self {
        Self::Failed(error)
    }
}

fn args<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, AgentError> {
    let value = if value.is_null() { json!({}) } else { value };
    serde_json::from_value(value).map_err(|error| AgentError::Invalid(error.to_string()))
}

fn node(editor: &Editor, id: &str) -> Result<NodeId, AgentError> {
    NodeId::parse(id)
        .filter(|id| editor.doc.contains(*id))
        .ok_or_else(|| AgentError::Invalid(format!("node {id:?} does not exist")))
}

fn nodes(editor: &Editor, ids: &[String]) -> Result<Vec<NodeId>, AgentError> {
    ids.iter().map(|id| node(editor, id)).collect()
}

fn ids_json(ids: &[NodeId]) -> Value {
    json!(ids.iter().map(ToString::to_string).collect::<Vec<_>>())
}

fn size_of(editor: &Editor, id: NodeId) -> (Option<f32>, Option<f32>) {
    let computed = editor
        .doc
        .get(id)
        .map(|n| n.style.computed())
        .unwrap_or_default();
    (computed.width.px(), computed.height.px())
}

/// Summary of pages and artboards.
#[must_use]
pub fn document_json(editor: &Editor) -> Value {
    let pages: Vec<Value> = editor
        .doc
        .pages
        .iter()
        .enumerate()
        .map(|(index, page)| {
            json!({
                "index": index,
                "name": page.name,
                "current": index == editor.page,
                "artboards": page.artboards.iter().map(|a| {
                    let (width, height) = size_of(editor, a.root);
                    json!({
                        "id": a.root.to_string(),
                        "name": editor.doc.display_name(a.root),
                        "file": a.file,
                        "x": a.x, "y": a.y, "width": width, "height": height,
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "project": editor.project.as_ref().map(|p| p.name.clone()),
        "revision": editor.revision,
        "pages": pages,
        "selection": ids_json(&editor.selection),
    })
}

/// Current selection with HTML.
#[must_use]
pub fn selection_json(editor: &Editor) -> Value {
    json!({
        "revision": editor.revision,
        "nodes": editor.selection.iter().map(|id| json!({
            "id": id.to_string(),
            "name": editor.doc.display_name(*id),
            "artboard": editor.doc.artboard_of(*id).map(|a| a.root.to_string()),
            "html": editor.html_of(*id, true),
        })).collect::<Vec<_>>(),
    })
}

fn tree_json(editor: &Editor, id: NodeId, depth: usize) -> Value {
    let Some(node) = editor.doc.get(id) else {
        return Value::Null;
    };
    let mut value = json!({
        "id": id.to_string(),
        "name": editor.doc.display_name(id),
        "tag": node.tag(),
    });
    if node.hidden {
        value["hidden"] = json!(true);
    }
    if editor.doc.is_text_layer(id) {
        value["text"] = json!(editor.doc.text_content(id));
        return value;
    }
    let children: Vec<NodeId> = node
        .children
        .iter()
        .copied()
        .filter(|c| !matches!(editor.doc.get(*c).map(|n| &n.kind), Some(NodeKind::Text(t)) if t.trim().is_empty()))
        .collect();
    if !children.is_empty() {
        value["children"] = if depth == 0 {
            json!(format!("{} children", children.len()))
        } else {
            json!(
                children
                    .iter()
                    .map(|c| tree_json(editor, *c, depth - 1))
                    .collect::<Vec<_>>()
            )
        };
    }
    value
}

/// Comments as JSON.
#[must_use]
pub fn comments_json(editor: &Editor, include_done: bool) -> Value {
    let Some(comments) = &editor.comments else {
        return json!([]);
    };
    json!(comments
        .items
        .iter()
        .filter(|c| include_done || c.status.is_active())
        .map(|c| json!({
            "id": c.id,
            "node_id": c.node,
            "node_name": NodeId::parse(&c.node).filter(|id| editor.doc.contains(*id)).map(|id| editor.doc.display_name(id)),
            "artboard": NodeId::parse(&c.node).and_then(|id| editor.doc.artboard_of(id)).map(|a| a.root.to_string()),
            "body": c.body,
            "author": c.author,
            "status": c.status,
        }))
        .collect::<Vec<_>>())
}

#[derive(Deserialize)]
struct NodeArg {
    node_id: String,
}

#[derive(Deserialize)]
struct NodesArg {
    node_ids: Vec<String>,
}

/// Execute one command.
pub fn execute(editor: &mut Editor, name: &str, arguments: Value) -> Result<Value, AgentError> {
    match name {
        "get_document" => Ok(document_json(editor)),
        "get_selection" => Ok(selection_json(editor)),
        "get_node" => {
            let a: NodeArg = args(arguments)?;
            let id = node(editor, &a.node_id)?;
            Ok(json!({ "id": id.to_string(), "html": editor.html_of(id, true) }))
        }
        "get_tree" => {
            #[derive(Deserialize)]
            struct A {
                node_id: Option<String>,
                depth: Option<usize>,
            }
            let a: A = args(arguments)?;
            let depth = a.depth.unwrap_or(4).clamp(1, 32);
            match a.node_id {
                Some(id) => Ok(tree_json(editor, node(editor, &id)?, depth)),
                None => {
                    let page = editor.doc.pages.get(editor.page);
                    Ok(json!({
                        "page": page.map(|p| p.name.clone()),
                        "artboards": page.map(|p| p.artboards.iter().map(|a| tree_json(editor, a.root, depth)).collect::<Vec<_>>()),
                    }))
                }
            }
        }
        "select_nodes" => {
            let a: NodesArg = args(arguments)?;
            let ids = nodes(editor, &a.node_ids)?;
            editor.select(ids);
            Ok(json!({ "selection": ids_json(&editor.selection) }))
        }
        "create_artboard" => {
            #[derive(Deserialize)]
            struct A {
                name: String,
                width: f32,
                height: f32,
                x: Option<f32>,
                y: Option<f32>,
                html: Option<String>,
            }
            let a: A = args(arguments)?;
            if !(1.0..=20_000.0).contains(&a.width) || !(1.0..=20_000.0).contains(&a.height) {
                return Err(AgentError::Invalid(
                    "width and height must be 1..=20000".into(),
                ));
            }
            let position = match (a.x, a.y) {
                (Some(x), Some(y)) => Some((x, y)),
                _ => None,
            };
            let id = editor.create_artboard(&a.name, (a.width, a.height), position)?;
            let children = match a.html {
                Some(html) if !html.trim().is_empty() => editor.insert_html(
                    &html,
                    InsertTarget {
                        parent: Some(id),
                        index: None,
                    },
                )?,
                _ => Vec::new(),
            };
            editor.select([id]);
            Ok(
                json!({ "id": id.to_string(), "children": ids_json(&children), "revision": editor.revision }),
            )
        }
        "write_html" => {
            #[derive(Deserialize)]
            struct A {
                html: String,
                parent_id: Option<String>,
                index: Option<usize>,
            }
            let a: A = args(arguments)?;
            let parent = a.parent_id.map(|id| node(editor, &id)).transpose()?;
            let ids = editor.insert_html(
                &a.html,
                InsertTarget {
                    parent,
                    index: a.index,
                },
            )?;
            Ok(json!({ "ids": ids_json(&ids), "revision": editor.revision }))
        }
        "replace_html" => {
            #[derive(Deserialize)]
            struct A {
                node_id: String,
                html: String,
            }
            let a: A = args(arguments)?;
            let id = node(editor, &a.node_id)?;
            let ids = editor.replace_html(id, &a.html)?;
            Ok(json!({ "ids": ids_json(&ids), "revision": editor.revision }))
        }
        "update_styles" => {
            #[derive(Deserialize)]
            struct A {
                node_ids: Vec<String>,
                styles: serde_json::Map<String, Value>,
            }
            let a: A = args(arguments)?;
            let ids = nodes(editor, &a.node_ids)?;
            let changes = a
                .styles
                .into_iter()
                .map(|(property, value)| {
                    let value = match value {
                        Value::Null => None,
                        Value::String(s) => Some(s),
                        Value::Number(n) => Some(n.to_string()),
                        other => Some(other.to_string()),
                    };
                    (property, value)
                })
                .collect::<Vec<_>>();
            editor.set_styles(&ids, &changes, None)?;
            Ok(json!({ "revision": editor.revision }))
        }
        "set_text" => {
            #[derive(Deserialize)]
            struct A {
                node_id: String,
                text: String,
            }
            let a: A = args(arguments)?;
            let id = node(editor, &a.node_id)?;
            editor.set_text(id, &a.text, None)?;
            Ok(json!({ "revision": editor.revision }))
        }
        "set_attributes" => {
            #[derive(Deserialize)]
            struct A {
                node_id: String,
                attributes: serde_json::Map<String, Value>,
            }
            let a: A = args(arguments)?;
            let id = node(editor, &a.node_id)?;
            let attrs = a
                .attributes
                .into_iter()
                .map(|(k, v)| (k, v.as_str().map(ToOwned::to_owned)))
                .collect::<Vec<_>>();
            editor.set_attributes(id, &attrs)?;
            Ok(json!({ "revision": editor.revision }))
        }
        "rename" => {
            #[derive(Deserialize)]
            struct A {
                node_id: String,
                name: String,
            }
            let a: A = args(arguments)?;
            let id = node(editor, &a.node_id)?;
            editor.rename(id, &a.name)?;
            Ok(json!({ "revision": editor.revision }))
        }
        "delete_nodes" => {
            let a: NodesArg = args(arguments)?;
            let ids = nodes(editor, &a.node_ids)?;
            let count = editor.delete(&ids)?;
            Ok(json!({ "deleted": count, "revision": editor.revision }))
        }
        "duplicate_nodes" => {
            let a: NodesArg = args(arguments)?;
            let ids = nodes(editor, &a.node_ids)?;
            let copies = editor.duplicate(&ids)?;
            Ok(json!({ "ids": ids_json(&copies), "revision": editor.revision }))
        }
        "move_node" => {
            #[derive(Deserialize)]
            struct A {
                node_id: String,
                parent_id: String,
                index: Option<usize>,
            }
            let a: A = args(arguments)?;
            let id = node(editor, &a.node_id)?;
            let parent = node(editor, &a.parent_id)?;
            editor.move_node(id, parent, a.index)?;
            Ok(json!({ "revision": editor.revision }))
        }
        "list_components" => {
            Ok(json!(COMPONENTS
            .iter()
            .map(|c| json!({ "component": c.key, "name": c.name, "description": c.description }))
            .collect::<Vec<_>>()))
        }
        "insert_component" => {
            #[derive(Deserialize)]
            struct A {
                component: String,
                parent_id: Option<String>,
                index: Option<usize>,
            }
            let a: A = args(arguments)?;
            let parent = a.parent_id.map(|id| node(editor, &id)).transpose()?;
            let ids = editor.insert_component(
                &a.component,
                InsertTarget {
                    parent,
                    index: a.index,
                },
            )?;
            Ok(json!({ "ids": ids_json(&ids), "revision": editor.revision }))
        }
        "export_code" => {
            #[derive(Deserialize)]
            struct A {
                node_id: String,
                format: String,
            }
            let a: A = args(arguments)?;
            let id = node(editor, &a.node_id)?;
            let format = CodeFormat::parse(&a.format)
                .ok_or_else(|| AgentError::Invalid(format!("unknown format {:?}", a.format)))?;
            Ok(json!({ "code": export(&editor.doc, id, format) }))
        }
        "list_comments" => {
            #[derive(Deserialize)]
            struct A {
                include_done: Option<bool>,
            }
            let a: A = args(arguments)?;
            Ok(comments_json(editor, a.include_done.unwrap_or(false)))
        }
        "add_comment" => {
            #[derive(Deserialize)]
            struct A {
                node_id: String,
                body: String,
            }
            let a: A = args(arguments)?;
            let id = node(editor, &a.node_id)?;
            let comments = editor
                .comments
                .as_mut()
                .ok_or_else(|| AgentError::Invalid("comments need an open project".into()))?;
            let comment = comments.add(&id.to_string(), (0.5, 0.5), &a.body, "agent")?;
            Ok(json!({ "comment_id": comment }))
        }
        "update_comment" => {
            #[derive(Deserialize)]
            struct A {
                comment_id: u64,
                status: Option<String>,
                body: Option<String>,
            }
            let a: A = args(arguments)?;
            let status = a
                .status
                .map(|s| {
                    CommentStatus::parse(&s)
                        .ok_or_else(|| AgentError::Invalid(format!("unknown status {s:?}")))
                })
                .transpose()?;
            let comments = editor
                .comments
                .as_mut()
                .ok_or_else(|| AgentError::Invalid("comments need an open project".into()))?;
            comments.update(a.comment_id, a.body.as_deref(), status)?;
            Ok(json!({ "ok": true }))
        }
        "undo" => Ok(json!({ "undone": editor.undo(), "revision": editor.revision })),
        "redo" => Ok(json!({ "redone": editor.redo(), "revision": editor.revision })),
        "save" => {
            editor.save()?;
            Ok(json!({ "saved": true, "revision": editor.revision }))
        }
        other => Err(AgentError::Unknown(other.to_owned())),
    }
}

/// MCP resource URIs.
pub const RESOURCES: &[(&str, &str, &str)] = &[
    (
        "gpui-studio://document",
        "document",
        "Pages, artboards, selection, and revision.",
    ),
    (
        "gpui-studio://selection",
        "selection",
        "Selected nodes with HTML.",
    ),
    (
        "gpui-studio://comments/active",
        "active-comments",
        "Open and in-progress review comments: the work queue.",
    ),
    (
        "gpui-studio://comments/all",
        "all-comments",
        "Every comment including resolved history.",
    ),
    (
        "gpui-studio://components",
        "components",
        "Built-in component library.",
    ),
];

/// Read one resource.
#[must_use]
pub fn read_resource(editor: &Editor, uri: &str) -> Option<String> {
    let value = match uri {
        "gpui-studio://document" => document_json(editor),
        "gpui-studio://selection" => selection_json(editor),
        "gpui-studio://comments/active" => comments_json(editor, false),
        "gpui-studio://comments/all" => comments_json(editor, true),
        "gpui-studio://components" => execute_readonly(editor, "list_components"),
        _ => return None,
    };
    serde_json::to_string_pretty(&value).ok()
}

fn execute_readonly(editor: &Editor, name: &str) -> Value {
    match name {
        "list_components" => {
            json!(COMPONENTS
            .iter()
            .map(|c| json!({ "component": c.key, "name": c.name, "description": c.description }))
            .collect::<Vec<_>>())
        }
        _ => document_json(editor),
    }
}

/// The artboard the live document refers to: the selection's artboard, else
/// the first on the current page.
#[must_use]
pub fn live_artboard(editor: &Editor) -> Option<NodeId> {
    editor
        .primary()
        .and_then(|id| editor.doc.artboard_of(id))
        .map(|a| a.root)
        .or_else(|| {
            editor
                .doc
                .pages
                .get(editor.page)
                .and_then(|p| p.artboards.first())
                .map(|a| a.root)
        })
}

/// Live-document HTML for the active artboard.
#[must_use]
pub fn live_html(editor: &Editor) -> String {
    live_artboard(editor).map_or_else(String::new, |root| artboard_document(&editor.doc, root))
}

/// Replace the active artboard from a complete HTML document.
pub fn apply_live_html(editor: &mut Editor, html: &str) -> Result<NodeId, AgentError> {
    let root = live_artboard(editor)
        .ok_or_else(|| AgentError::Invalid("there is no artboard to replace".into()))?;
    let slot = editor
        .doc
        .artboard_of(root)
        .cloned()
        .ok_or_else(|| AgentError::Invalid("missing artboard".into()))?;
    let new_root = editor.edit(None, |doc| {
        doc.remove_subtree_keep_artboard(root);
        let new_root = parse_document(doc, html, ImportOptions { keep_ids: true });
        doc.replace_artboard_root(root, new_root);
        if let Some(artboard) = doc.artboard_mut(new_root) {
            artboard.x = slot.x;
            artboard.y = slot.y;
        }
        Ok(new_root)
    })?;
    if editor.selection.is_empty() {
        editor.select([new_root]);
    }
    Ok(new_root)
}

/// Fresh demo editor used by tests.
#[must_use]
pub fn demo_editor() -> Editor {
    Editor::new(starter_document())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_can_build_and_restyle_a_design() {
        let mut editor = demo_editor();
        let created = execute(
            &mut editor,
            "create_artboard",
            json!({ "name": "Pricing", "width": 800, "height": 600, "html": "<h1 style=\"font-size: 40px\">Plans</h1><p>Pick one</p>" }),
        )
        .unwrap();
        let board = created["id"].as_str().unwrap().to_owned();
        let children = created["children"].as_array().unwrap().clone();
        assert_eq!(children.len(), 2);
        let heading = children[0].as_str().unwrap();
        execute(
            &mut editor,
            "update_styles",
            json!({ "node_ids": [heading], "styles": { "color": "#ff5a36", "font-size": null } }),
        )
        .unwrap();
        let html = execute(&mut editor, "get_node", json!({ "node_id": board })).unwrap();
        let html = html["html"].as_str().unwrap();
        assert!(html.contains("color: #ff5a36"), "{html}");
        assert!(!html.contains("font-size: 40px"));
        assert!(html.contains(&format!("data-id=\"{heading}\"")));

        let gpui = execute(
            &mut editor,
            "export_code",
            json!({ "node_id": heading, "format": "gpui" }),
        )
        .unwrap();
        assert!(
            gpui["code"]
                .as_str()
                .unwrap()
                .contains("text_color(rgb(0xff5a36))")
        );

        let tree = execute(&mut editor, "get_tree", json!({})).unwrap();
        assert_eq!(tree["artboards"].as_array().unwrap().len(), 3);

        execute(&mut editor, "undo", json!({})).unwrap();
        let html = execute(&mut editor, "get_node", json!({ "node_id": heading })).unwrap();
        assert!(html["html"].as_str().unwrap().contains("font-size: 40px"));
    }

    #[test]
    fn errors_are_reported_without_side_effects() {
        let mut editor = demo_editor();
        let revision = editor.revision;
        assert!(matches!(
            execute(
                &mut editor,
                "set_text",
                json!({ "node_id": "n99999", "text": "x" })
            ),
            Err(AgentError::Invalid(_))
        ));
        assert!(matches!(
            execute(&mut editor, "nope", json!({})),
            Err(AgentError::Unknown(_))
        ));
        assert!(matches!(
            execute(&mut editor, "update_styles", json!({ "node_ids": "n1" })),
            Err(AgentError::Invalid(_))
        ));
        assert_eq!(editor.revision, revision);
        for spec in COMMANDS {
            assert!((spec.schema)().is_object(), "{}", spec.name);
        }
    }

    #[test]
    fn live_document_round_trips_the_active_artboard() {
        let mut editor = demo_editor();
        let html = live_html(&editor);
        assert!(html.contains("Design in real HTML"));
        let edited = html.replace("Design in real HTML.", "Edited live.");
        let root = apply_live_html(&mut editor, &edited).unwrap();
        assert_eq!(editor.doc.pages[0].artboards[0].root, root);
        assert!(live_html(&editor).contains("Edited live."));
    }
}
