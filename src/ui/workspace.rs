//! The window: project tabs, Home, the MCP bridge, and session persistence.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable as _, Sizable as _, Theme, ThemeMode, TitleBar,
    WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, PathPromptOptions, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::*, px,
};
use gpui_mcp::{
    AppId, ApplicationCommandDescriptor, ApplicationCommandRequest, ApplicationCommandResponse,
    ApplicationCommandResult, BridgeConfig, BridgeError, BridgeHandle, ContextResource,
    ContextResourceDescriptor, ContextResourceRequest, ContextResourceResponse, ErrorCode,
    LiveDocument, LiveDocumentPreview, LiveDocumentRequest, LiveDocumentResponse,
    LiveDocumentSource,
};
use serde_json::{Value, json};

use super::studio::{Studio, tab_label};
use super::{
    CloseTab, LaunchConfig, NewProject, NextTab, OpenProject, PreviousTab, ROOT_CONTEXT, RedoEdit,
    SaveProject, ShowHome, TogglePanels, ToggleTheme, UndoEdit, ZoomIn, ZoomOut, ZoomReset,
    ZoomToFit, apply_brand,
};
use crate::agent;
use crate::editor::Editor;
use crate::workspace::{WorkspaceState, default_path, relative_time};

/// Workspace-level commands an agent can use alongside the project commands.
const WORKSPACE_COMMANDS: &[(&str, &str, &str)] = &[
    (
        "list_projects",
        "List projects",
        "Open project tabs (with the active one marked) and recent projects.",
    ),
    (
        "switch_project",
        "Switch project",
        "Activate an open project tab by index from list_projects; project commands then act on it.",
    ),
];

/// The root view.
pub(crate) struct Workspace {
    state: WorkspaceState,
    state_path: Option<PathBuf>,
    persisted: Option<WorkspaceState>,
    pub(crate) tabs: Vec<Entity<Studio>>,
    bridge: Option<BridgeHandle>,
    mcp_endpoint: Option<String>,
}

impl Workspace {
    pub(crate) fn new(launch: &LaunchConfig, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state_path = launch.state_path.clone().or_else(default_path);
        let state = state_path
            .as_deref()
            .map(WorkspaceState::load)
            .unwrap_or_default();
        let first_run = state.recent.is_empty() && state.open.is_empty();
        let mut workspace = Self {
            state,
            state_path,
            persisted: None,
            tabs: Vec::new(),
            bridge: None,
            mcp_endpoint: None,
        };
        if workspace.state.dark {
            Theme::change(ThemeMode::Dark, Some(window), cx);
            apply_brand(cx);
        }
        // Restore the previous session's tabs.
        let restore = std::mem::take(&mut workspace.state.open);
        let active = workspace.state.active.take();
        let active_path = active.and_then(|i| restore.get(i).cloned());
        for path in restore {
            workspace.open_path(&path, false, window, cx);
        }
        workspace.state.active = active_path
            .and_then(|p| workspace.state.open.iter().position(|o| *o == p))
            .or(workspace.state.active);
        match &launch.project {
            Some(path) => {
                workspace.open_path(path, true, window, cx);
            }
            None if first_run => {
                if let Some(example) = &launch.example {
                    workspace.open_path(example, true, window, cx);
                }
            }
            None => {}
        }
        if launch.mcp {
            workspace.install_bridge(window, cx);
        }
        workspace.focus_active(window, cx);
        workspace.persisted = Some(workspace.state.clone());
        workspace.start_persist_loop(window, cx);
        workspace
    }

    fn active_studio(&self) -> Option<Entity<Studio>> {
        self.state.active.and_then(|i| self.tabs.get(i).cloned())
    }

    fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(studio) = self.active_studio() {
            let handle = studio.read(cx).canvas_focus.clone();
            handle.focus(window, cx);
        }
    }

    /// Open (or switch to) a project folder. Returns whether it is now open.
    pub(crate) fn open_path(
        &mut self,
        path: &Path,
        activate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        if let Some(index) = self.state.open.iter().position(|p| *p == canonical) {
            if activate {
                self.activate(Some(index), window, cx);
            }
            return true;
        }
        let editor = match Editor::open(path) {
            Ok(editor) => editor,
            Err(error) => {
                window.push_notification(
                    Notification::error(format!("Could not open {}: {error:#}", path.display())),
                    cx,
                );
                return false;
            }
        };
        let root = editor
            .project
            .as_ref()
            .map_or(canonical, |p| p.root().to_owned());
        let name = editor
            .project
            .as_ref()
            .map_or_else(|| "Untitled".to_owned(), |p| p.name.clone());
        let view = self.state.views.get(&root).copied();
        let panels = self
            .active_studio()
            .map_or(self.state.panels, |s| s.read(cx).panels);
        let studio = cx.new(|cx| Studio::new(editor, view, panels, window, cx));
        let previous = self.state.active;
        self.state.open_tab(&root);
        self.tabs.push(studio);
        self.state.touch_recent(&root, &name);
        if !activate {
            self.state.active = previous;
        } else {
            self.focus_active(window, cx);
        }
        cx.notify();
        true
    }

    pub(crate) fn activate(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panels = self.active_studio().map(|s| s.read(cx).panels);
        self.capture_views(cx);
        self.state.active = index.filter(|i| *i < self.tabs.len());
        if let (Some(panels), Some(studio)) = (panels, self.active_studio()) {
            studio.update(cx, |studio, cx| {
                studio.panels = panels;
                cx.notify();
            });
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(studio) = self.tabs.get(index).cloned() else {
            return;
        };
        let saved = studio.update(cx, |studio, _| studio.editor.save());
        if let Err(error) = saved {
            window.push_notification(Notification::error(format!("Save failed: {error:#}")), cx);
            return;
        }
        self.capture_views(cx);
        if self.state.active == Some(index) {
            self.state.panels = studio.read(cx).panels;
        }
        self.tabs.remove(index);
        self.state.close_tab(index);
        self.focus_active(window, cx);
        cx.notify();
    }

    fn capture_views(&mut self, cx: &mut Context<Self>) {
        for (path, studio) in self.state.open.clone().into_iter().zip(self.tabs.clone()) {
            let studio = studio.read(cx);
            self.state.views.insert(path, studio.view_state());
        }
        if let Some(studio) = self.active_studio() {
            self.state.panels = studio.read(cx).panels;
        }
        self.state.dark = cx.theme().is_dark();
    }

    pub(crate) fn persist(&mut self, cx: &mut Context<Self>) {
        self.capture_views(cx);
        if self.persisted.as_ref() == Some(&self.state) {
            return;
        }
        if let Some(path) = &self.state_path
            && let Err(error) = self.state.save(path)
        {
            eprintln!("could not save the workspace: {error:#}");
            return;
        }
        self.persisted = Some(self.state.clone());
    }

    fn start_persist_loop(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |this, cx| this.persist(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn prompt_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open design folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.open_path(&path, true, window, cx)
                });
            }
        })
        .detach();
    }

    fn prompt_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let base = directories::UserDirs::new()
            .and_then(|dirs| dirs.document_dir().map(Path::to_owned))
            .or_else(|| directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_owned()))
            .unwrap_or_else(std::env::temp_dir);
        let receiver = cx.prompt_for_new_path(&base, Some("Untitled design"));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = receiver.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.open_path(&path, true, window, cx)
                });
            }
        })
        .detach();
    }

    fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = if cx.theme().is_dark() {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
        Theme::change(mode, Some(window), cx);
        apply_brand(cx);
        self.state.dark = mode == ThemeMode::Dark;
        cx.notify();
    }

    fn cycle_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        // Home is slot 0, tabs follow.
        let slots = self.tabs.len() as isize + 1;
        let current = self.state.active.map_or(0, |i| i as isize + 1);
        let next = (current + delta).rem_euclid(slots);
        self.activate((next > 0).then(|| (next - 1) as usize), window, cx);
    }

    // ---- MCP -------------------------------------------------------------------------

    fn projects_json(&self, cx: &Context<Self>) -> Value {
        json!({
            "open": self.tabs.iter().enumerate().map(|(index, studio)| {
                let studio = studio.read(cx);
                json!({
                    "index": index,
                    "name": studio.name(),
                    "path": self.state.open.get(index).map(|p| p.display().to_string()),
                    "active": self.state.active == Some(index),
                    "revision": studio.editor.revision,
                })
            }).collect::<Vec<_>>(),
            "recent": self.state.recent.iter().map(|r| json!({
                "name": r.name,
                "path": r.path.display().to_string(),
            })).collect::<Vec<_>>(),
        })
    }

    fn execute(
        &mut self,
        name: &str,
        arguments: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(Value, Option<u64>), agent::AgentError> {
        match name {
            "list_projects" => return Ok((self.projects_json(cx), None)),
            "switch_project" => {
                let index = arguments
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| agent::AgentError::Invalid("index is required".into()))?
                    as usize;
                if index >= self.tabs.len() {
                    return Err(agent::AgentError::Invalid(format!(
                        "no open project at index {index}"
                    )));
                }
                self.activate(Some(index), window, cx);
                return Ok((self.projects_json(cx), None));
            }
            _ => {}
        }
        let studio = self.active_studio().ok_or_else(|| {
            agent::AgentError::Invalid(
                "no project is open; use list_projects and switch_project".into(),
            )
        })?;
        if name == "export_image" {
            return studio.update(cx, |this, cx| {
                #[derive(serde::Deserialize)]
                struct A {
                    node_id: String,
                    format: Option<String>,
                    scale: Option<f32>,
                }
                let a: A = serde_json::from_value(arguments)
                    .map_err(|e| agent::AgentError::Invalid(e.to_string()))?;
                let id = crate::model::NodeId::parse(&a.node_id)
                    .filter(|id| this.editor.doc.contains(*id))
                    .ok_or_else(|| agent::AgentError::Invalid(format!("no node {}", a.node_id)))?;
                let format = match a.format.as_deref() {
                    None => crate::export_image::ImageFormat::Png,
                    Some(f) => crate::export_image::ImageFormat::parse(f).ok_or_else(|| {
                        agent::AgentError::Invalid(format!("unknown format {f:?}"))
                    })?,
                };
                let scale = a.scale.unwrap_or(2.0);
                if !(0.1..=8.0).contains(&scale) {
                    return Err(agent::AgentError::Invalid("scale must be 0.1..=8".into()));
                }
                let path = this.export_path(id, format, scale).ok_or_else(|| {
                    agent::AgentError::Invalid("the design is not saved as a project".into())
                })?;
                this.start_image_export(
                    id,
                    format,
                    scale,
                    super::image_export::ExportTarget::File(path.clone()),
                    cx,
                );
                Ok((
                    serde_json::json!({ "path": path, "status": "exporting" }),
                    None,
                ))
            });
        }
        studio.update(cx, |this, cx| {
            let before = this.editor.selection.clone();
            this.editor.measured = this.canvas.measured();
            let result = agent::execute(&mut this.editor, name, arguments);
            // Follow the agent: show what it just selected or created.
            if this.editor.selection != before
                && let Some(id) = this.editor.primary()
            {
                this.reveal(id, cx);
            }
            this.inspector.invalidate();
            cx.notify();
            result.map(|output| (output, Some(this.editor.revision)))
        })
    }

    fn install_bridge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let app_id = match AppId::new("gpui-studio") {
            Ok(id) => id,
            Err(error) => {
                eprintln!("invalid MCP app id: {error}");
                return;
            }
        };
        let bridge =
            match BridgeHandle::install(window, cx, BridgeConfig::new(app_id, "GPUI Studio")) {
                Ok(bridge) => bridge,
                Err(error) => {
                    eprintln!("MCP bridge unavailable: {error}");
                    return;
                }
            };
        let weak = cx.weak_entity();
        let commands = weak.clone();
        let command_result = bridge.on_command(move |request, window, cx| match request {
            ApplicationCommandRequest::List => {
                let mut list: Vec<ApplicationCommandDescriptor> = WORKSPACE_COMMANDS
                    .iter()
                    .map(|(name, title, description)| ApplicationCommandDescriptor {
                        name: (*name).to_owned(),
                        title: (*title).to_owned(),
                        description: (*description).to_owned(),
                        input_schema: if *name == "switch_project" {
                            json!({ "type": "object", "properties": { "index": { "type": "integer", "minimum": 0 } }, "required": ["index"], "additionalProperties": false })
                        } else {
                            json!({ "type": "object", "properties": {}, "additionalProperties": false })
                        },
                        mutating: *name == "switch_project",
                    })
                    .collect();
                list.extend(agent::COMMANDS.iter().map(|spec| ApplicationCommandDescriptor {
                    name: spec.name.to_owned(),
                    title: spec.title.to_owned(),
                    description: spec.description.to_owned(),
                    input_schema: (spec.schema)(),
                    mutating: spec.mutating,
                }));
                Ok(ApplicationCommandResponse::List(list))
            }
            ApplicationCommandRequest::Execute { name, arguments } => {
                let outcome = commands
                    .update(cx, |this, cx| this.execute(&name, arguments, window, cx))
                    .map_err(|_| BridgeError::new(ErrorCode::Unsupported, "Studio closed"))?;
                match outcome {
                    Ok((output, revision)) => Ok(ApplicationCommandResponse::Result(ApplicationCommandResult {
                        name,
                        revision,
                        output,
                    })),
                    Err(error) => Err(BridgeError::new(
                        match error {
                            agent::AgentError::Unknown(_) => ErrorCode::NotFound,
                            _ => ErrorCode::InvalidRequest,
                        },
                        error.to_string(),
                    )),
                }
            }
        });
        let resources = weak.clone();
        let resource_result = bridge.on_resource(move |request, _window, cx| match request {
            ContextResourceRequest::List => Ok(ContextResourceResponse::List(
                agent::RESOURCES
                    .iter()
                    .map(|(uri, name, description)| descriptor(uri, name, description))
                    .collect(),
            )),
            ContextResourceRequest::Read { uri } => {
                let (name, description) = agent::RESOURCES
                    .iter()
                    .find(|(u, _, _)| *u == uri)
                    .map(|(_, n, d)| (*n, *d))
                    .ok_or_else(|| BridgeError::new(ErrorCode::NotFound, "unknown resource"))?;
                let text = resources
                    .update(cx, |this, cx| {
                        this.active_studio()
                            .and_then(|studio| agent::read_resource(&studio.read(cx).editor, &uri))
                    })
                    .ok()
                    .flatten()
                    .ok_or_else(|| {
                        BridgeError::new(ErrorCode::NotFound, "no open project or unknown resource")
                    })?;
                Ok(ContextResourceResponse::Resource(ContextResource {
                    descriptor: descriptor(&uri, name, description),
                    text,
                }))
            }
        });
        let documents = weak;
        let document_result = bridge.on_document(move |request, _window, cx| {
            let studio = documents
                .update(cx, |this, _| this.active_studio())
                .map_err(|_| BridgeError::new(ErrorCode::Unsupported, "Studio closed"))?
                .ok_or_else(|| BridgeError::new(ErrorCode::NotFound, "no project is open"))?;
            studio.update(cx, |this, cx| {
                let response = match request {
                    LiveDocumentRequest::Get => LiveDocumentResponse::Document(live_document(this)),
                    LiveDocumentRequest::Preview {
                        expected_revision,
                        source,
                    } => {
                        if expected_revision != this.editor.revision {
                            return Err(BridgeError::new(
                                ErrorCode::StaleRevision,
                                format!(
                                    "document is at revision {}, not {expected_revision}",
                                    this.editor.revision
                                ),
                            ));
                        }
                        let applied = agent::apply_live_html(&mut this.editor, &source.html);
                        let diagnostics = match &applied {
                            Ok(_) => Vec::new(),
                            Err(error) => vec![gpui_mcp::LiveDocumentDiagnostic {
                                severity: "error".to_owned(),
                                message: error.to_string(),
                            }],
                        };
                        this.inspector.invalidate();
                        cx.notify();
                        LiveDocumentResponse::Preview(LiveDocumentPreview {
                            applied: applied.is_ok(),
                            document: live_document(this),
                            diagnostics,
                        })
                    }
                };
                Ok(response)
            })
        });
        for result in [command_result, resource_result, document_result] {
            if let Err(error) = result {
                eprintln!("MCP host registration failed: {error}");
            }
        }
        self.mcp_endpoint = Some(bridge.endpoint_path().display().to_string());
        self.bridge = Some(bridge);
    }

    // ---- rendering ---------------------------------------------------------------------

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let home_active = self.state.active.is_none();
        let mut tabs = h_flex().h_full().gap_0p5().child(
            Button::new("tab-home")
                .icon(Icon::new(Lucide::House))
                .ghost()
                .xsmall()
                .selected(home_active)
                .tooltip("Home")
                .on_click(cx.listener(|this, _, window, cx| this.activate(None, window, cx))),
        );
        for (index, studio) in self.tabs.iter().enumerate() {
            let active = self.state.active == Some(index);
            let label = tab_label(studio.read(cx));
            let dirty = studio.read(cx).editor.is_dirty();
            let group: SharedString = format!("tab-{index}").into();
            tabs = tabs.child(
                h_flex()
                    .id(SharedString::from(format!("project-tab-{index}")))
                    .group(group.clone())
                    .h(px(28.0))
                    .pl_2p5()
                    .pr_1()
                    .gap_1()
                    .rounded(px(7.0))
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .when(active, |this| {
                        this.bg(theme.secondary).font_weight(FontWeight::MEDIUM)
                    })
                    .when(!active, |this| this.text_color(theme.muted_foreground))
                    .hover(|this| this.bg(theme.secondary))
                    .child(div().max_w(px(180.0)).truncate().child(label))
                    .child(
                        div()
                            .id(SharedString::from(format!("close-tab-{index}")))
                            .size(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .hover(|this| this.bg(theme.muted))
                            .when(!active, |this| {
                                this.invisible().group_hover(group.clone(), |s| s.visible())
                            })
                            .child(if dirty && !active {
                                div()
                                    .size(px(6.0))
                                    .rounded_full()
                                    .bg(theme.muted_foreground)
                                    .into_any_element()
                            } else {
                                Icon::new(Lucide::X).xsmall().into_any_element()
                            })
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.close_tab(index, window, cx)
                            })),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate(Some(index), window, cx)
                    })),
            );
        }
        let entity = cx.entity();
        tabs.child(
            Button::new("new-tab")
                .icon(Icon::new(Lucide::Plus))
                .ghost()
                .xsmall()
                .tooltip("New or open project")
                .dropdown_menu(move |menu, _, _| {
                    let new = entity.clone();
                    let open = entity.clone();
                    menu.item(
                        PopupMenuItem::new("New project…").on_click(move |_, window, cx| {
                            new.update(cx, |this, cx| this.prompt_new(window, cx));
                        }),
                    )
                    .item(
                        PopupMenuItem::new("Open folder…").on_click(move |_, window, cx| {
                            open.update(cx, |this, cx| this.prompt_open(window, cx));
                        }),
                    )
                }),
        )
        .into_any_element()
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let connected = self.bridge.is_some();
        let endpoint: SharedString = self
            .mcp_endpoint
            .as_ref()
            .map_or_else(
                || "Agent bridge off (--no-mcp)".to_owned(),
                |path| format!("Agents connect through gpui-mcp\n{path}"),
            )
            .into();
        TitleBar::new()
            .child(
                h_flex()
                    .w_full()
                    .h_full()
                    .justify_between()
                    .pr_2()
                    .child(self.render_tabs(cx))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                div()
                                    .id("mcp-status")
                                    .size(px(24.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(div().size(px(7.0)).rounded_full().bg(if connected {
                                        gpui_kit::rgb(0x22c55e)
                                    } else {
                                        theme.muted_foreground.into()
                                    }))
                                    .tooltip(move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(endpoint.clone())
                                            .build(window, cx)
                                    }),
                            )
                            .child(
                                Button::new("theme")
                                    .icon(Icon::new(if theme.is_dark() {
                                        Lucide::Sun
                                    } else {
                                        Lucide::Moon
                                    }))
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Toggle light / dark")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_theme(window, cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_home(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mut grid = div().grid().grid_cols(3).gap_3();
        for (index, recent) in self.state.recent.iter().enumerate() {
            let path = recent.path.clone();
            let exists = path.join("studio.ron").is_file();
            let forget = path.clone();
            let group: SharedString = format!("recent-{index}").into();
            grid = grid.child(
                v_flex()
                    .id(SharedString::from(format!("recent-project-{index}")))
                    .group(group.clone())
                    .relative()
                    .gap_1()
                    .p_3()
                    .rounded(px(12.0))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .cursor_pointer()
                    .hover(|this| this.border_color(theme.primary.opacity(0.6)))
                    .when(!exists, |this| this.opacity(0.5))
                    .child(
                        div()
                            .h(px(96.0))
                            .rounded(px(8.0))
                            .bg(theme.secondary)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(Lucide::Frame).text_color(theme.muted_foreground)),
                    )
                    .child(
                        div()
                            .pt_1()
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(recent.name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(if exists {
                                relative_time(recent.opened_at)
                            } else {
                                "Missing".to_owned()
                            }),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(16.0))
                            .right(px(16.0))
                            .invisible()
                            .group_hover(group.clone(), |s| s.visible())
                            .child(
                                Button::new(SharedString::from(format!("forget-{index}")))
                                    .icon(Icon::new(Lucide::X))
                                    .xsmall()
                                    .tooltip("Remove from recents")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.forget_recent(&forget);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when(exists, |this| {
                        this.on_click(cx.listener(move |this, _, window, cx| {
                            this.open_path(&path, true, window, cx);
                        }))
                    }),
            );
        }
        div()
            .id("home")
            .size_full()
            .overflow_y_scrollbar()
            .bg(theme.sidebar)
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(920.0))
                    .mx_auto()
                    .px_8()
                    .py_10()
                    .gap_6()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(20.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Projects"),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new("home-open")
                                            .small()
                                            .outline()
                                            .icon(Icon::new(Lucide::FolderOpen))
                                            .label("Open folder")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.prompt_open(window, cx)
                                            })),
                                    )
                                    .child(
                                        Button::new("home-new")
                                            .small()
                                            .primary()
                                            .icon(Icon::new(Lucide::Plus))
                                            .label("New project")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.prompt_new(window, cx)
                                            })),
                                    ),
                            ),
                    )
                    .child(if self.state.recent.is_empty() {
                        div()
                            .py_16()
                            .flex()
                            .justify_center()
                            .text_color(theme.muted_foreground)
                            .child("No projects yet.")
                            .into_any_element()
                    } else {
                        grid.into_any_element()
                    }),
            )
            .into_any_element()
    }
}

fn live_document(studio: &Studio) -> LiveDocument {
    LiveDocument {
        revision: studio.editor.revision,
        source: LiveDocumentSource {
            html: agent::live_html(&studio.editor),
            css: String::new(),
            bindings_ron: String::new(),
        },
        diagnostics: Vec::new(),
    }
}

fn descriptor(uri: &str, name: &str, description: &str) -> ContextResourceDescriptor {
    ContextResourceDescriptor {
        uri: uri.to_owned(),
        name: name.to_owned(),
        title: None,
        description: Some(description.to_owned()),
        mime_type: "application/json".to_owned(),
        size: None,
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let title_bar = self.render_title_bar(cx);
        let body = match self.active_studio() {
            Some(studio) => studio.into_any_element(),
            None => self.render_home(cx),
        };
        v_flex()
            .id("workspace")
            .key_context(ROOT_CONTEXT)
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family("Geist")
            .text_size(px(12.0))
            .on_action(cx.listener(|this, _: &UndoEdit, _, cx| {
                if let Some(studio) = this.active_studio() {
                    studio.update(cx, |s, cx| {
                        s.editor.undo();
                        s.inspector.invalidate();
                        cx.notify();
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &RedoEdit, _, cx| {
                if let Some(studio) = this.active_studio() {
                    studio.update(cx, |s, cx| {
                        s.editor.redo();
                        s.inspector.invalidate();
                        cx.notify();
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &SaveProject, window, cx| {
                if let Some(studio) = this.active_studio() {
                    studio.update(cx, |s, cx| {
                        if s.apply(window, cx, |e| e.save()).is_some() {
                            window.push_notification(Notification::success("Saved"), cx);
                        }
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| {
                if let Some(studio) = this.active_studio() {
                    studio.update(cx, |s, cx| s.zoom_by(1.25, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| {
                if let Some(studio) = this.active_studio() {
                    studio.update(cx, |s, cx| s.zoom_by(0.8, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &ZoomReset, _, cx| {
                if let Some(studio) = this.active_studio() {
                    studio.update(cx, |s, cx| s.zoom_reset(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &ZoomToFit, _, cx| {
                if let Some(studio) = this.active_studio() {
                    studio.update(cx, |s, cx| s.zoom_fit(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &TogglePanels, _, cx| {
                if let Some(studio) = this.active_studio() {
                    studio.update(cx, |s, cx| s.toggle_panels(cx));
                }
            }))
            .on_action(
                cx.listener(|this, _: &OpenProject, window, cx| this.prompt_open(window, cx)),
            )
            .on_action(cx.listener(|this, _: &NewProject, window, cx| this.prompt_new(window, cx)))
            .on_action(
                cx.listener(|this, _: &ToggleTheme, window, cx| this.toggle_theme(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShowHome, window, cx| this.activate(None, window, cx)),
            )
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.cycle_tab(1, window, cx)))
            .on_action(
                cx.listener(|this, _: &PreviousTab, window, cx| this.cycle_tab(-1, window, cx)),
            )
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                if let Some(index) = this.state.active {
                    this.close_tab(index, window, cx);
                }
            }))
            .child(title_bar)
            .child(div().flex_1().min_h_0().child(body))
    }
}
