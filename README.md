# GPUI Studio

GPUI Studio is an offline-first, native design canvas in the spirit of
[Paper](https://paper.design): you design on an infinite canvas of artboards,
and every layer is a real HTML element with inline CSS. The editor itself is a
native [GPUI Kit](https://gpui-kit.com) application, and coding agents can read
and edit the same design live through
[`gpui-mcp`](https://github.com/themixednuts/gpui-mcp).

What you draw is what ships: artboards are stored as standalone `.html` files
that open in any browser, export as clean HTML/CSS, or export as GPUI Rust
builder code for native apps.

## Run

```console
cargo run                         # restores your last session (examples/welcome on first run)
cargo run -- --project ~/designs/app   # opens (or creates) any folder in a new tab
cargo run -- --no-mcp             # no agent bridge, fully local
```

An empty or new folder starts with a two-artboard landing-page design. Linux
builds need the usual GPUI native libraries (see `.github/workflows/ci.yml`).

## Workspace

Studio keeps several projects open as tabs. **Home** (the house tab, or
`⌘/Ctrl+Alt+H`) lists recent projects with Open folder and New project actions.
`⌘/Ctrl+O` opens a folder, `⌘/Ctrl+N` creates one, `⌘/Ctrl+W` closes the tab
(saving first), and `Ctrl+Tab` / `Ctrl+Shift+Tab` switch tabs. `⌘/Ctrl+\` hides
both sidebars for a full-bleed canvas; drag a sidebar edge to resize it.

Open tabs, the active tab, each project's page, zoom, and pan, the sidebar
layout, and the theme (`⌘/Ctrl+Shift+T`) are restored on the next launch. They
live in `workspace.ron` in the platform config directory (or in
`$GPUI_STUDIO_CONFIG_DIR`), never inside a project.

## Project format

```text
my-design/
  studio.ron                 pages, artboard files, canvas positions, connectors
  artboards/*.html           one standalone HTML page per artboard
  .gpui-studio/comments.ron  review comments
  .gpui-studio/cache/        materialized inline SVGs (safe to delete)
```

Each layer carries `data-id` (stable identity for undo, comments, and agents)
and optionally `data-name` (its layer name), `data-hidden`, or `data-locked`.
Code export strips these. Writes are atomic and only touch changed files. Edits
made on disk by other tools, such as an editor or an agent working on the files
directly, are merged back into the open canvas automatically.

## Canvas

| Tool | Key | Behavior |
| --- | --- | --- |
| Select | `V` | Click selects the top-level layer under the pointer; double-click drills in and edits text; `⌘/Ctrl`-click selects the deepest layer; `Shift` adds to the selection; drag on empty space or artboard background to marquee |
| Frame | `F` / `A` | Drag to draw. Outside artboards this creates an artboard; inside a frame it inserts into flex/grid flow, or positions absolutely in block flow |
| Rectangle | `R` | Same as Frame, with a fill |
| Ellipse | `O` | Same as Rectangle, with `border-radius: 50%` |
| Text | `T` | Click to place text and type in place |
| Pencil | `P` | Freehand drawing, smoothed into an SVG path layer |
| Line / Arrow | `L` / `Shift+L` | Straight SVG line or arrow; `Shift` snaps to 45° |
| Connector | `X` | Drag from one layer or artboard to another (or to empty canvas) to connect them |
| Hand | `H`, `Space`-drag, middle-drag | Pan |
| Comment | `C` | Pin a comment to a layer |

Dragging a selected layer moves artboards and absolutely positioned layers, and
reorders flow children, including reparenting into another frame, with a
drop indicator. Eight handles resize; `Shift` keeps the aspect ratio. Scroll
pans; `⌘/Ctrl` + scroll zooms around the pointer.

Shortcuts: `⌘/Ctrl+Z` / `⌘/Ctrl+Shift+Z` undo/redo · `⌘/Ctrl+D` duplicate ·
`⌘/Ctrl+C/X/V` copy, cut, and paste HTML (paste markup from anywhere to import
it as layers) · `⌘/Ctrl+G` / `⌘/Ctrl+Shift+G` group/ungroup · `Shift+A` toggle
auto layout · arrows nudge (`Shift` ×10) or reorder flow children ·
`⌘/Ctrl+[`/`]` send backward/bring forward · `Shift+1`/`Shift+2`/`Shift+0` zoom to fit,
selection, or 100% · `Esc` selects the parent · `Enter` selects children or edits
text · `⌘/Ctrl+S` save now (Studio also autosaves).

Pencil strokes, lines, and arrows are ordinary absolutely positioned inline
`<svg>` layers inside the frame you draw on (or in a transparent "Drawing"
artboard when drawn on empty canvas), so they export and open in browsers like
any other layer. Their stroke color and width are edited in the Design panel.

Connectors are page-level annotations for flows and diagrams, stored in
`studio.ron` rather than in the HTML. They attach to layers and follow them as
the layout changes, and support curved, straight, or elbow routing, an
arrowhead at the end, both ends, or neither, a color, and a label. Click a
connector to select and edit it; `Delete` removes it. Deleting a layer removes
its connectors.

## Panels

- **Layers:** pages (add, rename, delete), the layer tree in document order,
  rename, show/hide, and lock.
- **Assets:** artboard presets (Desktop, Laptop, Tablet, Mobile, Window) and
  eighteen components (Button, Button Group, Card, Badge, Alert, Toolbar,
  Avatar, Empty State, Titlebar, Tabs, Dialog, Dropdown, Dropdown Menu, Drawer,
  Scrollable, Resizable, Tooltip, Input). A component is inserted as ordinary,
  editable HTML, with no hidden linkage.
- **Design:** name and tag; X/Y/W/H; Fixed/Hug/Fill sizing; flow or absolute
  position; radius, opacity, and clip. Layout covers stack, auto layout
  (direction, wrap, a 3×3 alignment grid, space-between, gap, padding) and grid.
  Below that are fill, stroke (or vector stroke), typography (family, weight, size, line height,
  color, alignment, decoration), drop shadow, image source and link, and a raw
  CSS field for any other property.
- **Code:** the selection as HTML, HTML + CSS classes, or GPUI Rust, with copy.
- **Comments:** open and resolved review comments; click one to jump to its layer.

## Agents (MCP)

Install the server and add it to your MCP client:

```console
cargo install --git https://github.com/themixednuts/gpui-mcp --locked gpui-mcp-server
```

```json
{ "mcpServers": { "gpui": { "command": "gpui-mcp" } } }
```

Studio advertises its own commands through `list_app_commands` /
`execute_app_command`:

| Command | Purpose |
| --- | --- |
| `get_document`, `get_tree`, `get_node`, `get_selection` | Read pages, artboards, layer trees, and HTML with `data-id`s |
| `create_artboard`, `write_html`, `replace_html` | Build designs from HTML (`<style>` rules are inlined, scripts dropped) |
| `update_styles`, `set_text`, `set_attributes`, `rename` | Edit layers |
| `move_node`, `duplicate_nodes`, `delete_nodes` | Restructure |
| `list_components`, `insert_component` | Use the component library |
| `export_code` | HTML, HTML + CSS, or GPUI Rust |
| `list_comments`, `add_comment`, `update_comment` | Work the review queue |
| `list_connections`, `connect`, `update_connection`, `delete_connection` | Diagram flows between layers and artboards |
| `draw_vector` | Draw a line, arrow, or freehand path as an SVG layer |
| `list_projects`, `switch_project` | See the open projects and change the active tab |
| `select_nodes`, `undo`, `redo`, `save` | Collaborate with the person at the canvas |

Resources: `gpui-studio://document`, `gpui-studio://selection`,
`gpui-studio://comments/active`, `gpui-studio://comments/all`, and
`gpui-studio://components`. The live-document capability
(`get_live_document` / `preview_live_document`) exchanges the active artboard
as a complete HTML page with revision checks. Every gpui-mcp tool also works on
the Studio window itself: semantic tree, clicks, screenshots, and frame timing.

Agent edits use the same path as manual ones. Each is undoable, autosaved,
shown on the canvas immediately, and selected so you can see it. Commands,
resources, and the live document always address the active project tab.

## HTML rendering engine

Artboards are rendered natively by Studio's own HTML→GPUI engine
(`src/ui/paint.rs`), which reads the same computed style the Design panel edits:

- block flow, flexbox, grid (see below), absolute and relative positioning;
- `px`, `%`, `em`/`rem`, auto margins, min/max sizes, grow/shrink/basis, gap, padding;
- backgrounds, borders (solid/dashed), per-corner radii, box shadows, opacity, overflow clipping;
- typography: font stacks resolved to installed families, weights, italics,
  line height, alignment, decoration, `text-transform`, `white-space: nowrap`,
  and inline formatting (`<b>`, `<em>`, `<a>`, `<mark>`, `<br>`, styled `<span>`s)
  as styled text runs;
- user-agent defaults for headings, paragraphs, and buttons;
- `<img>` (local paths relative to `artboards/`), inline `<svg>`, and form controls.

### CSS grid

GPUI exposes grid only as `grid_cols(n)` / `grid_rows(n)`, which become
`repeat(n, minmax(0, 1fr))`, plus line and span placement. Its layout engine,
Taffy, supports full grid, but GPUI does not pass track lists through, and GPUI
Kit adds no grid layout. Studio therefore parses real track lists itself:

- uniform `fr` tracks (`repeat(3, 1fr)`, `1fr 1fr`) use native GPUI grid;
- anything else (`200px 1fr`, `auto`, `%`, `minmax()`, `repeat(auto-fill |
  auto-fit, minmax(…))`, explicit `grid-template-rows` heights, and
  `grid-column` spans and start lines) is laid out by resolving the tracks to pixel widths and
  placing items row by row, with auto placement and column spans, as rows of
  flex lines. `auto-fill` / `auto-fit` read the container's measured width from
  the previous frame and settle on the next.

Named areas, `grid-row` placement and row spans in emulated grids, dense
packing, and `subgrid` are not emulated yet; those items fall back to auto
placement.

### Box model

Taffy, and so GPUI, sizes boxes as `border-box`. Studio writes
`*, *::before, *::after { box-sizing: border-box; }` into every artboard file
and HTML export, so browsers match the canvas exactly.

### Limits

GPUI has no layout transforms or letter spacing, so `transform` and
`letter-spacing` are kept in the HTML but not drawn. Absolutely positioned layers
are placed relative to their parent; Studio adds `position: relative` to a parent
when you position a child inside it, so browsers agree. Geist ships as static
weights because GPUI instantiates variable fonts only at their default weight.

## Development

```console
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
```

The tests cover the CSS model, grid track resolution, connector routing,
HTML import/export round trips, persistence and
external-edit merging, undo, every agent command, code export, and a headless
GPUI tests. Those drive the real shell (click selection, in-place text
editing, undo, drawing, connectors, pencil strokes, workspace tabs, and session
restore) through native event dispatch, locating elements through the same
semantic tree MCP agents use.

`design-reference/` holds the original visual specification of the earlier
HTML-shell prototype and is not used by the application.

## License

Apache-2.0. Geist and Geist Mono are licensed under the SIL Open Font License
(`assets/fonts/OFL.txt`).
