//! Insertable components, artboard sizes, and the starter design.
//!
//! Components are plain HTML snippets with inline styles. Inserting one copies
//! ordinary, fully editable nodes into the design; nothing stays linked.

use crate::model::html::{ImportOptions, parse_document};
use crate::model::{Document, NodeId};

/// An artboard size preset.
#[derive(Clone, Copy, Debug)]
pub struct ArtboardPreset {
    /// Display name.
    pub name: &'static str,
    /// Width in px.
    pub width: f32,
    /// Height in px.
    pub height: f32,
}

/// Common device sizes.
pub const ARTBOARD_PRESETS: &[ArtboardPreset] = &[
    ArtboardPreset {
        name: "Desktop",
        width: 1440.0,
        height: 900.0,
    },
    ArtboardPreset {
        name: "Laptop",
        width: 1280.0,
        height: 800.0,
    },
    ArtboardPreset {
        name: "Tablet",
        width: 834.0,
        height: 1194.0,
    },
    ArtboardPreset {
        name: "Mobile",
        width: 390.0,
        height: 844.0,
    },
    ArtboardPreset {
        name: "Window",
        width: 960.0,
        height: 640.0,
    },
];

/// One insertable component.
#[derive(Clone, Copy, Debug)]
pub struct Component {
    /// Stable machine name (used by MCP).
    pub key: &'static str,
    /// Display name.
    pub name: &'static str,
    /// One-line description.
    pub description: &'static str,
    /// HTML with inline styles.
    pub html: &'static str,
}

const FONT: &str = "font-family: Geist, sans-serif";

/// The component library.
pub const COMPONENTS: &[Component] = &[
    Component {
        key: "button",
        name: "Button",
        description: "Primary action",
        html: r#"<button data-name="Button" style="display: flex; align-items: center; justify-content: center; gap: 8px; padding: 10px 18px; background-color: #111827; color: #ffffff; border-radius: 8px; font-size: 14px; font-weight: 600; border: 0px solid transparent">Get started</button>"#,
    },
    Component {
        key: "button_group",
        name: "Button Group",
        description: "Segmented actions",
        html: r#"<div data-name="Button Group" style="display: flex; padding: 3px; gap: 2px; background-color: #f3f4f6; border-radius: 10px"><button style="border: none; padding: 6px 14px; background-color: #ffffff; color: #111827; border-radius: 8px; font-size: 13px; font-weight: 600; box-shadow: 0 1px 2px rgba(0,0,0,0.08)">Day</button><button style="border: none; padding: 6px 14px; color: #6b7280; border-radius: 8px; font-size: 13px; font-weight: 500">Week</button><button style="border: none; padding: 6px 14px; color: #6b7280; border-radius: 8px; font-size: 13px; font-weight: 500">Month</button></div>"#,
    },
    Component {
        key: "card",
        name: "Card",
        description: "Content with actions",
        html: r#"<article data-name="Card" style="display: flex; flex-direction: column; gap: 12px; width: 320px; padding: 20px; background-color: #ffffff; border: 1px solid #e5e7eb; border-radius: 14px; box-shadow: 0 8px 24px rgba(15,23,42,0.06)"><h3 style="margin: 0; font-size: 17px; font-weight: 650; color: #111827">Quarterly report</h3><p style="margin: 0; font-size: 14px; line-height: 21px; color: #6b7280">Revenue grew 18% on strong retention across every plan.</p><div style="display: flex; gap: 8px; justify-content: flex-end"><button style="padding: 8px 14px; border: 1px solid #e5e7eb; border-radius: 8px; font-size: 13px; color: #111827">Dismiss</button><button style="border: none; padding: 8px 14px; background-color: #4f46e5; color: #ffffff; border-radius: 8px; font-size: 13px; font-weight: 600">Open</button></div></article>"#,
    },
    Component {
        key: "badge",
        name: "Badge",
        description: "Compact status",
        html: r#"<span data-name="Badge" style="display: flex; align-items: center; gap: 6px; padding: 3px 10px; background-color: #ecfdf5; color: #047857; border: 1px solid #a7f3d0; border-radius: 999px; font-size: 12px; font-weight: 600">● Live</span>"#,
    },
    Component {
        key: "alert",
        name: "Alert",
        description: "Dismissible message",
        html: r#"<div data-name="Alert" role="alert" style="display: flex; gap: 12px; align-items: flex-start; width: 420px; padding: 14px 16px; background-color: #fffbeb; border: 1px solid #fde68a; border-radius: 12px"><span style="font-size: 16px">⚠</span><div style="display: flex; flex-direction: column; gap: 4px; flex-grow: 1"><strong style="font-size: 14px; color: #92400e">Storage almost full</strong><p style="margin: 0; font-size: 13px; line-height: 19px; color: #b45309">You have used 92% of your plan. Upgrade to keep syncing.</p></div><button style="border: none; font-size: 14px; color: #b45309">✕</button></div>"#,
    },
    Component {
        key: "toolbar",
        name: "Toolbar",
        description: "Command group",
        html: r#"<div data-name="Toolbar" role="toolbar" style="display: flex; align-items: center; gap: 4px; padding: 6px; background-color: #ffffff; border: 1px solid #e5e7eb; border-radius: 12px; box-shadow: 0 2px 8px rgba(0,0,0,0.06)"><button style="border: none; padding: 6px 10px; border-radius: 8px; font-size: 13px; font-weight: 700; color: #111827">B</button><button style="border: none; padding: 6px 10px; border-radius: 8px; font-size: 13px; font-style: italic; color: #111827">I</button><button style="border: none; padding: 6px 10px; border-radius: 8px; font-size: 13px; text-decoration: underline; color: #111827">U</button><div style="width: 1px; height: 20px; background-color: #e5e7eb"></div><button style="border: none; padding: 6px 10px; border-radius: 8px; font-size: 13px; color: #111827">Link</button></div>"#,
    },
    Component {
        key: "avatar",
        name: "Avatar",
        description: "Initials identity",
        html: r#"<div data-name="Avatar" style="display: flex; align-items: center; justify-content: center; width: 40px; height: 40px; background-color: #e0e7ff; color: #4338ca; border-radius: 999px; font-size: 15px; font-weight: 650">JF</div>"#,
    },
    Component {
        key: "empty_state",
        name: "Empty State",
        description: "Recovery action",
        html: r#"<section data-name="Empty State" style="display: flex; flex-direction: column; align-items: center; gap: 10px; width: 360px; padding: 32px; border: 1px dashed #d1d5db; border-radius: 16px"><div style="display: flex; align-items: center; justify-content: center; width: 48px; height: 48px; background-color: #f3f4f6; border-radius: 12px; font-size: 22px">📂</div><strong style="font-size: 15px; color: #111827">No projects yet</strong><p style="margin: 0; font-size: 13px; color: #6b7280; text-align: center">Create a project to start designing with your team.</p><button style="border: none; padding: 8px 14px; background-color: #111827; color: #ffffff; border-radius: 8px; font-size: 13px; font-weight: 600">New project</button></section>"#,
    },
    Component {
        key: "titlebar",
        name: "Titlebar",
        description: "Window chrome",
        html: r#"<header data-name="Titlebar" role="toolbar" style="display: flex; align-items: center; justify-content: space-between; width: 640px; height: 40px; padding: 0 12px; background-color: #f9fafb; border-bottom: 1px solid #e5e7eb"><div style="display: flex; gap: 8px"><div style="width: 12px; height: 12px; background-color: #ff5f57; border-radius: 999px"></div><div style="width: 12px; height: 12px; background-color: #febc2e; border-radius: 999px"></div><div style="width: 12px; height: 12px; background-color: #28c840; border-radius: 999px"></div></div><span style="font-size: 13px; font-weight: 600; color: #374151">Untitled</span><div style="width: 52px"></div></header>"#,
    },
    Component {
        key: "tabs",
        name: "Tabs",
        description: "Tab list and panel",
        html: r#"<div data-name="Tabs" style="display: flex; flex-direction: column; gap: 12px; width: 420px"><div role="tablist" style="display: flex; gap: 20px; border-bottom: 1px solid #e5e7eb"><span role="tab" style="padding: 8px 0; font-size: 14px; font-weight: 600; color: #111827; border-bottom: 2px solid #111827">Overview</span><span role="tab" style="padding: 8px 0; font-size: 14px; color: #6b7280">Activity</span><span role="tab" style="padding: 8px 0; font-size: 14px; color: #6b7280">Settings</span></div><p role="tabpanel" style="margin: 0; font-size: 14px; line-height: 21px; color: #4b5563">Overview content goes here.</p></div>"#,
    },
    Component {
        key: "dialog",
        name: "Dialog",
        description: "Modal surface",
        html: r#"<div data-name="Dialog" role="dialog" style="display: flex; flex-direction: column; gap: 16px; width: 420px; padding: 24px; background-color: #ffffff; border-radius: 16px; box-shadow: 0 24px 64px rgba(15,23,42,0.18)"><div style="display: flex; flex-direction: column; gap: 6px"><h2 style="margin: 0; font-size: 18px; font-weight: 650; color: #111827">Delete project?</h2><p style="margin: 0; font-size: 14px; line-height: 21px; color: #6b7280">This permanently removes the project and its files.</p></div><div style="display: flex; justify-content: flex-end; gap: 8px"><button style="padding: 9px 16px; border: 1px solid #e5e7eb; border-radius: 8px; font-size: 14px; color: #111827">Cancel</button><button style="border: none; padding: 9px 16px; background-color: #dc2626; color: #ffffff; border-radius: 8px; font-size: 14px; font-weight: 600">Delete</button></div></div>"#,
    },
    Component {
        key: "dropdown",
        name: "Dropdown",
        description: "Select trigger",
        html: r#"<div data-name="Dropdown" style="display: flex; align-items: center; justify-content: space-between; width: 240px; padding: 9px 12px; background-color: #ffffff; border: 1px solid #d1d5db; border-radius: 8px; font-size: 14px; color: #111827"><span>Select a plan</span><span style="color: #9ca3af">⌄</span></div>"#,
    },
    Component {
        key: "dropdown_menu",
        name: "Dropdown Menu",
        description: "Action list",
        html: r#"<div data-name="Dropdown Menu" role="menu" style="display: flex; flex-direction: column; width: 200px; padding: 6px; background-color: #ffffff; border: 1px solid #e5e7eb; border-radius: 12px; box-shadow: 0 12px 32px rgba(15,23,42,0.12)"><div role="menuitem" style="padding: 8px 10px; border-radius: 6px; font-size: 13px; color: #111827; background-color: #f3f4f6">Rename</div><div role="menuitem" style="padding: 8px 10px; border-radius: 6px; font-size: 13px; color: #111827">Duplicate</div><div style="height: 1px; margin: 4px 0; background-color: #e5e7eb"></div><div role="menuitem" style="padding: 8px 10px; border-radius: 6px; font-size: 13px; color: #dc2626">Delete</div></div>"#,
    },
    Component {
        key: "drawer",
        name: "Drawer",
        description: "Side sheet",
        html: r#"<aside data-name="Drawer" style="display: flex; flex-direction: column; gap: 16px; width: 320px; height: 480px; padding: 20px; background-color: #ffffff; border-left: 1px solid #e5e7eb; box-shadow: -12px 0 32px rgba(15,23,42,0.08)"><div style="display: flex; justify-content: space-between; align-items: center"><strong style="font-size: 16px; color: #111827">Filters</strong><span style="color: #9ca3af">✕</span></div><p style="margin: 0; font-size: 13px; color: #6b7280">Narrow results by status, owner, and date.</p></aside>"#,
    },
    Component {
        key: "scrollable",
        name: "Scrollable",
        description: "Bounded list",
        html: r#"<div data-name="Scrollable" style="display: flex; flex-direction: column; gap: 8px; width: 280px; height: 200px; padding: 12px; overflow-y: auto; border: 1px solid #e5e7eb; border-radius: 12px"><div style="padding: 10px; background-color: #f9fafb; border-radius: 8px; font-size: 13px">Item one</div><div style="padding: 10px; background-color: #f9fafb; border-radius: 8px; font-size: 13px">Item two</div><div style="padding: 10px; background-color: #f9fafb; border-radius: 8px; font-size: 13px">Item three</div><div style="padding: 10px; background-color: #f9fafb; border-radius: 8px; font-size: 13px">Item four</div><div style="padding: 10px; background-color: #f9fafb; border-radius: 8px; font-size: 13px">Item five</div></div>"#,
    },
    Component {
        key: "resizable",
        name: "Resizable",
        description: "Two-pane layout",
        html: r#"<div data-name="Resizable" style="display: flex; width: 520px; height: 260px; border: 1px solid #e5e7eb; border-radius: 12px; overflow: hidden"><div style="width: 180px; padding: 14px; background-color: #f9fafb; font-size: 13px; color: #374151">Sidebar</div><div style="width: 4px; background-color: #e5e7eb"></div><div style="flex-grow: 1; padding: 14px; font-size: 13px; color: #374151">Content</div></div>"#,
    },
    Component {
        key: "tooltip",
        name: "Tooltip",
        description: "Contextual hint",
        html: r#"<div data-name="Tooltip" role="tooltip" style="padding: 6px 10px; background-color: #111827; color: #f9fafb; border-radius: 6px; font-size: 12px; box-shadow: 0 4px 12px rgba(0,0,0,0.2)">Copy to clipboard</div>"#,
    },
    Component {
        key: "input",
        name: "Input",
        description: "Labeled text field",
        html: r#"<label data-name="Input" style="display: flex; flex-direction: column; gap: 6px; width: 280px"><span style="font-size: 13px; font-weight: 600; color: #374151">Email</span><div style="padding: 9px 12px; background-color: #ffffff; border: 1px solid #d1d5db; border-radius: 8px; font-size: 14px; color: #9ca3af">you@example.com</div></label>"#,
    },
];

/// Lookup by key.
#[must_use]
pub fn component(key: &str) -> Option<&'static Component> {
    COMPONENTS.iter().find(|c| c.key == key)
}

const DESKTOP: &str = r##"<!doctype html><html><head><title>Landing — Desktop</title></head><body>
<main data-name="Landing — Desktop" style="position: relative; display: flex; flex-direction: column; width: 1280px; height: 820px; background-color: #fbfaf7; overflow: hidden; font-family: Geist, sans-serif; color: #17181c">
  <header data-name="Nav" style="display: flex; align-items: center; justify-content: space-between; padding: 22px 56px">
    <div data-name="Logo" style="display: flex; align-items: center; gap: 10px"><div style="width: 22px; height: 22px; background-color: #ff5a36; border-radius: 6px"></div><strong style="font-size: 17px; font-weight: 700">Northwind</strong></div>
    <nav style="display: flex; gap: 28px; font-size: 14px; color: #5b5e66"><span>Product</span><span>Customers</span><span>Pricing</span><span>Docs</span></nav>
    <button style="border: none; padding: 9px 16px; background-color: #17181c; color: #ffffff; border-radius: 999px; font-size: 14px; font-weight: 600">Sign in</button>
  </header>
  <section data-name="Hero" style="display: flex; flex-direction: column; align-items: center; gap: 22px; padding: 72px 56px 40px 56px">
    <span style="padding: 5px 12px; background-color: #ffe9e2; color: #c2410c; border-radius: 999px; font-size: 13px; font-weight: 600">New · Agents can edit your designs</span>
    <h1 style="margin: 0; width: 820px; font-size: 64px; line-height: 70px; font-weight: 700; text-align: center">Design in real HTML. Ship it as native GPUI.</h1>
    <p style="margin: 0; width: 600px; font-size: 19px; line-height: 29px; color: #5b5e66; text-align: center">Every layer on this canvas is an HTML element with inline CSS, so what you draw is exactly what your app renders.</p>
    <div data-name="Actions" style="display: flex; gap: 12px; padding-top: 8px"><button style="border: none; padding: 13px 22px; background-color: #ff5a36; color: #ffffff; border-radius: 12px; font-size: 16px; font-weight: 600">Start designing</button><button style="padding: 13px 22px; background-color: #ffffff; border: 1px solid #e3e1db; border-radius: 12px; font-size: 16px; font-weight: 600">Read the docs</button></div>
  </section>
  <section data-name="Features" style="display: flex; gap: 20px; padding: 24px 56px">
    <article style="display: flex; flex-direction: column; gap: 8px; flex-grow: 1; padding: 22px; background-color: #ffffff; border: 1px solid #ecebe6; border-radius: 16px"><strong style="font-size: 16px">Real layout</strong><p style="margin: 0; font-size: 14px; line-height: 21px; color: #5b5e66">Flexbox, grid, padding and gaps — the same engine at design and run time.</p></article>
    <article style="display: flex; flex-direction: column; gap: 8px; flex-grow: 1; padding: 22px; background-color: #ffffff; border: 1px solid #ecebe6; border-radius: 16px"><strong style="font-size: 16px">Agent ready</strong><p style="margin: 0; font-size: 14px; line-height: 21px; color: #5b5e66">MCP tools let coding agents read, write, and restyle any layer.</p></article>
    <article style="display: flex; flex-direction: column; gap: 8px; flex-grow: 1; padding: 22px; background-color: #ffffff; border: 1px solid #ecebe6; border-radius: 16px"><strong style="font-size: 16px">Offline first</strong><p style="margin: 0; font-size: 14px; line-height: 21px; color: #5b5e66">Plain files in your repo. No account, no cloud, no lock-in.</p></article>
  </section>
</main>
</body></html>"##;

const MOBILE: &str = r##"<!doctype html><html><head><title>Landing — Mobile</title></head><body>
<main data-name="Landing — Mobile" style="position: relative; display: flex; flex-direction: column; gap: 20px; width: 390px; height: 820px; padding: 20px; background-color: #fbfaf7; overflow: hidden; font-family: Geist, sans-serif; color: #17181c">
  <header style="display: flex; align-items: center; justify-content: space-between"><div style="display: flex; align-items: center; gap: 8px"><div style="width: 20px; height: 20px; background-color: #ff5a36; border-radius: 6px"></div><strong style="font-size: 16px">Northwind</strong></div><span style="font-size: 20px">☰</span></header>
  <h1 style="margin: 0; padding-top: 24px; font-size: 38px; line-height: 42px; font-weight: 700">Design in real HTML.</h1>
  <p style="margin: 0; font-size: 16px; line-height: 24px; color: #5b5e66">Every layer is an HTML element, so what you draw is exactly what your app renders.</p>
  <button style="border: none; padding: 14px; background-color: #ff5a36; color: #ffffff; border-radius: 12px; font-size: 16px; font-weight: 600; text-align: center">Start designing</button>
  <article style="display: flex; flex-direction: column; gap: 6px; padding: 18px; background-color: #ffffff; border: 1px solid #ecebe6; border-radius: 16px"><strong style="font-size: 15px">Agent ready</strong><p style="margin: 0; font-size: 14px; line-height: 20px; color: #5b5e66">MCP tools let coding agents edit any layer.</p></article>
</main>
</body></html>"##;

/// The design a new project starts with.
#[must_use]
pub fn starter_document() -> Document {
    let mut doc = Document::new();
    let options = ImportOptions { keep_ids: false };
    let desktop = parse_document(&mut doc, DESKTOP, options);
    doc.add_artboard(0, desktop, 0.0, 0.0);
    let mobile = parse_document(&mut doc, MOBILE, options);
    doc.add_artboard(0, mobile, 1360.0, 0.0);
    doc
}

/// Style for a fresh drawn frame.
#[must_use]
pub fn frame_style(width: f32, height: f32) -> String {
    format!(
        "width: {}px; height: {}px; background-color: #ffffff; border: 1px solid #e3e1db",
        crate::model::style::fmt_num(width),
        crate::model::style::fmt_num(height)
    )
}

/// Style for a fresh drawn rectangle.
#[must_use]
pub fn rectangle_style(width: f32, height: f32) -> String {
    format!(
        "width: {}px; height: {}px; background-color: #d9d9d9",
        crate::model::style::fmt_num(width),
        crate::model::style::fmt_num(height)
    )
}

/// Style for a fresh text layer.
#[must_use]
pub fn text_style() -> String {
    format!("{FONT}; font-size: 16px; color: #17181c")
}

/// Every component parses into exactly one root.
#[must_use]
pub fn instantiate(doc: &mut Document, component: &Component) -> Option<NodeId> {
    let roots =
        crate::model::html::parse_fragment(doc, component.html, ImportOptions { keep_ids: false });
    match roots.as_slice() {
        [root] => Some(*root),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_and_starter_parse() {
        let mut doc = Document::new();
        for component in COMPONENTS {
            let root = instantiate(&mut doc, component)
                .unwrap_or_else(|| panic!("{} must have one root", component.key));
            assert_eq!(doc.get(root).unwrap().name.as_deref(), Some(component.name));
        }
        let starter = starter_document();
        assert_eq!(starter.pages[0].artboards.len(), 2);
        assert!(starter.len() > 30);
    }
}
