//! The native Studio window, built on GPUI Kit.

mod canvas;
mod inspector;
mod paint;
mod panels;

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, Theme, ThemeMode,
    TitleBar, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, KeyBinding,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Subscription, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, size,
};
use gpui_mcp::{
    AppId, ApplicationCommandDescriptor, ApplicationCommandRequest, ApplicationCommandResponse,
    ApplicationCommandResult, BridgeConfig, BridgeError, BridgeHandle, ContextResource,
    ContextResourceDescriptor, ContextResourceRequest, ContextResourceResponse, ErrorCode,
    LiveDocument, LiveDocumentPreview, LiveDocumentRequest, LiveDocumentResponse,
    LiveDocumentSource,
};

use crate::agent;
use crate::editor::{Editor, Tool};
use crate::export::CodeFormat;
use crate::model::NodeId;
use crate::ui::canvas::CanvasState;
use crate::ui::inspector::Inspector;
use crate::ui::paint::FontBook;

use gpui_kit::assets::IconName as Lucide;

mod actions {
    #![allow(missing_docs)]
    gpui_kit::actions!(
        studio,
        [
            SelectTool,
            FrameTool,
            RectangleTool,
            TextTool,
            HandTool,
            CommentTool,
            DeleteSelection,
            DuplicateSelection,
            CopySelection,
            CutSelection,
            PasteClipboard,
            UndoEdit,
            RedoEdit,
            GroupSelection,
            UngroupSelection,
            ToggleAutoLayout,
            SelectAllSiblings,
            EscapeSelection,
            EnterSelection,
            NudgeLeft,
            NudgeRight,
            NudgeUp,
            NudgeDown,
            NudgeLeftBig,
            NudgeRightBig,
            NudgeUpBig,
            NudgeDownBig,
            ZoomIn,
            ZoomOut,
            ZoomToFit,
            ZoomToSelection,
            ZoomReset,
            SaveProject,
            OpenProject,
            BringForward,
            SendBackward,
            ToggleHidden,
            ToggleLocked,
            ToggleTheme,
            RenameSelection
        ]
    );
}
use actions::*;

const CANVAS_CONTEXT: &str = "StudioCanvas";
const ROOT_CONTEXT: &str = "Studio";

/// Process configuration.
#[derive(Clone, Debug)]
pub struct StudioConfig {
    /// Project folder to open (created and scaffolded when missing).
    pub project: PathBuf,
    /// Install the local MCP bridge.
    pub mcp: bool,
}

fn bindings() -> Vec<KeyBinding> {
    let c = Some(CANVAS_CONTEXT);
    let r = Some(ROOT_CONTEXT);
    vec![
        KeyBinding::new("v", SelectTool, c),
        KeyBinding::new("f", FrameTool, c),
        KeyBinding::new("a", FrameTool, c),
        KeyBinding::new("r", RectangleTool, c),
        KeyBinding::new("t", TextTool, c),
        KeyBinding::new("h", HandTool, c),
        KeyBinding::new("c", CommentTool, c),
        KeyBinding::new("backspace", DeleteSelection, c),
        KeyBinding::new("delete", DeleteSelection, c),
        KeyBinding::new("secondary-d", DuplicateSelection, c),
        KeyBinding::new("secondary-c", CopySelection, c),
        KeyBinding::new("secondary-x", CutSelection, c),
        KeyBinding::new("secondary-v", PasteClipboard, c),
        KeyBinding::new("secondary-g", GroupSelection, c),
        KeyBinding::new("secondary-shift-g", UngroupSelection, c),
        KeyBinding::new("shift-a", ToggleAutoLayout, c),
        KeyBinding::new("secondary-a", SelectAllSiblings, c),
        KeyBinding::new("escape", EscapeSelection, c),
        KeyBinding::new("enter", EnterSelection, c),
        KeyBinding::new("left", NudgeLeft, c),
        KeyBinding::new("right", NudgeRight, c),
        KeyBinding::new("up", NudgeUp, c),
        KeyBinding::new("down", NudgeDown, c),
        KeyBinding::new("shift-left", NudgeLeftBig, c),
        KeyBinding::new("shift-right", NudgeRightBig, c),
        KeyBinding::new("shift-up", NudgeUpBig, c),
        KeyBinding::new("shift-down", NudgeDownBig, c),
        KeyBinding::new("shift-1", ZoomToFit, c),
        KeyBinding::new("shift-2", ZoomToSelection, c),
        KeyBinding::new("shift-0", ZoomReset, c),
        // Layouts that report the shifted symbol instead of shift+digit.
        KeyBinding::new("!", ZoomToFit, c),
        KeyBinding::new("shift-!", ZoomToFit, c),
        KeyBinding::new("@", ZoomToSelection, c),
        KeyBinding::new("shift-@", ZoomToSelection, c),
        KeyBinding::new(")", ZoomReset, c),
        KeyBinding::new("shift-)", ZoomReset, c),
        KeyBinding::new("=", ZoomIn, c),
        KeyBinding::new("-", ZoomOut, c),
        KeyBinding::new("secondary-]", BringForward, c),
        KeyBinding::new("secondary-[", SendBackward, c),
        KeyBinding::new("secondary-shift-h", ToggleHidden, c),
        KeyBinding::new("secondary-shift-l", ToggleLocked, c),
        KeyBinding::new("secondary-r", RenameSelection, c),
        KeyBinding::new("secondary-z", UndoEdit, r),
        KeyBinding::new("secondary-shift-z", RedoEdit, r),
        KeyBinding::new("secondary-y", RedoEdit, r),
        KeyBinding::new("secondary-s", SaveProject, r),
        KeyBinding::new("secondary-o", OpenProject, r),
        KeyBinding::new("secondary-=", ZoomIn, r),
        KeyBinding::new("secondary--", ZoomOut, r),
        KeyBinding::new("secondary-0", ZoomReset, r),
        KeyBinding::new("secondary-1", ZoomToFit, r),
    ]
}

/// Paper-like palette applied over GPUI Kit's light/dark themes.
fn apply_brand(cx: &mut App) {
    Theme::update(cx, |theme| {
        let accent: gpui_kit::Hsla = gpui_kit::rgb(0x0d99ff).into();
        theme.font_family = "Geist".into();
        theme.mono_font_family = "Geist Mono".into();
        theme.font_size = px(13.0);
        theme.radius = px(6.0);
        theme.ring = accent;
        theme.primary = accent;
        theme.primary_hover = accent.opacity(0.9);
        theme.primary_active = accent.opacity(0.8);
        theme.button_primary = accent;
        theme.button_primary_hover = accent.opacity(0.9);
        theme.button_primary_active = accent.opacity(0.8);
        theme.selection = accent.opacity(0.25);
        if !theme.is_dark() {
            theme.background = gpui_kit::rgb(0xffffff).into();
            theme.sidebar = gpui_kit::rgb(0xffffff).into();
            theme.title_bar = gpui_kit::rgb(0xffffff).into();
            theme.border = gpui_kit::rgb(0xe8e8e6).into();
        }
    });
}

/// Static Geist faces. GPUI instantiates variable fonts only at their default
/// weight, so each weight ships as its own face for CSS `font-weight` to match.
fn bundled_fonts() -> Vec<Cow<'static, [u8]>> {
    macro_rules! fonts {
        ($($file:literal),* $(,)?) => {
            vec![$(Cow::Borrowed(include_bytes!(concat!("../../assets/fonts/", $file)).as_slice())),*]
        };
    }
    fonts![
        "Geist-Light.ttf",
        "Geist-Regular.ttf",
        "Geist-Italic.ttf",
        "Geist-Medium.ttf",
        "Geist-MediumItalic.ttf",
        "Geist-SemiBold.ttf",
        "Geist-SemiBoldItalic.ttf",
        "Geist-Bold.ttf",
        "Geist-BoldItalic.ttf",
        "Geist-Black.ttf",
        "GeistMono-Regular.ttf",
        "GeistMono-Medium.ttf",
        "GeistMono-SemiBold.ttf",
        "GeistMono-Bold.ttf",
    ]
}

/// Start Studio and block until the window closes.
pub fn run(config: StudioConfig) {
    gpui_kit::application()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);
            if let Err(error) = cx.text_system().add_fonts(bundled_fonts()) {
                eprintln!("could not register bundled fonts: {error:#}");
            }
            Theme::change(ThemeMode::Light, None, cx);
            apply_brand(cx);
            cx.bind_keys(bindings());
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1440.0), px(900.0)), cx)),
                window_min_size: Some(size(px(900.0), px(560.0))),
                ..TitleBar::window_options()
            };
            let config = config.clone();
            let opened = gpui_kit::open_window(options, cx, move |window, cx| {
                window.set_window_title("GPUI Studio");
                cx.new(|cx| match Studio::new(&config, window, cx) {
                    Ok(studio) => studio,
                    Err(error) => {
                        eprintln!("could not open {}: {error:#}", config.project.display());
                        std::process::exit(1);
                    }
                })
            });
            if let Err(error) = opened {
                eprintln!("could not open the Studio window: {error:#}");
                cx.quit();
                return;
            }
            cx.activate(true);
        });
}

/// Embedded icon assets: GPUI Kit's defaults plus the Lucide icons Studio uses.
struct Assets;

gpui_kit::assets::icon_assets!(
    StudioIcons,
    [
        MousePointer2,
        Frame,
        Square,
        Type,
        Hand,
        MessageSquare,
        MessageSquarePlus,
        Trash,
        Copy,
        Eye,
        EyeOff,
        Lock,
        LockOpen,
        Layers,
        Component,
        Plus,
        Minus,
        ChevronDown,
        ChevronRight,
        Undo2,
        Redo2,
        Code,
        Download,
        Sun,
        Moon,
        Monitor,
        Smartphone,
        Tablet,
        Image,
        Group,
        Ungroup,
        AlignStartVertical,
        AlignCenterVertical,
        AlignEndVertical,
        AlignStartHorizontal,
        AlignCenterHorizontal,
        AlignEndHorizontal,
        ArrowRight,
        ArrowDown,
        Grid2x2,
        Rows3,
        Columns3,
        Bold,
        Italic,
        Underline,
        Strikethrough,
        TextAlignStart,
        TextAlignCenter,
        TextAlignEnd,
        FolderOpen,
        Save,
        Check,
        CheckCheck,
        X,
        Sparkles,
        Plug,
        PanelLeft,
        PanelRight,
        ZoomIn,
        ZoomOut,
        Scan,
        Maximize,
        Shapes,
        Box,
        LayoutTemplate,
        FileCode,
        Diamond,
        Circle,
        Ellipsis,
        Pipette,
        SquareDashed
    ]
);

impl gpui_kit::AssetSource for Assets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        if let Some(found) = StudioIcons.load(path)? {
            return Ok(Some(found));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        let mut all = StudioIcons.list(path)?;
        all.extend(gpui_kit::assets::Assets.list(path)?);
        Ok(all)
    }
}

/// Which tab the left sidebar shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LeftTab {
    Layers,
    Assets,
}

/// Which tab the right sidebar shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RightTab {
    Design,
    Code,
    Comments,
}

/// The root view.
pub struct Studio {
    editor: Editor,
    tool: Tool,
    canvas: CanvasState,
    canvas_focus: FocusHandle,
    fonts: Rc<FontBook>,
    inspector: Inspector,
    left_tab: LeftTab,
    right_tab: RightTab,
    code_format: CodeFormat,
    collapsed: BTreeSet<NodeId>,
    rename: Option<(NodeId, Entity<InputState>)>,
    page_rename: Option<(usize, Entity<InputState>)>,
    comment_draft: Option<(NodeId, (f32, f32), Entity<InputState>)>,
    hovered_comment: Option<u64>,
    show_done_comments: bool,
    bridge: Option<BridgeHandle>,
    mcp_endpoint: Option<String>,
    shown_status: String,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for Studio {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.canvas_focus.clone()
    }
}

impl Studio {
    fn new(
        config: &StudioConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Self> {
        let editor = Editor::open(&config.project)?;
        let fonts = Rc::new(FontBook::new(cx.text_system().all_font_names()));
        let inspector = Inspector::new(window, cx);
        let canvas_focus = cx.focus_handle();
        let mut studio = Self {
            editor,
            tool: Tool::Select,
            canvas: CanvasState::new(),
            canvas_focus,
            fonts,
            inspector,
            left_tab: LeftTab::Layers,
            right_tab: RightTab::Design,
            code_format: CodeFormat::Html,
            collapsed: BTreeSet::new(),
            rename: None,
            page_rename: None,
            comment_draft: None,
            hovered_comment: None,
            show_done_comments: false,
            bridge: None,
            mcp_endpoint: None,
            shown_status: String::new(),
            _subscriptions: Vec::new(),
        };
        if config.mcp {
            studio.install_bridge(window, cx);
        }
        studio.start_background_loop(window, cx);
        studio.canvas_focus.focus(window, cx);
        Ok(studio)
    }

    fn start_background_loop(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                let alive = this.update_in(cx, |this, window, cx| {
                    let saved = this.editor.autosave();
                    let reloaded = this.editor.sync_external();
                    if reloaded {
                        this.inspector.invalidate();
                    }
                    if saved || reloaded {
                        cx.notify();
                    }
                    this.flush_status(window, cx);
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn flush_status(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.status != self.shown_status {
            self.shown_status = self.editor.status.clone();
            if self.shown_status.contains("failed") {
                window.push_notification(Notification::error(self.shown_status.clone()), cx);
            }
            cx.notify();
        }
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
                    self.editor.status = format!("MCP unavailable: {error}");
                    return;
                }
            };
        let weak = cx.weak_entity();
        let commands = weak.clone();
        let command_result = bridge.on_command(move |request, _window, cx| {
            match request {
                ApplicationCommandRequest::List => Ok(ApplicationCommandResponse::List(
                    agent::COMMANDS
                        .iter()
                        .map(|spec| ApplicationCommandDescriptor {
                            name: spec.name.to_owned(),
                            title: spec.title.to_owned(),
                            description: spec.description.to_owned(),
                            input_schema: (spec.schema)(),
                            mutating: spec.mutating,
                        })
                        .collect(),
                )),
                ApplicationCommandRequest::Execute { name, arguments } => {
                    let outcome = commands
                        .update(cx, |this, cx| {
                            let before = this.editor.selection.clone();
                            let result = agent::execute(&mut this.editor, &name, arguments);
                            // Follow the agent: show what it just selected or created.
                            if this.editor.selection != before
                                && let Some(id) = this.editor.primary()
                            {
                                this.reveal(id, cx);
                            }
                            this.inspector.invalidate();
                            cx.notify();
                            result.map(|output| (output, this.editor.revision))
                        })
                        .map_err(|_| BridgeError::new(ErrorCode::Unsupported, "Studio closed"))?;
                    match outcome {
                        Ok((output, revision)) => Ok(ApplicationCommandResponse::Result(
                            ApplicationCommandResult {
                                name,
                                revision: Some(revision),
                                output,
                            },
                        )),
                        Err(error) => Err(BridgeError::new(
                            match error {
                                agent::AgentError::Unknown(_) => ErrorCode::NotFound,
                                _ => ErrorCode::InvalidRequest,
                            },
                            error.to_string(),
                        )),
                    }
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
                    .update(cx, |this, _| agent::read_resource(&this.editor, &uri))
                    .ok()
                    .flatten()
                    .ok_or_else(|| BridgeError::new(ErrorCode::NotFound, "unknown resource"))?;
                Ok(ContextResourceResponse::Resource(ContextResource {
                    descriptor: descriptor(&uri, name, description),
                    text,
                }))
            }
        });
        let documents = weak;
        let document_result = bridge.on_document(move |request, _window, cx| {
            documents
                .update(cx, |this, cx| {
                    let response = match request {
                        LiveDocumentRequest::Get => {
                            LiveDocumentResponse::Document(this.live_document())
                        }
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
                                document: this.live_document(),
                                diagnostics,
                            })
                        }
                    };
                    Ok(response)
                })
                .map_err(|_| BridgeError::new(ErrorCode::Unsupported, "Studio closed"))?
        });
        for result in [command_result, resource_result, document_result] {
            if let Err(error) = result {
                eprintln!("MCP host registration failed: {error}");
            }
        }
        self.mcp_endpoint = Some(bridge.endpoint_path().display().to_string());
        self.bridge = Some(bridge);
    }

    fn live_document(&self) -> LiveDocument {
        LiveDocument {
            revision: self.editor.revision,
            source: LiveDocumentSource {
                html: agent::live_html(&self.editor),
                css: String::new(),
                bindings_ron: String::new(),
            },
            diagnostics: Vec::new(),
        }
    }

    /// Run an editor operation from the UI, reporting failures.
    fn apply<R>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        op: impl FnOnce(&mut Editor) -> anyhow::Result<R>,
    ) -> Option<R> {
        let result = op(&mut self.editor);
        self.inspector.invalidate();
        cx.notify();
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                window.push_notification(Notification::warning(format!("{error:#}")), cx);
                None
            }
        }
    }

    fn set_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.tool = tool;
        self.canvas.cancel_drag();
        cx.notify();
    }

    fn open_project_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                let _ = this.update_in(cx, |this, window, cx| this.open_project(path, window, cx));
            }
        })
        .detach();
    }

    fn open_project(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.editor.save() {
            window.push_notification(Notification::error(format!("Save failed: {error:#}")), cx);
            return;
        }
        match Editor::open(&path) {
            Ok(editor) => {
                self.editor = editor;
                self.canvas = CanvasState::new();
                self.collapsed.clear();
                self.inspector.invalidate();
                window.push_notification(
                    Notification::success(format!("Opened {}", path.display())),
                    cx,
                );
            }
            Err(error) => {
                window.push_notification(Notification::error(format!("{error:#}")), cx);
            }
        }
        cx.notify();
    }

    fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = if cx.theme().is_dark() {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
        Theme::change(mode, Some(window), cx);
        apply_brand(cx);
        cx.notify();
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        let tool_button =
            |id: &'static str, icon: Lucide, tool: Tool, current: Tool, cx: &mut Context<Self>| {
                Button::new(id)
                    .icon(Icon::new(icon))
                    .ghost()
                    .small()
                    .selected(current == tool)
                    .tooltip(tool.label())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_tool(tool, cx);
                        this.canvas_focus.focus(window, cx);
                    }))
            };
        let current = self.tool;
        let zoom = format!("{:.0}%", self.canvas.camera.zoom * 100.0);
        let project_name = self
            .editor
            .project
            .as_ref()
            .map_or_else(|| "Untitled".to_owned(), |p| p.name.clone());
        let saved = if self.editor.is_dirty() {
            "Editing…"
        } else {
            "Saved"
        };
        let mcp_connected = self.bridge.is_some();
        let entity = cx.entity();
        let menu_entity = entity.clone();
        let zoom_entity = entity.clone();
        let insert_entity = entity;
        TitleBar::new()
            .child(
                h_flex()
                    .w_full()
                    .h_full()
                    .justify_between()
                    .pr_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("app-menu")
                                    .ghost()
                                    .small()
                                    .child(
                                        h_flex()
                                            .gap_1p5()
                                            .child(
                                                div()
                                                    .size(px(14.0))
                                                    .rounded(px(4.0))
                                                    .bg(gpui_kit::rgb(0x0d99ff)),
                                            )
                                            .child(
                                                div()
                                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                                    .child(project_name),
                                            ),
                                    )
                                    .dropdown_menu(move |menu, _, _| {
                                        let open = menu_entity.clone();
                                        let save = menu_entity.clone();
                                        let theme = menu_entity.clone();
                                        menu.item(PopupMenuItem::new("Open folder…").on_click(
                                            move |_, window, cx| {
                                                open.update(cx, |this, cx| {
                                                    this.open_project_dialog(window, cx)
                                                });
                                            },
                                        ))
                                        .item(PopupMenuItem::new("Save now").on_click(
                                            move |_, window, cx| {
                                                save.update(cx, |this, cx| {
                                                    this.apply(window, cx, |e| e.save());
                                                });
                                            },
                                        ))
                                        .separator()
                                        .item(
                                            PopupMenuItem::new("Toggle light / dark").on_click(
                                                move |_, window, cx| {
                                                    theme.update(cx, |this, cx| {
                                                        this.toggle_theme(window, cx)
                                                    });
                                                },
                                            ),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(saved),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_0p5()
                            .p_0p5()
                            .rounded(px(8.0))
                            .bg(theme.secondary)
                            .child(tool_button(
                                "tool-select",
                                Lucide::MousePointer2,
                                Tool::Select,
                                current,
                                cx,
                            ))
                            .child(tool_button(
                                "tool-frame",
                                Lucide::Frame,
                                Tool::Frame,
                                current,
                                cx,
                            ))
                            .child(tool_button(
                                "tool-rect",
                                Lucide::Square,
                                Tool::Rectangle,
                                current,
                                cx,
                            ))
                            .child(tool_button(
                                "tool-text",
                                Lucide::Type,
                                Tool::Text,
                                current,
                                cx,
                            ))
                            .child(tool_button(
                                "tool-hand",
                                Lucide::Hand,
                                Tool::Hand,
                                current,
                                cx,
                            ))
                            .child(tool_button(
                                "tool-comment",
                                Lucide::MessageSquare,
                                Tool::Comment,
                                current,
                                cx,
                            ))
                            .child(div().w(px(1.0)).h(px(16.0)).mx_1().bg(theme.border))
                            .child(
                                Button::new("insert-menu")
                                    .icon(Icon::new(Lucide::Component))
                                    .ghost()
                                    .small()
                                    .tooltip("Insert component")
                                    .dropdown_menu(move |mut menu, _, _| {
                                        menu = menu.label("Insert into selection");
                                        for component in crate::presets::COMPONENTS {
                                            let entity = insert_entity.clone();
                                            menu = menu.item(
                                                PopupMenuItem::new(component.name).on_click(
                                                    move |_, window, cx| {
                                                        entity.update(cx, |this, cx| {
                                                            this.insert_component(
                                                                component.key,
                                                                window,
                                                                cx,
                                                            );
                                                        });
                                                    },
                                                ),
                                            );
                                        }
                                        menu
                                    }),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("undo")
                                    .icon(Icon::new(Lucide::Undo2))
                                    .ghost()
                                    .small()
                                    .disabled(!self.editor.history.can_undo())
                                    .tooltip("Undo")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.editor.undo();
                                        this.inspector.invalidate();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("redo")
                                    .icon(Icon::new(Lucide::Redo2))
                                    .ghost()
                                    .small()
                                    .disabled(!self.editor.history.can_redo())
                                    .tooltip("Redo")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.editor.redo();
                                        this.inspector.invalidate();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("zoom-menu")
                                    .ghost()
                                    .small()
                                    .label(zoom)
                                    .dropdown_caret(true)
                                    .dropdown_menu(move |menu, _, _| {
                                        let a = zoom_entity.clone();
                                        let b = zoom_entity.clone();
                                        let c = zoom_entity.clone();
                                        let d = zoom_entity.clone();
                                        let e = zoom_entity.clone();
                                        menu.item(PopupMenuItem::new("Zoom in").on_click(
                                            move |_, _, cx| {
                                                a.update(cx, |this, cx| this.zoom_by(1.25, cx));
                                            },
                                        ))
                                        .item(PopupMenuItem::new("Zoom out").on_click(
                                            move |_, _, cx| {
                                                b.update(cx, |this, cx| this.zoom_by(0.8, cx));
                                            },
                                        ))
                                        .item(PopupMenuItem::new("Zoom to 100%").on_click(
                                            move |_, _, cx| {
                                                c.update(cx, |this, cx| this.zoom_reset(cx));
                                            },
                                        ))
                                        .item(PopupMenuItem::new("Zoom to fit").on_click(
                                            move |_, _, cx| {
                                                d.update(cx, |this, cx| this.zoom_fit(cx));
                                            },
                                        ))
                                        .item(
                                            PopupMenuItem::new("Zoom to selection").on_click(
                                                move |_, _, cx| {
                                                    e.update(cx, |this, cx| {
                                                        this.zoom_selection(cx)
                                                    });
                                                },
                                            ),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .id("mcp-status")
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_full()
                                    .bg(theme.secondary)
                                    .text_xs()
                                    .child(div().size(px(6.0)).rounded_full().bg(
                                        if mcp_connected {
                                            gpui_kit::rgb(0x22c55e)
                                        } else {
                                            gpui_kit::rgb(0x9ca3af)
                                        },
                                    ))
                                    .child(if mcp_connected { "MCP" } else { "Offline" })
                                    .tooltip({
                                        let text: SharedString = self
                                            .mcp_endpoint
                                            .as_ref()
                                            .map_or_else(
                                                || "Agent bridge disabled (--no-mcp)".to_owned(),
                                                |path| {
                                                    format!(
                                                        "Agents connect through gpui-mcp\n{path}"
                                                    )
                                                },
                                            )
                                            .into();
                                        move |window, cx| {
                                            gpui_kit::component::tooltip::Tooltip::new(text.clone())
                                                .build(window, cx)
                                        }
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
                                    .small()
                                    .tooltip("Toggle light / dark")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_theme(window, cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn insert_component(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.editor.insertion_target();
        self.apply(window, cx, |e| e.insert_component(key, target));
        self.canvas_focus.focus(window, cx);
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        let selection = match self.editor.selection.as_slice() {
            [] => "No selection".to_owned(),
            [one] => {
                let size = self
                    .canvas
                    .doc_bounds(*one)
                    .map(|b| format!(" · {:.0} × {:.0}", b.size.width, b.size.height))
                    .unwrap_or_default();
                format!("{}{size}", self.editor.doc.display_name(*one))
            }
            many => format!("{} layers", many.len()),
        };
        h_flex()
            .h(px(26.0))
            .px_3()
            .gap_4()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.background)
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(div().flex_1().truncate().child(self.editor.status.clone()))
            .child(selection)
            .child(format!("rev {}", self.editor.revision))
            .into_any_element()
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

impl Render for Studio {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_inspector(window, cx);
        let theme = cx.theme().clone();
        let toolbar = self.render_toolbar(cx);
        let left = self.render_left_panel(window, cx);
        let canvas = self.render_canvas(window, cx);
        let right = self.render_right_panel(window, cx);
        let status = self.render_status_bar(cx);
        v_flex()
            .id("studio")
            .key_context(ROOT_CONTEXT)
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family("Geist")
            .text_size(px(12.0))
            .on_action(cx.listener(|this, _: &UndoEdit, _, cx| {
                this.editor.undo();
                this.inspector.invalidate();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &RedoEdit, _, cx| {
                this.editor.redo();
                this.inspector.invalidate();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SaveProject, window, cx| {
                if this.apply(window, cx, |e| e.save()).is_some() {
                    window.push_notification(Notification::success("Saved"), cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &OpenProject, window, cx| {
                    this.open_project_dialog(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &ToggleTheme, window, cx| this.toggle_theme(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom_by(1.25, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom_by(0.8, cx)))
            .on_action(cx.listener(|this, _: &ZoomReset, _, cx| this.zoom_reset(cx)))
            .on_action(cx.listener(|this, _: &ZoomToFit, _, cx| this.zoom_fit(cx)))
            .child(toolbar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(left)
                    .child(canvas)
                    .child(right),
            )
            .child(status)
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        AppContext as _, Bounds, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
        MouseUpEvent, PlatformInput, Point, TestAppContext, Window, WindowBounds, WindowOptions,
        point, px, size,
    };

    use super::{Studio, StudioConfig, bindings, bundled_fonts};
    use crate::model::NodeId;

    /// Window-space center of a design node, located through the same
    /// semantic tree MCP agents read.
    fn center(automation: &gpui_mcp::Automation, id: NodeId) -> Point<gpui_kit::Pixels> {
        let tree = automation.snapshot();
        let suffix = format!("node-{id}");
        let node = tree
            .nodes
            .values()
            .find(|node| node.id.ends_with(&suffix))
            .unwrap_or_else(|| panic!("{suffix} is not in the semantic tree"));
        let rect = node.bounds.expect("node has bounds");
        point(
            px(rect.x + rect.width / 2.0),
            px(rect.y + rect.height / 2.0),
        )
    }

    fn bounds(automation: &gpui_mcp::Automation, id: NodeId) -> gpui_mcp::Rect {
        let suffix = format!("node-{id}");
        automation
            .snapshot()
            .nodes
            .values()
            .find(|node| node.id.ends_with(&suffix))
            .and_then(|node| node.bounds)
            .unwrap_or_else(|| panic!("{suffix} has no bounds"))
    }

    fn click(
        window: &mut Window,
        cx: &mut gpui_kit::App,
        position: Point<gpui_kit::Pixels>,
        count: usize,
    ) {
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers: Modifiers::default(),
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position,
                modifiers: Modifiers::default(),
                click_count: count,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position,
                modifiers: Modifiers::default(),
                click_count: count,
            }),
            cx,
        );
        window.render_frame(cx);
    }

    #[gpui_kit::test]
    fn select_draw_edit_and_undo_through_the_real_shell(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().expect("temp project");
        let project = dir.path().join("design");
        let automation = gpui_mcp::Automation::isolated();
        let observed = automation.clone();
        let (handle, view) = cx.update(|cx| {
            gpui_kit::init(cx);
            cx.text_system().add_fonts(bundled_fonts()).expect("fonts");
            cx.bind_keys(bindings());
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1440.0), px(900.0)),
                    ))),
                    ..WindowOptions::default()
                },
                cx,
                |window, cx| {
                    observed.attach(window);
                    let config = StudioConfig {
                        project: project.clone(),
                        mcp: false,
                    };
                    cx.new(|cx| Studio::new(&config, window, cx).expect("open studio"))
                },
            )
            .expect("open window")
        });
        cx.run_until_parked();

        let (hero, heading, board) = cx.update(|cx| {
            let doc = &view.read(cx).editor.doc;
            let board = doc.pages[0].artboards[0].root;
            let named = |name: &str| {
                doc.descendants(board)
                    .into_iter()
                    .find(|id| doc.get(*id).and_then(|n| n.name.as_deref()) == Some(name))
                    .expect("starter layer")
            };
            let hero = named("Hero");
            let heading = doc
                .descendants(hero)
                .into_iter()
                .find(|id| doc.get(*id).is_some_and(|n| n.tag() == "h1"))
                .expect("heading");
            (hero, heading, board)
        });

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);

            // A single click selects the top-level layer, like Paper and Figma.
            click(window, cx, center(&automation, heading), 1);
            assert_eq!(view.read(cx).editor.selection, vec![hero]);

            // Double-clicking text edits it in place.
            click(window, cx, center(&automation, heading), 2);
        })
        .expect("select");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(view.read(cx).canvas.text_edit_target() == Some(heading));
            window.press("ctrl-a", cx);
            window.input("Native design", cx);
            window.press("enter", cx);
        })
        .expect("type");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let doc = &view.read(cx).editor.doc;
            assert_eq!(doc.text_content(heading).as_deref(), Some("Native design"));
            // Undo restores the text (focus returned to the canvas).
            window.press("ctrl-z", cx);
        })
        .expect("commit");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let doc = &view.read(cx).editor.doc;
            assert_eq!(
                doc.text_content(heading).as_deref(),
                Some("Design in real HTML. Ship it as native GPUI.")
            );
            // R + drag draws a rectangle inside the artboard's flow.
            window.press("r", cx);
        })
        .expect("undo");
        cx.run_until_parked();
        let before = cx.update(|cx| view.read(cx).editor.doc.children(board).len());
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            // Empty space near the artboard's bottom edge, below every section.
            let bottom = bounds(&automation, board);
            let start = point(px(bottom.x + 40.0), px(bottom.y + bottom.height - 6.0));
            window.drag(start, start + point(px(60.0), px(4.0)), cx);
        })
        .expect("draw");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let studio = view.read(cx);
            assert_eq!(studio.editor.doc.children(board).len(), before + 1);
            let rect = *studio.editor.doc.children(board).last().expect("rect");
            assert_eq!(studio.editor.selection, vec![rect]);
            assert_eq!(studio.editor.doc.display_name(rect), "Rectangle");
            // Delete removes it again.
            window.press("backspace", cx);
        })
        .expect("select rect");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(view.read(cx).editor.doc.children(board).len(), before);
        })
        .expect("drive studio");

        // Everything persisted is plain HTML.
        cx.update(|cx| {
            view.update(cx, |studio, _| studio.editor.save().expect("save"));
        });
        let html =
            std::fs::read_to_string(dir.path().join("design/artboards/landing-desktop.html"))
                .expect("artboard file");
        assert!(html.contains("Design in real HTML. Ship it as native GPUI."));
        assert!(!html.contains("data-name=\"Rectangle\""));
    }
}
