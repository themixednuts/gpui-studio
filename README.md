# GPUI Studio

GPUI Studio is an offline-first native visual builder for GPUI applications. Its own shell is authored with pure HTML, local CSS, and typed RON bindings, then rendered by GPUI through `gpui-mcp-html` and HTMLSwap.

The Rust dependency graph pins the exact tested
[`gpui-mcp`](https://github.com/themixednuts/gpui-mcp) commit. Cargo may fetch
that source on the first build; after it is cached, Studio and its project data
remain local and require no network service. The `design-reference/` directory
is retained only as the original visual specification and is not loaded by the
application at runtime.

## One component document, two projections

Studio has one versioned component graph in `.gpui-studio/components.ron`. Layout, appearance,
typed props, state, semantic events, and actions belong to that graph. HTML/CSS/RON and GPUI are
editable projections of the same component—not different component types and not different canvas
runtimes:

- **HTML/CSS/RON projection** emits standards-based HTML with no HTMLSwap-only attributes, local CSS, and typed RON event/state bindings. HTMLSwap resolves that bundle into GPUI.
- **GPUI projection** emits the corresponding builder chain and event handlers from the same graph.

Changing the projection never changes the live canvas, selection, component identity, props, or
logic. Creating a component also has one path; the projection choice only decides which source view
opens first. Older `native-components.ron` files are read as a migration input and newly saved in the
neutral schema.

The graph covers Studio's live component/layout/paint/action vocabulary. Arbitrary user Rust, new
concrete GPUI element types, and new compiled application hooks remain an explicit compile boundary;
those should run in a supervised preview-host process rather than a dynamically loaded Rust ABI.

The offline component catalog currently installs seventeen complete recipes into that graph: Button,
Button Group, Card, Badge, Alert, Toolbar, Avatar, Empty State, semantic Titlebar, Tabs, Dialog,
Dropdown, Dropdown Menu, Drawer, Scrollable, Resizable, and Tooltip. Dialog already supplies the modal
contract, so the catalog intentionally does not add a duplicate Modal alias. A preset is copied into
the project as ordinary editable nodes, typography, typed props,
local state, variants, slots, and action contracts; it has no hidden runtime or read-only template
layer. Explicit logic edges take precedence over shorthand node click actions, so HTML/RON and GPUI
emit one deterministic handler per event. Actions such as `open_project` and `dismiss` remain
portable host contracts instead of embedding application-specific behavior.

## Professional canvas model

The canvas includes responsive/desktop/tablet/mobile viewports, collision-safe fit and centering,
bounded zoom, device orientation, and configurable grid snapping. Embedded HTML media queries
resolve against the selected logical viewport rather than the outer Studio window. Structured
Layout, Style, and Logic inspector surfaces and Console/States dock surfaces operate on the same
selection and component graph.

For component documents, the inspector is a two-way editor rather than a metric mock. It edits
Hug/Fill/Fixed sizing, min/max bounds, Flexbox and Grid placement, wrapping, grow/shrink/basis,
per-edge padding/margins/offsets, overflow, opacity, rotation, constraints, text, colors, radius,
font family/size/weight/line height, semantic actions and state, design tokens, typed props, local
state, variants, and slots. Inspector and MCP mutations both pass through the same revision-checked
component transaction engine. Changes repaint without Cargo and are atomically persisted after a
short quiet period so typing does not rewrite the document on every keystroke.

The editor uses an R-tree for hit testing and marquee queries, visual-order Flex/Grid insertion,
union bounds for multi-selection, start/center/end/grid smart guides, and fractional sibling order
keys for stable reordering. Logic graphs use deterministic longest-path layering with alternating
barycentric crossing reduction. Floating surfaces use flip, shift, available-size, and hidden-anchor
middleware; nested menus share a time-bounded pointer-intent safe corridor rather than ad-hoc hover
timeouts.

Hot reload uses last-good atomic replacement and stable control state. Filesystem bursts are
coalesced, byte-identical notifications do not repaint, live revision polling avoids cloning the
document, semantic selection snapshots are shared per generation, font discovery is cached, and
unchanged native text inputs skip entity updates and binding clones. MCP live preview still compiles
the complete HTML/CSS/RON candidate and checks the expected revision before a single swap.

Native OS decorations are an output-window policy, not simulated preview chrome. Windows and macOS
map custom decorations to GPUI's transparent titlebar option; Linux maps them to client-side window
decorations. A titlebar is an ordinary semantic component in the canonical graph, with editable
children and minimize/maximize/close actions. It can therefore be created and projected as either
pure `<header role="toolbar">` HTML/CSS/RON or a GPUI builder chain without changing its identity.

## Spatial review tasks

Comments are durable project tasks stored offline in `.gpui-studio/annotations.ron`. Each task records:

- stable authored ID and fully namespaced runtime ID;
- source revision;
- captured GPUI rectangle;
- normalized anchor within that rectangle;
- comment and lifecycle status: `Open`, `InProgress`, `Done`, or `Archived`.

The MCP resource handler resolves the stable runtime ID against the latest semantic tree every time a client reads the resource. This avoids handing a model stale screen coordinates after a resize or edit.

`Open` and `InProgress` tasks form the active queue. Marking a task `Done` removes it from that queue immediately while retaining it in history for accountability. All lifecycle changes use staged, flushed file replacement before Studio exposes the new state, so a failed write cannot create a UI/disk mismatch.

Studio exposes standard MCP resources when MCP is enabled:

- `gpui-studio://project/manifest`
- `gpui-studio://selection`
- `gpui-studio://tasks/active`
- `gpui-studio://tasks/history`
- `gpui-studio://theme`

Resources use JSON for MCP interoperability. Project bindings and Studio-owned editable documents use versioned RON on disk.

## Offline editor themes

Studio themes its editor chrome separately from the application being built. This keeps project preview colors honest while allowing the builder itself to use any light or dark theme.

Theme definitions are TOML and merge in deterministic precedence order:

1. bundled themes in `themes/`;
2. user themes in `~/.gpui-studio/themes/`;
3. project overrides in `<project>/.gpui-studio/themes/`.

Later files with the same normalized theme name override only the tokens they specify. Studio watches the user and project directories and reapplies changed themes without restarting or recompiling. The selected name and light/dark variant are saved atomically in `.gpui-studio/workspace.ron`.

The format follows the useful separation in `az-rs`: identity and fonts at the top level, with explicit light/dark variants and semantic color tokens below them. A minimal project override is:

```toml
name = "Foundry"

[dark.colors]
accent = "#ff7a52"
review_surface = "#281c19"
review_border = "#a64f38"
```

Theme files are bounded, validated, local-only inputs: at most 64 regular `.toml` files per catalog, 128 KiB each, no symlinks, strict color values, and a fail-closed schema. The built-in Foundry dark and Paper light themes guarantee an offline fallback.

## Run

```powershell
cargo run
```

The default project is `examples/welcome`. Use `--project PATH` to open another HTML-backed project, or `--no-mcp` for an entirely in-process offline session.

## Responsive layout contract

The outer shell, flex workspace, canvas frame, embedded component host, embedded HTML root, and inner project canvas form one explicit `height: 100%`/`min-height: 0` chain. A native GPUI regression test repeatedly resizes the real Studio shell and welcome project through 900, 650, 420, and 760 logical pixels and verifies that every boundary continues to track the window.
