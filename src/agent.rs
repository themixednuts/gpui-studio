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
use crate::geometry::{Align, Axis};
use crate::model::html::{ImportOptions, artboard_document, parse_document};
use crate::model::{ArrowHeads, ConnectorStyle, Endpoint, NodeId, NodeKind};
use crate::presets::{COMPONENTS, starter_document};
use crate::shapes::{Stroke, freehand_svg, line_svg};

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
        name: "list_connections",
        title: "List connections",
        description: "Connectors (arrows) on the current page: id, endpoints (layer ids or points), label, style.",
        schema: || object(json!({}), &[]),
        mutating: false,
    },
    CommandSpec {
        name: "connect",
        title: "Connect layers",
        description: "Draw a connector arrow between two layers (it follows them as they move), or from/to free canvas points. Use for user flows and annotations.",
        schema: || {
            object(
                json!({
                    "from_id": { "type": "string", "description": NODE },
                    "to_id": { "type": "string", "description": NODE },
                    "from_point": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "[x, y] in canvas coordinates when from_id is omitted." },
                    "to_point": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 },
                    "label": { "type": "string" },
                    "style": { "type": "string", "enum": ["curved", "straight", "elbow"] },
                    "heads": { "type": "string", "enum": ["end", "both", "none"] },
                    "color": { "type": "string" }
                }),
                &[],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "update_connection",
        title: "Update connection",
        description: "Change a connector's label, color, style, or arrowheads.",
        schema: || {
            object(
                json!({
                    "connection_id": { "type": "integer" },
                    "label": { "type": "string" },
                    "style": { "type": "string", "enum": ["curved", "straight", "elbow"] },
                    "heads": { "type": "string", "enum": ["end", "both", "none"] },
                    "color": { "type": "string" }
                }),
                &["connection_id"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "delete_connection",
        title: "Delete connection",
        description: "Remove a connector.",
        schema: || {
            object(
                json!({ "connection_id": { "type": "integer" } }),
                &["connection_id"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "draw_vector",
        title: "Draw vector",
        description: "Draw a line, arrow, or freehand path as an inline SVG layer inside a parent, using points in the parent's coordinates.",
        schema: || {
            object(
                json!({
                    "parent_id": { "type": "string", "description": NODE },
                    "kind": { "type": "string", "enum": ["line", "arrow", "path"] },
                    "points": { "type": "array", "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 }, "minItems": 2, "maxItems": 4096 },
                    "color": { "type": "string" },
                    "width": { "type": "number", "minimum": 0.5, "maximum": 64 }
                }),
                &["parent_id", "kind", "points"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "list_project_components",
        title: "List project components",
        description: "Main components defined in this project (layers marked data-component) with their instances. Editing a main updates every instance except properties an instance overrides.",
        schema: || object(json!({}), &[]),
        mutating: false,
    },
    CommandSpec {
        name: "create_component",
        title: "Create component",
        description: "Make a layer a main component so instances of it stay in sync.",
        schema: || {
            object(
                json!({ "node_id": { "type": "string", "description": NODE }, "name": { "type": "string" } }),
                &["node_id"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "create_instance",
        title: "Create instance",
        description: "Insert an instance of a main component into parent_id (at index), or next to the main when parent_id is omitted.",
        schema: || {
            object(
                json!({
                    "component_id": { "type": "string", "description": "Id of the main component." },
                    "parent_id": { "type": "string", "description": NODE },
                    "index": { "type": "integer", "minimum": 0 }
                }),
                &["component_id"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "detach_instance",
        title: "Detach instance",
        description: "Turn an instance into ordinary layers that no longer sync.",
        schema: || {
            object(
                json!({ "node_id": { "type": "string", "description": NODE } }),
                &["node_id"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "reset_overrides",
        title: "Reset overrides",
        description: "Make an instance match its main component again (keeps its own position).",
        schema: || {
            object(
                json!({ "node_id": { "type": "string", "description": NODE } }),
                &["node_id"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "read_messages",
        title: "Read chat messages",
        description: "Messages the person wrote in Studio's chat (their prompts to you), oldest first, with the layers they had selected. By default returns only unread ones and marks them read. Pass your name once so the person sees who is working. To wait for the next message, call the gpui-mcp tool wait_for_element with query \"Unread chat messages for the agent\"; it matches while the person has unread messages for you.",
        schema: || {
            object(
                json!({
                    "since": { "type": "integer", "minimum": 0, "description": "Return messages after this id (including already-read ones)." },
                    "agent": { "type": "string", "description": "Your display name, e.g. \"Claude\"." }
                }),
                &[],
            )
        },
        mutating: false,
    },
    CommandSpec {
        name: "send_message",
        title: "Send chat message",
        description: "Reply to the person in Studio's chat: answers, questions, or progress notes. Optionally point at layers.",
        schema: || {
            object(
                json!({
                    "text": { "type": "string" },
                    "reply_to": { "type": "integer" },
                    "node_ids": { "type": "array", "items": { "type": "string" } },
                    "agent": { "type": "string" }
                }),
                &["text"],
            )
        },
        mutating: false,
    },
    CommandSpec {
        name: "set_status",
        title: "Set agent status",
        description: "Show the person what you are working on: a short status and the layers you are focused on (your own selection, drawn on the canvas in your color). Call with done=true when finished.",
        schema: || {
            object(
                json!({
                    "status": { "type": "string" },
                    "node_ids": { "type": "array", "items": { "type": "string" } },
                    "done": { "type": "boolean" },
                    "agent": { "type": "string" }
                }),
                &[],
            )
        },
        mutating: false,
    },
    CommandSpec {
        name: "annotate",
        title: "Annotate layers",
        description: "Point something out on the canvas: a labelled, colored outline on layers that follows them as the layout changes (not saved in the design). Reusing a key replaces that annotation. Prefer this over gpui-mcp highlight_elements for design feedback; use add_comment for feedback that should persist.",
        schema: || {
            object(
                json!({
                    "node_ids": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                    "label": { "type": "string" },
                    "color": { "type": "string", "description": "CSS color (default #f97316)." },
                    "key": { "type": "string", "description": "Stable key to update or clear this annotation later." },
                    "agent": { "type": "string" }
                }),
                &["node_ids", "label"],
            )
        },
        mutating: false,
    },
    CommandSpec {
        name: "clear_annotations",
        title: "Clear annotations",
        description: "Remove one annotation by key, or all annotations.",
        schema: || object(json!({ "key": { "type": "string" } }), &[]),
        mutating: false,
    },
    CommandSpec {
        name: "get_activity",
        title: "Get activity",
        description: "Recent agent edits with document revisions, plus whether the person has paused agent edits.",
        schema: || object(json!({ "since": { "type": "integer", "minimum": 0 } }), &[]),
        mutating: false,
    },
    CommandSpec {
        name: "export_image",
        title: "Export image",
        description: "Render a layer or artboard to PNG (at scale) or SVG in the project's exports/ folder, laid out exactly as on the canvas. Returns the file path; the file is written within a moment.",
        schema: || {
            object(
                json!({
                    "node_id": { "type": "string", "description": NODE },
                    "format": { "type": "string", "enum": ["png", "svg"] },
                    "scale": { "type": "number", "minimum": 0.1, "maximum": 8 }
                }),
                &["node_id"],
            )
        },
        mutating: false,
    },
    CommandSpec {
        name: "list_versions",
        title: "List versions",
        description: "The project's version history, newest first: named versions and automatic checkpoints (including one before each batch of agent edits).",
        schema: || object(json!({}), &[]),
        mutating: false,
    },
    CommandSpec {
        name: "save_version",
        title: "Save version",
        description: "Record a named version of the whole design, e.g. before a risky change or when the person approves something.",
        schema: || object(json!({ "name": { "type": "string" } }), &["name"]),
        mutating: false,
    },
    CommandSpec {
        name: "restore_version",
        title: "Restore version",
        description: "Restore the design to a version (the current state is checkpointed first, and the restore is undoable).",
        schema: || {
            object(
                json!({ "version_id": { "type": "integer" } }),
                &["version_id"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "list_links",
        title: "List prototype links",
        description: "Prototype links on the current page: which layer navigates to which artboard (or back) and with what transition.",
        schema: || object(json!({}), &[]),
        mutating: false,
    },
    CommandSpec {
        name: "set_link",
        title: "Set prototype link",
        description: "Make clicking a layer in present mode navigate to an artboard (target_id), go back (target \"back\"), or remove the link (omit target).",
        schema: || {
            object(
                json!({
                    "node_id": { "type": "string", "description": NODE },
                    "target_id": { "type": "string", "description": "Artboard id, or \"back\"." },
                    "transition": { "type": "string", "enum": ["instant", "dissolve", "slide-left", "slide-right"] }
                }),
                &["node_id"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "list_variables",
        title: "List variables",
        description: "Design variables (CSS custom properties on :root) with their values, resolved values, kind (color, number, other), and how many layers use each. Use them in styles as var(--name).",
        schema: || object(json!({}), &[]),
        mutating: false,
    },
    CommandSpec {
        name: "set_variable",
        title: "Set variable",
        description: "Create or update a design variable. Layers styled with var(--name) update everywhere.",
        schema: || {
            object(
                json!({
                    "name": { "type": "string", "description": "Name such as \"brand\" or \"space-4\" (a leading -- is optional)." },
                    "value": { "type": "string", "description": "Any CSS value, e.g. #ff5a36, 16px, or var(--other)." }
                }),
                &["name", "value"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "rename_variable",
        title: "Rename variable",
        description: "Rename a design variable and every var() reference to it.",
        schema: || {
            object(
                json!({ "name": { "type": "string" }, "new_name": { "type": "string" } }),
                &["name", "new_name"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "delete_variable",
        title: "Delete variable",
        description: "Delete a design variable; layers that used it keep its value inline.",
        schema: || object(json!({ "name": { "type": "string" } }), &["name"]),
        mutating: true,
    },
    CommandSpec {
        name: "import_image",
        title: "Import image",
        description: "Copy a local image file (png, jpg, gif, webp, svg, bmp) into the project's artboards/assets/ and place it as an <img> layer: inside parent_id (absolutely at x/y in block flow, or at index in flex/grid), or as a new artboard at canvas x/y when parent_id is omitted.",
        schema: || {
            object(
                json!({
                    "path": { "type": "string", "description": "Absolute path of the image file." },
                    "parent_id": { "type": "string", "description": NODE },
                    "x": { "type": "number" },
                    "y": { "type": "number" },
                    "index": { "type": "integer", "minimum": 0 }
                }),
                &["path"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "align_nodes",
        title: "Align nodes",
        description: "Align artboards or absolutely positioned layers to each other (or one layer to its parent) using their rendered bounds.",
        schema: || {
            object(
                json!({
                    "node_ids": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                    "align": { "type": "string", "enum": ["left", "center", "right", "top", "middle", "bottom"] }
                }),
                &["node_ids", "align"],
            )
        },
        mutating: true,
    },
    CommandSpec {
        name: "distribute_nodes",
        title: "Distribute nodes",
        description: "Space three or more artboards or absolutely positioned layers evenly along an axis.",
        schema: || {
            object(
                json!({
                    "node_ids": { "type": "array", "items": { "type": "string" }, "minItems": 3 },
                    "axis": { "type": "string", "enum": ["horizontal", "vertical"] }
                }),
                &["node_ids", "axis"],
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
                "connections": page.connections.len(),
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

fn parse_style(value: Option<&str>) -> Result<Option<ConnectorStyle>, AgentError> {
    value
        .map(|v| {
            ConnectorStyle::parse(v)
                .ok_or_else(|| AgentError::Invalid(format!("unknown style {v:?}")))
        })
        .transpose()
}

fn parse_heads(value: Option<&str>) -> Result<Option<ArrowHeads>, AgentError> {
    value
        .map(|v| {
            ArrowHeads::parse(v).ok_or_else(|| AgentError::Invalid(format!("unknown heads {v:?}")))
        })
        .transpose()
}

fn endpoint_json(editor: &Editor, endpoint: Endpoint) -> Value {
    match endpoint {
        Endpoint::Node(id) => {
            json!({ "node_id": id.to_string(), "name": editor.doc.display_name(id) })
        }
        Endpoint::Point { x, y } => json!({ "point": [x, y] }),
    }
}

/// Connectors on the current page.
#[must_use]
pub fn connections_json(editor: &Editor) -> Value {
    let Some(page) = editor.doc.pages.get(editor.page) else {
        return json!([]);
    };
    json!(
        page.connections
            .iter()
            .map(|c| json!({
                "id": c.id,
                "from": endpoint_json(editor, c.from),
                "to": endpoint_json(editor, c.to),
                "label": c.label,
                "color": c.color,
                "style": c.style,
                "heads": c.heads,
            }))
            .collect::<Vec<_>>()
    )
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

fn message_json(editor: &Editor, m: &crate::collab::Message) -> Value {
    json!({
        "id": m.id,
        "from": match &m.author { crate::collab::Actor::Person => "person".to_owned(), crate::collab::Actor::Agent { name } => name.clone() },
        "text": m.text,
        "reply_to": m.reply_to,
        "layers": m.nodes.iter().filter(|id| editor.doc.contains(**id)).map(|id| json!({
            "id": id.to_string(),
            "name": editor.doc.display_name(*id),
        })).collect::<Vec<_>>(),
    })
}

/// The chat as JSON (for the chat resource).
#[must_use]
pub fn chat_json(editor: &Editor) -> Value {
    json!({
        "unread": editor.collab.unread_for_agent(),
        "paused": editor.collab.paused,
        "messages": editor.collab.messages().iter().map(|m| message_json(editor, m)).collect::<Vec<_>>(),
    })
}

/// Every design variable with its resolved value, kind, and usage count.
#[must_use]
pub fn variables_json(editor: &Editor) -> Value {
    use crate::model::variables::{VariableKind, kind_of, resolve};
    let vars = &editor.doc.variables;
    Value::Array(
        vars.iter()
            .map(|(name, value)| {
                json!({
                    "name": name,
                    "css": format!("var(--{name})"),
                    "value": value,
                    "resolved": resolve(value, vars),
                    "kind": match kind_of(value, vars) {
                        VariableKind::Color => "color",
                        VariableKind::Number => "number",
                        VariableKind::Other => "other",
                    },
                    "used_by": editor.doc.variable_usage(name),
                })
            })
            .collect(),
    )
}

/// Rendered bounds in document pixels, when the canvas has laid the node out.
fn bounds_json(editor: &Editor, id: NodeId) -> Value {
    editor.measured.get(&id).map_or(
        Value::Null,
        |r| json!({ "x": r.x, "y": r.y, "width": r.w, "height": r.h }),
    )
}

/// Execute one command as the connected agent.
///
/// Agents are collaborators: while the person has paused them, design edits
/// are refused; the agent's selection becomes its own presence instead of
/// replacing the person's (unless the person follows the agent); and every
/// edit lands in the activity feed.
pub fn execute(editor: &mut Editor, name: &str, arguments: Value) -> Result<Value, AgentError> {
    let mutating = COMMANDS.iter().any(|c| c.name == name && c.mutating);
    if mutating && editor.collab.paused {
        return Err(AgentError::Invalid(
            "The person paused agent edits. Ask them in chat with send_message, then try again later.".into(),
        ));
    }
    if mutating {
        let agent = editor.collab.agent_name().to_owned();
        editor.checkpoint_before_agent(&agent);
    }
    let person = editor.selection.clone();
    let page = editor.page;
    let revision = editor.revision;
    let mentioned: Vec<NodeId> = ["node_id", "parent_id", "component_id"]
        .iter()
        .filter_map(|key| arguments.get(*key).and_then(Value::as_str))
        .chain(
            arguments
                .get("node_ids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        )
        .filter_map(NodeId::parse)
        .collect();
    let result = execute_command(editor, name, arguments);
    if editor.selection != person {
        let agent = editor.collab.agent_name().to_owned();
        let selection = editor.selection.clone();
        editor.collab.set_presence(&agent, Some(selection), None);
        if !editor.collab.follow {
            editor.selection = person;
            editor.selection.retain(|id| editor.doc.contains(*id));
            editor.page = page.min(editor.doc.pages.len().saturating_sub(1));
        }
    }
    if result.is_ok() && editor.revision != revision {
        let nodes: Vec<NodeId> = if mentioned.is_empty() {
            editor
                .collab
                .presence()
                .get(editor.collab.agent_name())
                .map(|p| p.nodes.clone())
                .unwrap_or_default()
        } else {
            mentioned
        };
        let names: Vec<String> = nodes
            .iter()
            .filter(|id| editor.doc.contains(**id))
            .take(3)
            .map(|id| editor.doc.display_name(*id))
            .collect();
        let summary = if names.is_empty() {
            name.replace('_', " ")
        } else {
            format!("{} · {}", name.replace('_', " "), names.join(", "))
        };
        let actor = editor.collab.agent_actor();
        editor.collab.log(actor, summary, nodes, editor.revision);
    }
    result
}

fn execute_command(editor: &mut Editor, name: &str, arguments: Value) -> Result<Value, AgentError> {
    match name {
        "get_document" => Ok(document_json(editor)),
        "get_selection" => Ok(selection_json(editor)),
        "get_node" => {
            let a: NodeArg = args(arguments)?;
            let id = node(editor, &a.node_id)?;
            Ok(
                json!({ "id": id.to_string(), "html": editor.html_of(id, true), "bounds": bounds_json(editor, id) }),
            )
        }
        "list_project_components" => {
            let doc = &editor.doc;
            Ok(Value::Array(
                doc.components()
                    .into_iter()
                    .map(|main| {
                        json!({
                            "id": main.to_string(),
                            "name": doc.component_name(main),
                            "artboard": doc.artboard_of(main).map(|a| a.root.to_string()),
                            "instances": doc.instances_of(main).iter().map(|i| json!({
                                "id": i.to_string(),
                                "overridden": doc.has_overrides(*i),
                            })).collect::<Vec<_>>(),
                        })
                    })
                    .collect(),
            ))
        }
        "create_component" => {
            #[derive(Deserialize)]
            struct A {
                node_id: String,
                name: Option<String>,
            }
            let a: A = args(arguments)?;
            let id = node(editor, &a.node_id)?;
            let name = editor.create_component(id, a.name.as_deref())?;
            Ok(json!({ "id": id.to_string(), "name": name, "revision": editor.revision }))
        }
        "create_instance" => {
            #[derive(Deserialize)]
            struct A {
                component_id: String,
                parent_id: Option<String>,
                index: Option<usize>,
            }
            let a: A = args(arguments)?;
            let main = node(editor, &a.component_id)?;
            let parent = a.parent_id.map(|id| node(editor, &id)).transpose()?;
            let id = editor.create_instance(
                main,
                InsertTarget {
                    parent,
                    index: a.index,
                },
            )?;
            Ok(
                json!({ "id": id.to_string(), "html": editor.html_of(id, true), "revision": editor.revision }),
            )
        }
        "detach_instance" | "reset_overrides" => {
            let a: NodeArg = args(arguments)?;
            let id = node(editor, &a.node_id)?;
            if name == "detach_instance" {
                editor.detach_instance(id)?;
            } else {
                editor.reset_overrides(id)?;
            }
            Ok(json!({ "id": id.to_string(), "revision": editor.revision }))
        }
        "read_messages" => {
            #[derive(Deserialize)]
            struct A {
                since: Option<u64>,
                agent: Option<String>,
            }
            let a: A = args(arguments)?;
            editor.collab.introduce(a.agent.as_deref());
            let messages = editor.collab.read_for_agent(a.since);
            Ok(json!({
                "messages": messages.iter().map(|m| message_json(editor, m)).collect::<Vec<_>>(),
                "paused": editor.collab.paused,
            }))
        }
        "send_message" => {
            #[derive(Deserialize)]
            struct A {
                text: String,
                reply_to: Option<u64>,
                node_ids: Option<Vec<String>>,
                agent: Option<String>,
            }
            let a: A = args(arguments)?;
            editor.collab.introduce(a.agent.as_deref());
            let nodes = nodes(editor, &a.node_ids.unwrap_or_default())?;
            let actor = editor.collab.agent_actor();
            let id = editor
                .collab
                .post(actor, &a.text, nodes, a.reply_to)
                .ok_or_else(|| AgentError::Invalid("text is empty".into()))?;
            Ok(json!({ "message_id": id }))
        }
        "set_status" => {
            #[derive(Deserialize)]
            struct A {
                status: Option<String>,
                node_ids: Option<Vec<String>>,
                done: Option<bool>,
                agent: Option<String>,
            }
            let a: A = args(arguments)?;
            editor.collab.introduce(a.agent.as_deref());
            let agent = editor.collab.agent_name().to_owned();
            if a.done == Some(true) {
                editor.collab.clear_presence(&agent);
            } else {
                let ids = a.node_ids.map(|ids| nodes(editor, &ids)).transpose()?;
                editor.collab.set_presence(&agent, ids, a.status);
            }
            Ok(json!({ "agent": agent }))
        }
        "annotate" => {
            #[derive(Deserialize)]
            struct A {
                node_ids: Vec<String>,
                label: String,
                color: Option<String>,
                key: Option<String>,
                agent: Option<String>,
            }
            let a: A = args(arguments)?;
            editor.collab.introduce(a.agent.as_deref());
            let ids = nodes(editor, &a.node_ids)?;
            if ids.is_empty() {
                return Err(AgentError::Invalid("node_ids is empty".into()));
            }
            let color = a.color.unwrap_or_else(|| "#f97316".to_owned());
            if crate::model::Color::parse_loose(&color).is_none() {
                return Err(AgentError::Invalid(format!("invalid color {color:?}")));
            }
            let key = a
                .key
                .unwrap_or_else(|| format!("a{}", editor.collab.annotations().len() + 1));
            editor.collab.annotate(crate::collab::Annotation {
                key: key.clone(),
                nodes: ids,
                label: a
                    .label
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(120)
                    .collect(),
                color,
                agent: editor.collab.agent_name().to_owned(),
            });
            Ok(json!({ "key": key }))
        }
        "clear_annotations" => {
            #[derive(Deserialize)]
            struct A {
                key: Option<String>,
            }
            let a: A = args(arguments)?;
            Ok(json!({ "removed": editor.collab.clear_annotations(a.key.as_deref()) }))
        }
        "get_activity" => {
            #[derive(Deserialize)]
            struct A {
                since: Option<u64>,
            }
            let a: A = args(arguments)?;
            let since = a.since.unwrap_or(0);
            Ok(json!({
                "paused": editor.collab.paused,
                "activity": editor.collab.activity().iter().filter(|e| e.id > since).map(|e| json!({
                    "id": e.id,
                    "actor": e.actor.name(),
                    "summary": e.summary,
                    "node_ids": ids_json(&e.nodes),
                    "revision": e.revision,
                })).collect::<Vec<_>>(),
            }))
        }
        "export_image" => Err(AgentError::Invalid(
            "export_image needs the Studio window to lay the layer out".into(),
        )),
        "list_versions" => Ok(Value::Array(
            editor
                .versions
                .as_ref()
                .map(|v| v.list().to_vec())
                .unwrap_or_default()
                .into_iter()
                .map(|v| json!({ "id": v.id, "name": v.name, "author": v.author, "auto": v.auto }))
                .collect(),
        )),
        "save_version" => {
            #[derive(Deserialize)]
            struct A {
                name: String,
            }
            let a: A = args(arguments)?;
            let agent = editor.collab.agent_name().to_owned();
            let saved = editor
                .checkpoint(&a.name, &agent, false)?
                .ok_or_else(|| AgentError::Invalid("versions need a saved project".into()))?;
            Ok(json!({ "id": saved.id, "name": saved.name }))
        }
        "restore_version" => {
            #[derive(Deserialize)]
            struct A {
                version_id: u64,
            }
            let a: A = args(arguments)?;
            editor.restore_version(a.version_id)?;
            Ok(json!({ "revision": editor.revision }))
        }
        "list_links" => {
            use crate::model::prototype::LinkTarget;
            let doc = &editor.doc;
            Ok(Value::Array(
                doc.page_links(editor.page)
                    .into_iter()
                    .map(|l| {
                        json!({
                            "node_id": l.source.to_string(),
                            "node_name": doc.display_name(l.source),
                            "target": match l.target {
                                LinkTarget::Back => json!("back"),
                                LinkTarget::Artboard(id) => json!({ "id": id.to_string(), "name": doc.display_name(id) }),
                            },
                            "transition": l.transition.as_str(),
                        })
                    })
                    .collect(),
            ))
        }
        "set_link" => {
            use crate::model::prototype::{LinkTarget, Transition};
            #[derive(Deserialize)]
            struct A {
                node_id: String,
                target_id: Option<String>,
                transition: Option<String>,
            }
            let a: A = args(arguments)?;
            let id = node(editor, &a.node_id)?;
            let target = match a.target_id.as_deref() {
                None => None,
                Some("back") => Some(LinkTarget::Back),
                Some(target) => Some(LinkTarget::Artboard(node(editor, target)?)),
            };
            let transition = match a.transition.as_deref() {
                None => Transition::Instant,
                Some(t) => Transition::parse(t)
                    .ok_or_else(|| AgentError::Invalid(format!("unknown transition {t:?}")))?,
            };
            editor.set_link(id, target, transition)?;
            Ok(json!({ "id": id.to_string(), "revision": editor.revision }))
        }
        "list_variables" => Ok(variables_json(editor)),
        "set_variable" => {
            #[derive(Deserialize)]
            struct A {
                name: String,
                value: String,
            }
            let a: A = args(arguments)?;
            let name = editor.set_variable(&a.name, &a.value)?;
            Ok(json!({ "name": name, "revision": editor.revision }))
        }
        "rename_variable" => {
            #[derive(Deserialize)]
            struct A {
                name: String,
                new_name: String,
            }
            let a: A = args(arguments)?;
            let name = editor.rename_variable(a.name.trim_start_matches("--"), &a.new_name)?;
            Ok(json!({ "name": name, "revision": editor.revision }))
        }
        "delete_variable" => {
            #[derive(Deserialize)]
            struct A {
                name: String,
            }
            let a: A = args(arguments)?;
            let detached = editor.delete_variable(a.name.trim_start_matches("--"))?;
            Ok(json!({ "detached_layers": detached, "revision": editor.revision }))
        }
        "import_image" => {
            #[derive(Deserialize)]
            struct A {
                path: String,
                parent_id: Option<String>,
                x: Option<f32>,
                y: Option<f32>,
                index: Option<usize>,
            }
            let a: A = args(arguments)?;
            let path = std::path::Path::new(&a.path);
            if !crate::assets::is_image_path(path) {
                return Err(AgentError::Invalid(
                    "path must be a png, jpg, gif, webp, svg, or bmp file".into(),
                ));
            }
            let size = std::fs::metadata(path)
                .map_err(|e| AgentError::Invalid(format!("cannot read {}: {e}", a.path)))?
                .len();
            if size > crate::assets::MAX_IMAGE_BYTES as u64 {
                return Err(AgentError::Invalid(
                    "images larger than 32 MB are not imported".into(),
                ));
            }
            let bytes = std::fs::read(path)
                .map_err(|e| AgentError::Invalid(format!("cannot read {}: {e}", a.path)))?;
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("image.png")
                .to_owned();
            let parent = a.parent_id.map(|id| node(editor, &id)).transpose()?;
            let at = match (a.x, a.y) {
                (Some(x), Some(y)) => Some((x, y)),
                _ => None,
            };
            let id = editor.import_image(
                &bytes,
                &name,
                crate::editor::ImagePlacement {
                    parent,
                    at,
                    index: a.index,
                },
            )?;
            Ok(
                json!({ "id": id.to_string(), "html": editor.html_of(id, true), "revision": editor.revision }),
            )
        }
        "align_nodes" => {
            #[derive(Deserialize)]
            struct A {
                node_ids: Vec<String>,
                align: String,
            }
            let a: A = args(arguments)?;
            let ids = nodes(editor, &a.node_ids)?;
            let align = Align::parse(&a.align)
                .ok_or_else(|| AgentError::Invalid(format!("unknown align {:?}", a.align)))?;
            let moved = editor.align(&ids, align)?;
            Ok(json!({ "moved": moved, "revision": editor.revision }))
        }
        "distribute_nodes" => {
            #[derive(Deserialize)]
            struct A {
                node_ids: Vec<String>,
                axis: String,
            }
            let a: A = args(arguments)?;
            let ids = nodes(editor, &a.node_ids)?;
            let axis = Axis::parse(&a.axis)
                .ok_or_else(|| AgentError::Invalid(format!("unknown axis {:?}", a.axis)))?;
            let moved = editor.distribute(&ids, axis)?;
            Ok(json!({ "moved": moved, "revision": editor.revision }))
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
        "list_connections" => Ok(connections_json(editor)),
        "connect" => {
            #[derive(Deserialize)]
            struct A {
                from_id: Option<String>,
                to_id: Option<String>,
                from_point: Option<[f32; 2]>,
                to_point: Option<[f32; 2]>,
                label: Option<String>,
                style: Option<String>,
                heads: Option<String>,
                color: Option<String>,
            }
            let a: A = args(arguments)?;
            let endpoint = |id: Option<String>,
                            point: Option<[f32; 2]>,
                            which: &str|
             -> Result<Endpoint, AgentError> {
                match (id, point) {
                    (Some(id), _) => Ok(Endpoint::Node(node(editor, &id)?)),
                    (None, Some([x, y])) => Ok(Endpoint::Point { x, y }),
                    (None, None) => Err(AgentError::Invalid(format!(
                        "{which}_id or {which}_point is required"
                    ))),
                }
            };
            let from = endpoint(a.from_id, a.from_point, "from")?;
            let to = endpoint(a.to_id, a.to_point, "to")?;
            let style = parse_style(a.style.as_deref())?;
            let heads = parse_heads(a.heads.as_deref())?;
            let id = editor.connect(from, to)?;
            if a.label.is_some() || a.color.is_some() || style.is_some() || heads.is_some() {
                editor.history.seal();
                if let Err(error) = editor.update_connection(
                    id,
                    a.label.as_deref(),
                    a.color.as_deref(),
                    style,
                    heads,
                    Some("connect"),
                ) {
                    editor.undo();
                    return Err(error.into());
                }
            }
            Ok(json!({ "connection_id": id, "revision": editor.revision }))
        }
        "update_connection" => {
            #[derive(Deserialize)]
            struct A {
                connection_id: u64,
                label: Option<String>,
                style: Option<String>,
                heads: Option<String>,
                color: Option<String>,
            }
            let a: A = args(arguments)?;
            let style = parse_style(a.style.as_deref())?;
            let heads = parse_heads(a.heads.as_deref())?;
            editor.update_connection(
                a.connection_id,
                a.label.as_deref(),
                a.color.as_deref(),
                style,
                heads,
                None,
            )?;
            Ok(json!({ "revision": editor.revision }))
        }
        "delete_connection" => {
            #[derive(Deserialize)]
            struct A {
                connection_id: u64,
            }
            let a: A = args(arguments)?;
            editor.delete_connection(a.connection_id)?;
            Ok(json!({ "revision": editor.revision }))
        }
        "draw_vector" => {
            #[derive(Deserialize)]
            struct A {
                parent_id: String,
                kind: String,
                points: Vec<[f32; 2]>,
                color: Option<String>,
                width: Option<f32>,
            }
            let a: A = args(arguments)?;
            let parent = node(editor, &a.parent_id)?;
            if a.points.len() < 2 || a.points.len() > 4096 {
                return Err(AgentError::Invalid(
                    "points must have 2..=4096 entries".into(),
                ));
            }
            let mut stroke = Stroke::default();
            if let Some(color) = a.color {
                stroke.color = crate::model::Color::parse_loose(&color)
                    .ok_or_else(|| AgentError::Invalid(format!("invalid color {color:?}")))?
                    .to_css();
            }
            if let Some(width) = a.width {
                stroke.width = width.clamp(0.5, 64.0);
            }
            let points: Vec<(f32, f32)> = a.points.iter().map(|[x, y]| (*x, *y)).collect();
            let svg = match a.kind.as_str() {
                "line" | "arrow" => line_svg(
                    points[0],
                    points[points.len() - 1],
                    &stroke,
                    a.kind == "arrow",
                ),
                "path" => freehand_svg(&points, &stroke)
                    .ok_or_else(|| AgentError::Invalid("path needs distinct points".into()))?,
                other => return Err(AgentError::Invalid(format!("unknown kind {other:?}"))),
            };
            let id = editor.insert_vector(parent, &svg)?;
            Ok(json!({ "id": id.to_string(), "revision": editor.revision }))
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
    (
        "gpui-studio://chat",
        "chat",
        "The person's chat with you: their prompts and your replies (read_messages marks them read).",
    ),
    (
        "gpui-studio://variables",
        "variables",
        "Design variables (CSS custom properties) to style with var(--name).",
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
        "gpui-studio://variables" => variables_json(editor),
        "gpui-studio://chat" => chat_json(editor),
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
    fn agent_can_connect_layers_and_draw_vectors() {
        let mut editor = demo_editor();
        let doc = execute(&mut editor, "get_document", json!({})).unwrap();
        let desktop = doc["pages"][0]["artboards"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let mobile = doc["pages"][0]["artboards"][1]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let made = execute(
            &mut editor,
            "connect",
            json!({ "from_id": desktop, "to_id": mobile, "label": "Responsive", "style": "elbow" }),
        )
        .unwrap();
        let id = made["connection_id"].as_u64().unwrap();
        let list = execute(&mut editor, "list_connections", json!({})).unwrap();
        assert_eq!(list[0]["label"], "Responsive");
        assert_eq!(list[0]["style"], "elbow");
        assert_eq!(list[0]["to"]["node_id"], mobile);
        assert!(execute(&mut editor, "connect", json!({ "from_id": desktop })).is_err());
        assert!(
            execute(
                &mut editor,
                "connect",
                json!({ "from_id": desktop, "to_point": [0, 0], "style": "wiggly" })
            )
            .is_err()
        );
        assert_eq!(
            execute(&mut editor, "list_connections", json!({}))
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1,
            "a rejected connect leaves nothing behind"
        );
        execute(
            &mut editor,
            "update_connection",
            json!({ "connection_id": id, "heads": "both" }),
        )
        .unwrap();
        let drawn = execute(
            &mut editor,
            "draw_vector",
            json!({ "parent_id": desktop, "kind": "arrow", "points": [[40, 40], [200, 120]], "color": "#ff5a36", "width": 3 }),
        )
        .unwrap();
        let html = execute(&mut editor, "get_node", json!({ "node_id": drawn["id"] })).unwrap();
        assert!(
            html["html"]
                .as_str()
                .unwrap()
                .contains("stroke=\"#ff5a36\"")
        );
        execute(
            &mut editor,
            "delete_connection",
            json!({ "connection_id": id }),
        )
        .unwrap();
        assert!(
            execute(&mut editor, "list_connections", json!({}))
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
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

    #[test]
    fn agent_aligns_and_distributes_with_measured_bounds() {
        let mut editor = demo_editor();
        let board = editor.doc.pages[0].artboards[0].root;
        let ids = editor
            .insert_html(
                "<div style=\"position: absolute; left: 10px; top: 0px; width: 20px; height: 20px\"></div>\
                 <div style=\"position: absolute; left: 50px; top: 30px; width: 40px; height: 10px\"></div>\
                 <div style=\"position: absolute; left: 200px; top: 5px; width: 20px; height: 20px\"></div>",
                InsertTarget {
                    parent: Some(board),
                    index: None,
                },
            )
            .unwrap();
        // Without a rendered layout there is nothing to align against.
        let names: Vec<String> = ids.iter().map(ToString::to_string).collect();
        assert!(
            execute(
                &mut editor,
                "align_nodes",
                json!({ "node_ids": names, "align": "top" })
            )
            .is_err()
        );
        let rects = [
            (10.0, 0.0, 20.0, 20.0),
            (50.0, 30.0, 40.0, 10.0),
            (200.0, 5.0, 20.0, 20.0),
        ];
        editor
            .measured
            .insert(board, crate::geometry::Rect::new(0.0, 0.0, 1440.0, 900.0));
        for (id, (x, y, w, h)) in ids.iter().zip(rects) {
            editor
                .measured
                .insert(*id, crate::geometry::Rect::new(x, y, w, h));
        }
        let out = execute(
            &mut editor,
            "align_nodes",
            json!({ "node_ids": names, "align": "bottom" }),
        )
        .unwrap();
        assert_eq!(out["moved"], 2);
        let top = |editor: &Editor, i: usize| {
            editor
                .doc
                .get(ids[i])
                .unwrap()
                .style
                .get("top")
                .map(ToOwned::to_owned)
        };
        assert_eq!(top(&editor, 0).as_deref(), Some("20px"));
        assert_eq!(top(&editor, 1).as_deref(), Some("30px"));
        execute(
            &mut editor,
            "distribute_nodes",
            json!({ "node_ids": names, "axis": "horizontal" }),
        )
        .unwrap();
        assert_eq!(
            editor.doc.get(ids[1]).unwrap().style.get("left"),
            Some("95px")
        );
        assert!(
            execute(
                &mut editor,
                "distribute_nodes",
                json!({ "node_ids": names, "axis": "diagonal" })
            )
            .is_err()
        );
        let node = execute(&mut editor, "get_node", json!({ "node_id": names[2] })).unwrap();
        assert_eq!(node["bounds"]["width"], 20.0);
    }

    #[test]
    fn agent_manages_variables() {
        let mut editor = demo_editor();
        execute(
            &mut editor,
            "set_variable",
            json!({ "name": "--brand", "value": "#ff5a36" }),
        )
        .unwrap();
        execute(
            &mut editor,
            "set_variable",
            json!({ "name": "accent", "value": "var(--brand)" }),
        )
        .unwrap();
        let board = editor.doc.pages[0].artboards[0].root.to_string();
        execute(
            &mut editor,
            "update_styles",
            json!({ "node_ids": [board], "styles": { "background-color": "var(--accent)" } }),
        )
        .unwrap();
        let list = execute(&mut editor, "list_variables", json!({})).unwrap();
        let accent = list
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == "accent")
            .unwrap();
        assert_eq!(accent["resolved"], "#ff5a36");
        assert_eq!(accent["kind"], "color");
        assert_eq!(accent["used_by"], 1);
        execute(
            &mut editor,
            "rename_variable",
            json!({ "name": "brand", "new_name": "primary" }),
        )
        .unwrap();
        assert_eq!(
            editor.doc.variables.get("accent").map(String::as_str),
            Some("var(--primary)")
        );
        execute(&mut editor, "delete_variable", json!({ "name": "accent" })).unwrap();
        let html = execute(
            &mut editor,
            "export_code",
            json!({ "node_id": board, "format": "html" }),
        )
        .unwrap();
        let code = html["code"].as_str().unwrap();
        assert!(
            code.contains("var(--primary)") && code.contains("--primary: #ff5a36;"),
            "{code}"
        );
        assert!(
            read_resource(&editor, "gpui-studio://variables")
                .unwrap()
                .contains("primary")
        );
        assert!(execute(&mut editor, "delete_variable", json!({ "name": "nope" })).is_err());
    }

    #[test]
    fn agent_builds_and_syncs_components() {
        let mut editor = demo_editor();
        let board = editor.doc.pages[0].artboards[0].root;
        let made = execute(
            &mut editor,
            "write_html",
            json!({ "html": "<div data-name=\"Tag\" style=\"display: flex; padding: 4px; background-color: #eee\"><span>New</span></div>", "parent_id": board.to_string() }),
        )
        .unwrap();
        let tag = made["ids"][0].as_str().unwrap().to_owned();
        execute(&mut editor, "create_component", json!({ "node_id": tag })).unwrap();
        let instance = execute(
            &mut editor,
            "create_instance",
            json!({ "component_id": tag }),
        )
        .unwrap();
        let instance = instance["id"].as_str().unwrap().to_owned();
        execute(
            &mut editor,
            "update_styles",
            json!({ "node_ids": [tag], "styles": { "background-color": "#0d99ff" } }),
        )
        .unwrap();
        let list = execute(&mut editor, "list_project_components", json!({})).unwrap();
        assert_eq!(list[0]["name"], "Tag");
        assert_eq!(list[0]["instances"][0]["id"], instance.as_str());
        assert_eq!(list[0]["instances"][0]["overridden"], false);
        let html = execute(&mut editor, "get_node", json!({ "node_id": instance })).unwrap();
        assert!(html["html"].as_str().unwrap().contains("#0d99ff"));
        execute(
            &mut editor,
            "update_styles",
            json!({ "node_ids": [instance], "styles": { "padding": "9px" } }),
        )
        .unwrap();
        let list = execute(&mut editor, "list_project_components", json!({})).unwrap();
        assert_eq!(list[0]["instances"][0]["overridden"], true);
        execute(
            &mut editor,
            "reset_overrides",
            json!({ "node_id": instance }),
        )
        .unwrap();
        execute(
            &mut editor,
            "detach_instance",
            json!({ "node_id": instance }),
        )
        .unwrap();
        assert!(
            execute(
                &mut editor,
                "detach_instance",
                json!({ "node_id": instance })
            )
            .is_err()
        );
    }

    #[test]
    fn agent_links_screens_for_prototypes() {
        let mut editor = demo_editor();
        let desktop = editor.doc.pages[0].artboards[0].root;
        let mobile = editor.doc.pages[0].artboards[1].root.to_string();
        let button = editor
            .doc
            .descendants(desktop)
            .into_iter()
            .find(|id| editor.doc.get(*id).is_some_and(|n| n.tag() == "button"))
            .unwrap()
            .to_string();
        execute(
            &mut editor,
            "set_link",
            json!({ "node_id": button, "target_id": mobile, "transition": "slide-left" }),
        )
        .unwrap();
        let links = execute(&mut editor, "list_links", json!({})).unwrap();
        assert_eq!(links[0]["target"]["id"], mobile.as_str());
        assert_eq!(links[0]["transition"], "slide-left");
        let code = execute(
            &mut editor,
            "export_code",
            json!({ "node_id": button, "format": "html" }),
        )
        .unwrap();
        assert!(!code["code"].as_str().unwrap().contains("data-link"));
        assert!(
            execute(
                &mut editor,
                "set_link",
                json!({ "node_id": button, "target_id": button })
            )
            .is_err()
        );
        execute(&mut editor, "set_link", json!({ "node_id": button })).unwrap();
        assert!(
            execute(&mut editor, "list_links", json!({}))
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn agents_collaborate_without_taking_the_persons_selection() {
        let mut editor = demo_editor();
        let desktop = editor.doc.pages[0].artboards[0].root;
        let mobile = editor.doc.pages[0].artboards[1].root;
        editor.select([mobile]);
        editor.collab.post(
            crate::collab::Actor::Person,
            "Make the desktop artboard warmer",
            vec![desktop],
            None,
        );

        let inbox = execute(&mut editor, "read_messages", json!({ "agent": "Claude" })).unwrap();
        assert_eq!(
            inbox["messages"][0]["text"],
            "Make the desktop artboard warmer"
        );
        assert_eq!(inbox["messages"][0]["layers"][0]["id"], desktop.to_string());
        execute(
            &mut editor,
            "set_status",
            json!({ "status": "Warming colors", "node_ids": [desktop.to_string()] }),
        )
        .unwrap();
        execute(
            &mut editor,
            "select_nodes",
            json!({ "node_ids": [desktop.to_string()] }),
        )
        .unwrap();
        execute(
            &mut editor,
            "update_styles",
            json!({ "node_ids": [desktop.to_string()], "styles": { "background-color": "#fff4ec" } }),
        )
        .unwrap();
        // The person's selection is untouched; the agent's is its presence.
        assert_eq!(editor.selection, vec![mobile]);
        let presence = &editor.collab.presence()["Claude"];
        assert_eq!(presence.nodes, vec![desktop]);
        assert_eq!(presence.status, "Warming colors");
        let activity = execute(&mut editor, "get_activity", json!({})).unwrap();
        assert_eq!(activity["activity"][0]["actor"], "Claude");
        assert!(
            activity["activity"][0]["summary"]
                .as_str()
                .unwrap()
                .starts_with("update styles")
        );
        execute(
            &mut editor,
            "send_message",
            json!({ "text": "Done — warmer background." }),
        )
        .unwrap();
        assert_eq!(
            editor.collab.messages().last().unwrap().author.name(),
            "Claude"
        );
        assert!(
            read_resource(&editor, "gpui-studio://chat")
                .unwrap()
                .contains("warmer background")
        );

        // Paused: edits are refused, chat still works.
        editor.collab.paused = true;
        assert!(
            execute(
                &mut editor,
                "update_styles",
                json!({ "node_ids": [desktop.to_string()], "styles": { "opacity": "0.5" } })
            )
            .is_err()
        );
        assert!(
            execute(
                &mut editor,
                "send_message",
                json!({ "text": "Waiting for you" })
            )
            .is_ok()
        );
        editor.collab.paused = false;

        // Annotations follow layers and can be cleared by key.
        execute(&mut editor, "annotate", json!({ "node_ids": [desktop.to_string()], "label": "Low contrast", "key": "contrast" })).unwrap();
        execute(
            &mut editor,
            "annotate",
            json!({ "node_ids": [mobile.to_string()], "label": "Too tight", "color": "#e11d48" }),
        )
        .unwrap();
        assert!(
            execute(
                &mut editor,
                "annotate",
                json!({ "node_ids": [desktop.to_string()], "label": "x", "color": "nope" })
            )
            .is_err()
        );
        assert_eq!(editor.collab.annotations().len(), 2);
        execute(
            &mut editor,
            "clear_annotations",
            json!({ "key": "contrast" }),
        )
        .unwrap();
        assert_eq!(editor.collab.annotations()[0].label, "Too tight");
        execute(&mut editor, "clear_annotations", json!({})).unwrap();
        assert!(editor.collab.annotations().is_empty());

        // Following: the agent's selection becomes the person's.
        editor.collab.follow = true;
        execute(
            &mut editor,
            "select_nodes",
            json!({ "node_ids": [desktop.to_string()] }),
        )
        .unwrap();
        assert_eq!(editor.selection, vec![desktop]);
        execute(&mut editor, "set_status", json!({ "done": true })).unwrap();
        assert!(editor.collab.presence().is_empty());
    }
}
