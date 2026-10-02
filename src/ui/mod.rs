//! The native Studio window, built on GPUI Kit.

mod canvas;
mod chat;
mod image_export;
mod inspector;
mod paint;
mod panels;
mod present;
mod studio;
mod variables;
mod workspace;

use std::borrow::Cow;
use std::path::PathBuf;

use gpui_kit::component::{Theme, ThemeMode, TitleBar};
use gpui_kit::{
    App, AppContext as _, KeyBinding, SharedString, WindowBounds, WindowOptions, px, size,
};

pub(crate) use studio::{LeftTab, RightTab, Studio};
use workspace::Workspace;

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
            RenameSelection,
            EllipseTool,
            PencilTool,
            LineTool,
            ArrowTool,
            ConnectorTool,
            NewProject,
            CloseTab,
            NextTab,
            PreviousTab,
            ShowHome,
            TogglePanels,
            AlignLeft,
            AlignHCenter,
            AlignRight,
            AlignTop,
            AlignVCenter,
            AlignBottom,
            DistributeHorizontal,
            DistributeVertical,
            PlaceImage,
            CreateComponent,
            DetachInstance,
            StartPresenting,
            ExportSelection,
            ToggleChat
        ]
    );
}
use actions::*;

const CANVAS_CONTEXT: &str = "StudioCanvas";
const ROOT_CONTEXT: &str = "Studio";

/// Process configuration.
#[derive(Clone, Debug, Default)]
pub struct LaunchConfig {
    /// Project folder to open (created and scaffolded when missing).
    pub project: Option<PathBuf>,
    /// Project opened on the very first launch.
    pub example: Option<PathBuf>,
    /// Workspace file; defaults to the platform config directory.
    pub state_path: Option<PathBuf>,
    /// Install the local MCP bridge.
    pub mcp: bool,
}

fn bindings() -> Vec<KeyBinding> {
    let c = Some(CANVAS_CONTEXT);
    let r = Some(ROOT_CONTEXT);
    vec![
        KeyBinding::new("v", SelectTool, c),
        KeyBinding::new("f", FrameTool, c),
        KeyBinding::new("o", EllipseTool, c),
        KeyBinding::new("p", PencilTool, c),
        KeyBinding::new("l", LineTool, c),
        KeyBinding::new("shift-l", ArrowTool, c),
        KeyBinding::new("x", ConnectorTool, c),
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
        KeyBinding::new("secondary-shift-k", PlaceImage, c),
        KeyBinding::new("secondary-alt-k", CreateComponent, c),
        KeyBinding::new("secondary-alt-b", DetachInstance, c),
        KeyBinding::new("secondary-alt-enter", StartPresenting, c),
        KeyBinding::new("secondary-shift-e", ExportSelection, c),
        KeyBinding::new("alt-a", AlignLeft, c),
        KeyBinding::new("alt-h", AlignHCenter, c),
        KeyBinding::new("alt-d", AlignRight, c),
        KeyBinding::new("alt-w", AlignTop, c),
        KeyBinding::new("alt-v", AlignVCenter, c),
        KeyBinding::new("alt-s", AlignBottom, c),
        KeyBinding::new("alt-shift-h", DistributeHorizontal, c),
        KeyBinding::new("alt-shift-v", DistributeVertical, c),
        KeyBinding::new("secondary-z", UndoEdit, r),
        KeyBinding::new("secondary-shift-z", RedoEdit, r),
        KeyBinding::new("secondary-y", RedoEdit, r),
        KeyBinding::new("secondary-s", SaveProject, r),
        KeyBinding::new("secondary-o", OpenProject, r),
        KeyBinding::new("secondary-n", NewProject, r),
        KeyBinding::new("secondary-w", CloseTab, r),
        KeyBinding::new("ctrl-tab", NextTab, r),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, r),
        KeyBinding::new("secondary-alt-h", ShowHome, r),
        KeyBinding::new("secondary-\\", TogglePanels, r),
        KeyBinding::new("secondary-j", ToggleChat, r),
        KeyBinding::new("secondary-shift-t", ToggleTheme, r),
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
pub fn run(config: LaunchConfig) {
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
                cx.new(|cx| Workspace::new(&config, window, cx))
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
        Spline,
        MoveUpRight,
        Slash,
        Pencil,
        House,
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
        SquareDashed,
        AlignHorizontalSpaceAround,
        AlignVerticalSpaceAround,
        Variable,
        Unlink,
        Hexagon,
        ArrowLeft,
        Play
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

#[cfg(test)]
mod tests {
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        AppContext as _, Bounds, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
        MouseUpEvent, PlatformInput, Point, TestAppContext, Window, WindowBounds, WindowOptions,
        point, px, size,
    };

    use super::{LaunchConfig, Workspace, bindings, bundled_fonts};
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
        let state = dir.path().join("workspace.ron");
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
                    let config = LaunchConfig {
                        project: Some(project.clone()),
                        example: None,
                        state_path: Some(state.clone()),
                        mcp: false,
                    };
                    cx.new(|cx| Workspace::new(&config, window, cx))
                },
            )
            .expect("open window")
        });
        cx.run_until_parked();
        let workspace = view;
        let view = cx.update(|cx| workspace.read(cx).tabs[0].clone());

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
            window.press("secondary-a", cx);
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
            window.press("secondary-z", cx);
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

    fn boot(
        cx: &mut TestAppContext,
        launch: LaunchConfig,
    ) -> (
        gpui_kit::AnyWindowHandle,
        gpui_kit::Entity<Workspace>,
        gpui_mcp::Automation,
    ) {
        let automation = gpui_mcp::Automation::isolated();
        let observed = automation.clone();
        let (handle, workspace) = cx.update(|cx| {
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
                    cx.new(|cx| Workspace::new(&launch, window, cx))
                },
            )
            .expect("open window")
        });
        cx.run_until_parked();
        (handle, workspace, automation)
    }

    #[gpui_kit::test]
    fn workspace_tabs_home_and_session_restore(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().expect("temp dir");
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        let state = dir.path().join("workspace.ron");
        let launch = LaunchConfig {
            project: Some(a.clone()),
            example: None,
            state_path: Some(state.clone()),
            mcp: false,
        };
        let (handle, workspace, _) = boot(cx, launch);
        cx.update_window(handle, |_, window, cx| {
            workspace.update(cx, |ws, cx| {
                assert!(ws.open_path(&b, true, window, cx));
                assert_eq!(ws.tabs.len(), 2);
                // Reopening an open project focuses it instead of duplicating.
                assert!(ws.open_path(&a, true, window, cx));
                assert_eq!(ws.tabs.len(), 2);
                ws.close_tab(1, window, cx);
                assert_eq!(ws.tabs.len(), 1);
                ws.activate(None, window, cx);
                ws.persist(cx);
            });
        })
        .expect("drive workspace");
        let saved = crate::workspace::WorkspaceState::load(&state);
        assert_eq!(saved.open.len(), 1);
        assert_eq!(saved.active, None, "Home was active");
        assert_eq!(saved.recent.len(), 2);
        assert!(saved.views.contains_key(&saved.open[0]));

        // A new session restores the open tab and shows Home again.
        let (_, restored, _) = boot(
            cx,
            LaunchConfig {
                project: None,
                example: None,
                state_path: Some(state),
                mcp: false,
            },
        );
        cx.update(|cx| assert_eq!(restored.read(cx).tabs.len(), 1));
    }

    #[gpui_kit::test]
    fn connector_and_pencil_tools_create_real_objects(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().expect("temp dir");
        let (handle, workspace, automation) = boot(
            cx,
            LaunchConfig {
                project: Some(dir.path().join("design")),
                example: None,
                state_path: Some(dir.path().join("workspace.ron")),
                mcp: false,
            },
        );
        let studio = cx.update(|cx| workspace.read(cx).tabs[0].clone());
        let (desktop, mobile) = cx.update(|cx| {
            let doc = &studio.read(cx).editor.doc;
            (
                doc.pages[0].artboards[0].root,
                doc.pages[0].artboards[1].root,
            )
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            window.press("x", cx);
        })
        .expect("connector tool");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.drag(
                center(&automation, desktop),
                center(&automation, mobile),
                cx,
            );
        })
        .expect("draw connector");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let s = studio.read(cx);
            let connections = &s.editor.doc.pages[0].connections;
            assert_eq!(connections.len(), 1);
            let (from, to) = (
                connections[0].from.node().expect("attached"),
                connections[0].to.node().expect("attached"),
            );
            assert_eq!(s.editor.doc.root_of(from), desktop);
            assert_eq!(s.editor.doc.root_of(to), mobile);
            assert_eq!(s.selected_connection, Some(connections[0].id));
            window.press("backspace", cx);
        })
        .expect("delete connector");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(studio.read(cx).editor.doc.pages[0].connections.is_empty());
            window.press("p", cx);
        })
        .expect("pencil");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let start = center(&automation, desktop);
            window.drag(start, start + point(px(80.0), px(30.0)), cx);
        })
        .expect("draw stroke");
        cx.run_until_parked();
        cx.update(|cx| {
            let doc = &studio.read(cx).editor.doc;
            let vectors = doc
                .descendants(desktop)
                .into_iter()
                .filter(|id| {
                    matches!(
                        doc.get(*id).map(|n| &n.kind),
                        Some(crate::model::NodeKind::Svg(_))
                    )
                })
                .count();
            assert_eq!(
                vectors, 1,
                "the pencil stroke is an SVG layer in the artboard"
            );
        });
    }

    #[gpui_kit::test]
    fn pasting_an_image_copies_it_into_the_project(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().expect("temp dir");
        let project = dir.path().join("design");
        let (handle, workspace, automation) = boot(
            cx,
            LaunchConfig {
                project: Some(project.clone()),
                example: None,
                state_path: Some(dir.path().join("workspace.ron")),
                mcp: false,
            },
        );
        let studio = cx.update(|cx| workspace.read(cx).tabs[0].clone());
        let desktop = cx.update(|cx| studio.read(cx).editor.doc.pages[0].artboards[0].root);
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            click(window, cx, center(&automation, desktop), 1);
        })
        .expect("focus canvas");
        cx.run_until_parked();
        cx.update(|cx| {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_image(
                &gpui_kit::Image::from_bytes(
                    gpui_kit::ImageFormat::Png,
                    crate::assets::tests_png(),
                ),
            ));
        });
        cx.update_window(handle, |_, window, cx| {
            window.press("secondary-v", cx);
        })
        .expect("paste");
        cx.run_until_parked();
        cx.update(|cx| {
            let editor = &studio.read(cx).editor;
            let image = editor.primary().expect("pasted image is selected");
            let node = editor.doc.get(image).expect("node");
            assert_eq!(node.tag(), "img");
            assert_eq!(node.attr("src"), Some("assets/pasted-image.png"));
            assert_eq!(editor.doc.root_of(image), desktop);
        });
        assert!(project.join("artboards/assets/pasted-image.png").exists());
    }

    #[gpui_kit::test]
    fn presenting_follows_prototype_links(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().expect("temp dir");
        let (handle, workspace, automation) = boot(
            cx,
            LaunchConfig {
                project: Some(dir.path().join("design")),
                example: None,
                state_path: Some(dir.path().join("workspace.ron")),
                mcp: false,
            },
        );
        let studio = cx.update(|cx| workspace.read(cx).tabs[0].clone());
        let (desktop, mobile, button) = cx.update(|cx| {
            studio.update(cx, |s, _| {
                let doc = &s.editor.doc;
                let desktop = doc.pages[0].artboards[0].root;
                let mobile = doc.pages[0].artboards[1].root;
                let button = doc
                    .descendants(desktop)
                    .into_iter()
                    .find(|id| doc.get(*id).is_some_and(|n| n.tag() == "button"))
                    .expect("starter button");
                s.editor
                    .set_link(
                        button,
                        Some(crate::model::prototype::LinkTarget::Artboard(mobile)),
                        crate::model::prototype::Transition::Dissolve,
                    )
                    .expect("link");
                s.editor.select([desktop]);
                (desktop, mobile, button)
            })
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            studio.update(cx, |s, cx| s.start_present(window, cx));
        })
        .expect("present");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            assert_eq!(
                studio.read(cx).present.as_ref().map(|p| p.current),
                Some(desktop)
            );
            click(window, cx, center(&automation, button), 1);
        })
        .expect("click hotspot");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                studio.read(cx).present.as_ref().map(|p| p.current),
                Some(mobile)
            );
            window.press("left", cx);
        })
        .expect("back");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                studio.read(cx).present.as_ref().map(|p| p.current),
                Some(desktop)
            );
            window.press("escape", cx);
        })
        .expect("exit");
        cx.run_until_parked();
        cx.update(|cx| assert!(studio.read(cx).present.is_none()));
    }

    #[gpui_kit::test]
    fn exporting_an_artboard_writes_svg_and_png(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().expect("temp dir");
        let project = dir.path().join("design");
        let (handle, workspace, _) = boot(
            cx,
            LaunchConfig {
                project: Some(project.clone()),
                example: None,
                state_path: Some(dir.path().join("workspace.ron")),
                mcp: false,
            },
        );
        let studio = cx.update(|cx| workspace.read(cx).tabs[0].clone());
        for format in [
            crate::export_image::ImageFormat::Svg,
            crate::export_image::ImageFormat::Png,
        ] {
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                studio.update(cx, |s, cx| {
                    let board = s.editor.doc.pages[0].artboards[0].root;
                    let path = s.export_path(board, format, 1.0).expect("project path");
                    s.start_image_export(
                        board,
                        format,
                        1.0,
                        super::image_export::ExportTarget::File(path),
                        cx,
                    );
                });
            })
            .expect("start export");
            for _ in 0..6 {
                cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                    .expect("frame");
                cx.run_until_parked();
            }
            cx.update(|cx| {
                eprintln!(
                    "DEBUG job pending: {}",
                    studio.read(cx).export_job.is_some()
                )
            });
        }
        let svg = std::fs::read_to_string(project.join("exports/landing-desktop.svg"))
            .expect("svg written");
        assert!(
            svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1280\""),
            "{svg}"
        );
        assert!(
            svg.contains("fill=\"#ff5a36\""),
            "the primary button is drawn"
        );
        let png = std::fs::read(project.join("exports/landing-desktop.png")).expect("png written");
        assert_eq!(
            crate::assets::image_size(&png, "png"),
            Some((1280.0, 820.0))
        );
        cx.update(|cx| assert!(studio.read(cx).export_job.is_none()));
    }

    #[gpui_kit::test]
    fn chatting_with_an_agent_while_it_works_beside_you(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().expect("temp dir");
        let (handle, workspace, automation) = boot(
            cx,
            LaunchConfig {
                project: Some(dir.path().join("design")),
                example: None,
                state_path: Some(dir.path().join("workspace.ron")),
                mcp: false,
            },
        );
        let studio = cx.update(|cx| workspace.read(cx).tabs[0].clone());
        let (desktop, mobile) = cx.update(|cx| {
            let doc = &studio.read(cx).editor.doc;
            (
                doc.pages[0].artboards[0].root,
                doc.pages[0].artboards[1].root,
            )
        });
        // The person selects the mobile artboard, opens chat, and asks.
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            studio.update(cx, |s, _| s.editor.select([mobile]));
            window.press("secondary-j", cx);
        })
        .expect("open chat");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.input("Warm up the desktop hero", cx);
            window.press("enter", cx);
        })
        .expect("send");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("frame");
        let label = |automation: &gpui_mcp::Automation, text: &str| {
            automation.snapshot().nodes.values().any(|n| {
                n.label.as_deref().is_some_and(|l| l.contains(text))
                    || n.text.as_ref().is_some_and(|t| t.text.contains(text))
            })
        };
        cx.update(|cx| {
            let collab = &studio.read(cx).editor.collab;
            assert_eq!(collab.messages().len(), 1);
            assert_eq!(collab.messages()[0].nodes, vec![mobile]);
            assert_eq!(collab.unread_for_agent(), 1);
        });
        assert!(
            label(&automation, "Unread chat messages for the agent"),
            "the agent can wait for this badge"
        );

        // The agent reads, focuses on the desktop artboard, edits, and replies.
        cx.update(|cx| {
            studio.update(cx, |s, cx| {
                let e = &mut s.editor;
                crate::agent::execute(e, "read_messages", serde_json::json!({ "agent": "Claude" })).expect("read");
                crate::agent::execute(e, "set_status", serde_json::json!({ "status": "Warming", "node_ids": [desktop.to_string()] })).expect("status");
                crate::agent::execute(e, "update_styles", serde_json::json!({ "node_ids": [desktop.to_string()], "styles": { "background-color": "#fff1e6" } })).expect("edit");
                crate::agent::execute(e, "send_message", serde_json::json!({ "text": "Warmed it up." })).expect("reply");
                cx.notify();
            });
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("frame");
        cx.update(|cx| {
            let s = studio.read(cx);
            assert_eq!(
                s.editor.selection,
                vec![mobile],
                "the person's selection is untouched"
            );
            assert_eq!(s.editor.collab.unread_for_agent(), 0);
        });
        assert!(
            label(&automation, "Warmed it up."),
            "the reply shows in the chat"
        );
        assert!(
            label(&automation, "Claude · Warming"),
            "the agent's presence shows"
        );
        assert!(!label(&automation, "Unread chat messages for the agent"));
    }
}
