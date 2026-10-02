# GPUI Studio

GPUI Studio is an offline-first, native design canvas in the spirit of
[Paper](https://paper.design): you design on an infinite canvas of artboards,
and every layer is a real HTML element with inline CSS. The editor itself is a
native [GPUI Kit](https://gpui-kit.com) application, and your own coding agent
works on the same design beside you through
[`gpui-mcp`](https://github.com/themixednuts/gpui-mcp).

What you draw is what ships: artboards are stored as standalone `.html` files
that open in any browser, export as clean HTML/CSS (with media queries), as
PNG/SVG images, or as GPUI Rust builder code for native apps.

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
  studio.ron                  pages, artboard files, positions, connectors, variables
  artboards/*.html            one standalone HTML page per artboard
  artboards/assets/           imported images
  exports/                    PNG/SVG exports
  .gpui-studio/comments.ron   review comments
  .gpui-studio/chat.ron       chat with your agent
  .gpui-studio/versions/      version history
  .gpui-studio/cache/         materialized inline SVGs (safe to delete)
```

Each layer carries `data-id` (stable identity for undo, comments, and agents)
and optionally `data-name`, `data-hidden`, or `data-locked`. Components,
instances, prototype links, and breakpoints are also plain attributes
(`data-component`, `data-instance`, `data-ref`, `data-link`,
`data-transition`, `data-breakpoint`). Code export strips all of these. Writes
are atomic and only touch changed files; edits made on disk by other tools are
merged back into the open canvas automatically.

## Canvas

| Tool | Key | Behavior |
| --- | --- | --- |
| Select | `V` | Click selects the top-level layer under the pointer; double-click drills in and edits text; `⌘/Ctrl`-click selects the deepest layer; `Shift` adds to the selection; drag on empty space or artboard background to marquee |
| Frame | `F` / `A` | Drag to draw. Outside artboards this creates an artboard; inside a frame it inserts into flex/grid flow, or positions absolutely in block flow |
| Rectangle / Ellipse | `R` / `O` | Same as Frame, with a fill (ellipses use `border-radius: 50%`) |
| Text | `T` | Click to place text and type in place |
| Pencil | `P` | Freehand drawing, smoothed into an SVG path layer |
| Line / Arrow | `L` / `Shift+L` | Straight SVG line or arrow; `Shift` snaps to 45° |
| Connector | `X` | Drag from one layer or artboard to another to connect them |
| Image | `⌘/Ctrl+Shift+K` | Place image files; you can also drop files on the canvas or paste images |
| Hand | `H`, `Space`-drag, middle-drag | Pan |
| Comment | `C` | Pin a comment to a layer |

Dragging a selected layer moves artboards and absolutely positioned layers, and
reorders flow children, including reparenting into another frame. Moving and
resizing **snap** to the edges and centers of siblings, the parent, and other
artboards, with red guides (hold `⌘/Ctrl` to move freely). Eight handles
resize; `Shift` keeps the aspect ratio. The selected container shows its
**padding and gaps** as pink bands: drag a band to change it (`Shift`: all
sides, `Alt`: the opposite side too). **Rulers** (`Shift+R`) measure from the
selected artboard and highlight the selection.

Shortcuts: `⌘/Ctrl+Z` / `⌘/Ctrl+Shift+Z` undo/redo · `⌘/Ctrl+D` duplicate ·
`⌘/Ctrl+C/X/V` copy, cut, and paste HTML or images · `⌘/Ctrl+G` /
`⌘/Ctrl+Shift+G` group/ungroup · `Shift+A` auto layout · arrows nudge (`Shift`
×10) · `⌘/Ctrl+[`/`]` send backward/bring forward · `Alt+A/H/D` and `Alt+W/V/S`
align left/center/right and top/middle/bottom · `Alt+Shift+H/V` distribute ·
`⌘/Ctrl+Alt+K` create component · `⌘/Ctrl+Alt+B` detach instance ·
`⌘/Ctrl+Shift+E` export · `⌘/Ctrl+Alt+Enter` present · `⌘/Ctrl+J` chat ·
`Shift+1`/`Shift+2`/`Shift+0` zoom to fit, selection, or 100% · `Esc` selects
the parent · `Enter` selects children or edits text · `⌘/Ctrl+S` save now
(Studio also autosaves).

Pencil strokes, lines, and arrows are absolutely positioned inline `<svg>`
layers. Imported images are copied into `artboards/assets/` and placed in the
frame under the pointer (or as a new artboard). Connectors are page-level
annotations stored in `studio.ron`; they follow the layers they connect.

## Panels

- **Layers:** pages and the layer tree. Drag rows to reorder or move layers
  into other frames; rename, show/hide, lock.
- **Assets:** this project's components, artboard presets, and a library of
  eighteen ready-made components inserted as editable HTML.
- **Variables:** design tokens (colors, numbers, other values) shared by every
  artboard as CSS custom properties. Rename rewrites references; delete keeps
  the value inline. Fill, stroke, and text color fields bind to color variables.
- **History:** named versions and automatic checkpoints; restore (undoable) or delete.
- **Design:** name and tag; component/instance controls; align and distribute;
  position, size (Fixed/Hug/Fill), radius, opacity, clip; breakpoints
  (artboards) or constraints (absolute layers); layout (stack, auto layout,
  grid); fill, stroke, typography, shadow; image source and link; PNG/SVG
  export; and a raw CSS field.
- **Prototype:** what a click on the selected layer does, the transition, the
  shared-element name, and Present. Flows draw on the canvas as green arrows.
- **Code:** the selection as HTML, HTML + CSS classes (with media queries for
  breakpoints), or GPUI Rust.
- **Comments:** open and resolved review comments.

### Components

Make any layer a main component (`⌘/Ctrl+Alt+K`) and insert instances from
Assets. Edits to the main flow into every instance through a three-way merge,
so properties, text, and children an instance changed itself stay as
overrides. Instances keep their own placement; nested instances sync through
their parent component. Reset overrides, go to the main, or detach from the
Design panel. Components and instances are purple.

### Responsive

Add breakpoints (Tablet 768, Mobile 390) to an artboard: each is a synced copy
at that width. Desktop edits flow into breakpoints; what you change only at a
breakpoint is its override, and the HTML + CSS export emits those overrides as
`@media (max-width: …)` rules. Absolutely positioned layers can be pinned
left, right, both (stretch), or center on each axis.

### Prototyping

Link any layer to an artboard (or Back) with an instant, dissolve, or slide
transition. Present (`⌘/Ctrl+Alt+Enter`) shows artboards full size: clicks
follow links, misses flash the hotspots, arrow keys step through the page, and
layers sharing a `view-transition-name` glide between screens.

The same files work in browsers through the **View Transitions API**: every
artboard declares `@view-transition { navigation: auto }`, and a generated
script turns links into navigations between artboard files with the link's
transition as a view-transition type. Studio drops the script on import and
regenerates it on save.

### Export

PNG (1–3×) and SVG of any layer or artboard, from the Design panel, the
shortcut, or agents. The layer is laid out at 1× off-screen and drawn as SVG
from exact boxes and GPUI's own text lines, then rasterized with resvg using
the same fonts, so exports match the canvas (and closely match Chromium).

### History

Studio records a version when a project opens, every ten minutes of editing,
and before each batch of agent edits, plus any version you name. Restoring
checkpoints the current design first and is a single undo step.

## Working with your agent

Studio has no built-in model: your own coding agent (Claude Code, Codex, …)
connects through `gpui-mcp` and collaborates on the canvas with you.

- **Prompt it** from the chat panel (`⌘/Ctrl+J`); your selection is attached
  as context. Dictate with your OS's voice input into the same box. The agent
  reads messages with `read_messages` and replies with `send_message`. While
  messages wait, the title bar is labelled "Unread chat messages for the
  agent", so an agent can block on gpui-mcp's `wait_for_element` for the next
  prompt.
- **Work side by side.** The agent's own selection and status
  (`set_status`) show on the canvas as an orange outline and name tag instead of
  taking over your selection (turn on Follow to follow it). It can point at
  things with labelled annotations that follow the layout (`annotate`). Every
  agent edit appears in the activity feed.
- **Stay in control.** Pause blocks agent edits; a version is recorded before
  each batch of agent edits so you can roll back its whole session.

Install the server and add it to your MCP client:

```console
cargo install --git https://github.com/themixednuts/gpui-mcp --locked gpui-mcp-server
```

```json
{ "mcpServers": { "gpui": { "command": "gpui-mcp" } } }
```

Studio's commands (`list_app_commands` / `execute_app_command`):

| Command | Purpose |
| --- | --- |
| `get_document`, `get_tree`, `get_node`, `get_selection` | Read pages, layer trees, HTML with `data-id`s, and rendered bounds |
| `create_artboard`, `write_html`, `replace_html` | Build designs from HTML (`<style>` rules and `:root` variables are adopted, scripts dropped) |
| `update_styles`, `set_text`, `set_attributes`, `rename` | Edit layers |
| `move_node`, `duplicate_nodes`, `delete_nodes`, `align_nodes`, `distribute_nodes`, `set_constraints` | Arrange |
| `list_components`, `insert_component`, `list_project_components`, `create_component`, `create_instance`, `detach_instance`, `reset_overrides` | Components |
| `list_variables`, `set_variable`, `rename_variable`, `delete_variable` | Design variables |
| `import_image`, `draw_vector` | Images and vectors |
| `list_connections`, `connect`, `update_connection`, `delete_connection` | Diagram connectors |
| `list_links`, `set_link`, `add_breakpoint` | Prototyping and responsive design |
| `export_code`, `export_image` | HTML, HTML + CSS, GPUI Rust; PNG/SVG |
| `list_comments`, `add_comment`, `update_comment` | The review queue |
| `read_messages`, `send_message`, `set_status`, `annotate`, `clear_annotations`, `get_activity` | Collaborate with the person |
| `list_versions`, `save_version`, `restore_version` | History |
| `list_projects`, `switch_project`, `select_nodes`, `undo`, `redo`, `save` | Workspace |

Resources: `gpui-studio://document`, `…/selection`, `…/comments/active`,
`…/comments/all`, `…/components`, `…/variables`, and `…/chat`. The
live-document capability exchanges the active artboard as a complete HTML page
with revision checks. Every gpui-mcp tool also works on the Studio window:
canvas layers appear in the semantic tree with their names and roles, so
`find_elements` locates them.

`scripts/mcp_pipeline.py` drives a running Studio through the real server,
for testing the workflow without a model: `smoke` checks the integration and
`agent` acts as a scripted collaborator that answers your chat messages.

## HTML rendering engine

Artboards are rendered natively by Studio's own HTML→GPUI engine
(`src/ui/paint.rs`), which reads the same computed style the Design panel edits:

- block flow, flexbox, grid (see below), absolute and relative positioning;
- `px`, `%`, `em`/`rem`, auto margins, min/max sizes, grow/shrink/basis, gap, padding;
- backgrounds, borders (solid/dashed), per-corner radii, box shadows, opacity, overflow clipping;
- `var()` with fallbacks and nesting;
- typography: font stacks resolved to installed families, weights, italics,
  line height, alignment, decoration, `text-transform`, `white-space: nowrap`,
  and inline formatting as styled text runs;
- user-agent defaults for headings, paragraphs, and buttons;
- `<img>` (local paths relative to `artboards/`, clipped by radii), inline `<svg>`, and form controls.

`gpui-mcp-html` (html5ever + lightningcss) currently supports only Zed's GPUI,
not the gpui-pre backend GPUI Kit uses, so Studio keeps its own engine for now.

### CSS grid

GPUI exposes grid only as `grid_cols(n)` / `grid_rows(n)`, which become
`repeat(n, minmax(0, 1fr))`, plus line and span placement. Its layout engine,
Taffy, supports full grid, but GPUI does not pass track lists through, and GPUI
Kit adds no grid layout. Studio therefore parses real track lists itself:

- uniform `fr` tracks (`repeat(3, 1fr)`, `1fr 1fr`) use native GPUI grid;
- anything else (`200px 1fr`, `auto`, `%`, `minmax()`, `repeat(auto-fill |
  auto-fit, minmax(…))`, explicit `grid-template-rows` heights, and
  `grid-column` spans and start lines) is laid out by resolving the tracks to
  pixel widths and placing items row by row as flex lines. `auto-fill` /
  `auto-fit` read the container's measured width from the previous frame and
  settle on the next.

Named areas, `grid-row` placement and row spans in emulated grids, dense
packing, and `subgrid` are not emulated yet.

### Box model and limits

Taffy, and so GPUI, sizes boxes as `border-box`; Studio writes the
`box-sizing: border-box` reset into every artboard file and HTML export, so
browsers match the canvas. GPUI has no layout transforms or letter spacing, so
`transform` and `letter-spacing` are kept in the HTML but not drawn. Geist
ships as static weights because GPUI instantiates variable fonts only at their
default weight.

## Development

```console
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
```

The tests cover the CSS and variable model, grid tracks, geometry (snapping,
alignment), components and overrides, connectors, prototypes, versions,
collaboration, image export, persistence and external-edit merging, every agent
command, code export, and headless GPUI tests that drive the real shell
(selection, text editing, drawing, connectors, layers, images, spacing handles,
presenting, exporting, chatting with an agent, and workspace restore) through
native event dispatch and the same semantic tree agents use.

`design-reference/` holds the original visual specification of the earlier
HTML-shell prototype and is not used by the application.

## License

Apache-2.0. Geist and Geist Mono are licensed under the SIL Open Font License
(`assets/fonts/OFL.txt`).
