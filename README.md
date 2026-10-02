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
cargo run                         # opens examples/welcome
cargo run -- --project ~/designs/app   # opens (or creates) any folder
cargo run -- --no-mcp             # no agent bridge, fully local
```

An empty or new folder starts with a two-artboard landing-page design. Linux
builds need the usual GPUI native libraries (see `.github/workflows/ci.yml`).

## Project format

```text
my-design/
  studio.ron                 pages, artboard files, canvas positions
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
| Text | `T` | Click to place text and type in place |
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
  Below that are fill, stroke, typography (family, weight, size, line height,
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
| `select_nodes`, `undo`, `redo`, `save` | Collaborate with the person at the canvas |

Resources: `gpui-studio://document`, `gpui-studio://selection`,
`gpui-studio://comments/active`, `gpui-studio://comments/all`, and
`gpui-studio://components`. The live-document capability
(`get_live_document` / `preview_live_document`) exchanges the active artboard
as a complete HTML page with revision checks. Every gpui-mcp tool also works on
the Studio window itself: semantic tree, clicks, screenshots, and frame timing.

Agent edits use the same path as manual ones. Each is undoable, autosaved,
shown on the canvas immediately, and selected so you can see it.

## HTML rendering engine

Artboards are rendered natively by Studio's own HTML→GPUI engine
(`src/ui/paint.rs`), which reads the same computed style the Design panel edits:

- block flow, flexbox, grid (equal tracks and spans), absolute and relative positioning;
- `px`, `%`, `em`/`rem`, auto margins, min/max sizes, grow/shrink/basis, gap, padding;
- backgrounds, borders (solid/dashed), per-corner radii, box shadows, opacity, overflow clipping;
- typography: font stacks resolved to installed families, weights, italics,
  line height, alignment, decoration, `text-transform`, `white-space: nowrap`,
  and inline formatting (`<b>`, `<em>`, `<a>`, `<mark>`, `<br>`, styled `<span>`s)
  as styled text runs;
- user-agent defaults for headings, paragraphs, and buttons;
- `<img>` (local paths relative to `artboards/`), inline `<svg>`, and form controls.

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

The tests cover the CSS model, HTML import/export round trips, persistence and
external-edit merging, undo, every agent command, code export, and a headless
GPUI test. That test drives the real shell (click selection, in-place text
editing, undo, drawing, deleting) through native event dispatch, locating
elements through the same semantic tree MCP agents use.

`design-reference/` holds the original visual specification of the earlier
HTML-shell prototype and is not used by the application.

## License

Apache-2.0. Geist and Geist Mono are licensed under the SIL Open Font License
(`assets/fonts/OFL.txt`).
