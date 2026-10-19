# DESIGN:OS Studio — Agent Rules & Tool Pack

## Build-Verify-Fix Workflow

Follow this 7-step loop on every design task in DESIGN:OS Studio:

1. **Orient** — Call `designos_get_document` and `designos_list_nodes` to inspect existing pages, frames, variables, shared styles, and components before creating or modifying anything.
2. **Ground** — Call `designos_get_variables` and `designos_get_guidelines` with `topic: "design-system"` to load the file's token sources, component sets, variants, and slot definitions. Never invent raw hex colours, off-scale pixel gaps, or ad-hoc rectangles when a variable, style, or component exists.
3. **Plan** — Pick a vertical archetype (`designos_get_guidelines` with `topic: "archetypes"`) or surface guide (`topic: "web-app"`, `"mobile-app"`, `"landing-page"`, `"slides"`, `"table"`) when starting a new screen. Place new top-level frames beside existing frames on the active page — never stack a new screen at `(0, 0)` on top of an existing frame.

4. **Build in sections** — Pass `in_progress: true` on intermediate `designos_apply_spec` / `designos_apply_batch` calls and `in_progress: false` (or omit) on the final call of a task. Prefer one `designos_apply_spec` call per screen section (header, navigation, main card, footer) or one `designos_apply_batch` call per multi-step edit over dozens of single-node calls.
5. **Read `issues` on every write** — Every mutating tool response includes `issues: {total, new_in_this_call, items}`. Fix `error`-severity items (`new_in_this_call > 0`) before moving to the next section. When a tool returns an error with `safe_to_retry: true`, apply its `corrected_example` or `valid_options` and retry once; when `safe_to_retry: false`, stop and follow `next_step`.

6. **Visual check** — Call `designos_render_png` on the finished frame and inspect the rendered PNG image for visual hierarchy, alignment, text clipping, and contrast that geometry rules alone cannot catch.
7. **Final audit** — Call `designos_lint` with `preset: "agent"` (or `"strict"` / `"marketing"` when appropriate) and resolve all remaining error-severity findings before reporting completion.

## Enabled MCP Tools

### `designos_get_document`
- **Purpose:** Summarize the current in-memory document: pages, counts, shared styles (fill, text, effect, grid) with their values, and undo state.
- **Args:** required: none
- **Errors:** `INVALID_ARGUMENTS`, `TOOL_DISABLED`
- **Example:** `{}`

### `designos_list_nodes`
- **Purpose:** List node summaries in numeric ID order. Query matches names case-insensitively; parent_id filters direct children. Use next_offset to paginate.
- **Args:** required: none (optional: `limit`, `offset`, `parent_id`, `query`)
- **Errors:** `INVALID_ARGUMENTS`, `VALUE_OUT_OF_RANGE`, `NODE_NOT_FOUND`, `TEXT_TOO_LONG`, `TOOL_DISABLED`
- **Example:** `{"parent_id":1,"limit":50}`

### `designos_get_node`
- **Purpose:** Read the full native node including geometry, paints, text, bindings, and child IDs.
- **Args:** required: `node_id`
- **Errors:** `INVALID_ARGUMENTS`, `NODE_NOT_FOUND`, `TOOL_DISABLED`
- **Example:** `{"node_id":2}`

### `designos_get_variables`
- **Purpose:** Read variable definitions and collections. With node_id, also resolve that node's direct bindings using inherited collection modes.
- **Args:** required: none (optional: `node_id`)
- **Errors:** `INVALID_ARGUMENTS`, `NODE_NOT_FOUND`, `TOOL_DISABLED`
- **Example:** `{"node_id":2}`

### `designos_generate_code`
- **Purpose:** Generate developer handoff code or SVG for a layer using resolved variable values. Returns source text; writes no files.
- **Args:** required: `node_id`, `language`
- **Errors:** `INVALID_ARGUMENTS`, `NODE_NOT_FOUND`, `UNKNOWN_LANGUAGE`, `TOOL_DISABLED`
- **Example:** `{"node_id":2,"language":"react"}`

### `designos_create_node`
- **Purpose:** Create one layer in a container; defaults to x/y=0, width/height=100. Supports undo. Text is only valid for text layers. Changes are in memory until saved.
- **Args:** required: `parent_id`, `kind` (optional: `height`, `lint`, `name`, `text`, `width`, `x`, `y`)
- **Errors:** `INVALID_ARGUMENTS`, `NODE_NOT_FOUND`, `NOT_A_CONTAINER`, `VALUE_OUT_OF_RANGE`, `INVALID_COLOR`, `OUTSIDE_OWNED_FRAMES`, `TOOL_DISABLED`
- **Example:** `{"parent_id":1,"kind":"frame","name":"Hero Section","width":1200,"height":640,"fill":"$easeui.bg.canvas"}`

### `designos_update_node`
- **Purpose:** Update specified fields of one non-page layer. Coordinates are parent-relative. Replacing text clears text runs and list ranges. Supports undo; changes remain in memory until saved.
- **Args:** required: `node_id` (optional: `height`, `lint`, `name`, `opacity`, `text`, `visible`, `width`, `x`, `y`)
- **Errors:** `INVALID_ARGUMENTS`, `NODE_NOT_FOUND`, `PAGE_ROOT_PROTECTED`, `MOVE_CYCLE`, `INVALID_COLOR`, `OUTSIDE_OWNED_FRAMES`, `TOOL_DISABLED`
- **Example:** `{"node_id":2,"name":"Updated Header","fill":"$easeui.bg.panel"}`

### `designos_delete_node`
- **Purpose:** Remove a non-page layer and its entire subtree. Supports undo; changes remain in memory until saved.
- **Args:** required: `node_id` (optional: `lint`)
- **Errors:** `INVALID_ARGUMENTS`, `NODE_NOT_FOUND`, `PAGE_ROOT_PROTECTED`, `OUTSIDE_OWNED_FRAMES`, `TOOL_DISABLED`
- **Example:** `{"node_id":3}`

### `designos_list_comments`
- **Purpose:** List canvas comment threads enriched with resolved world coordinates, target node ID/name/kind, and top-level frame ID/name. Filter by resolved state, page_id, or node_id.
- **Args:** required: none (optional: `node_id`, `page_id`, `resolved`)
- **Errors:** `INVALID_ARGUMENTS`, `TOOL_DISABLED`
- **Example:** `{"resolved":false}`

### `designos_add_comment`
- **Purpose:** Pin a new comment thread to a node (with local offset x/y) or page coordinates. Supports undo.
- **Args:** required: `text` (optional: `author`, `node_id`, `page_id`, `x`, `y`)
- **Errors:** `INVALID_ARGUMENTS`, `NODE_NOT_FOUND`, `TEXT_TOO_LONG`, `VALUE_OUT_OF_RANGE`, `TOOL_DISABLED`
- **Example:** `{"node_id":2,"x":24,"y":16,"text":"Increase contrast on subtitle"}`

### `designos_reply_comment`
- **Purpose:** Append a reply message to an existing canvas comment thread and optionally mark it resolved/unresolved. Supports undo.
- **Args:** required: `thread_id`, `text` (optional: `author`, `resolved`)
- **Errors:** `INVALID_ARGUMENTS`, `NODE_NOT_FOUND`, `TEXT_TOO_LONG`, `TOOL_DISABLED`
- **Example:** `{"thread_id":"t1","text":"Updated to $easeui.text.primary","resolved":true}`

### `designos_resolve_comment`
- **Purpose:** Mark a canvas comment thread resolved (default true) or unresolved (false). Supports undo.
- **Args:** required: `thread_id` (optional: `resolved`)
- **Errors:** `INVALID_ARGUMENTS`, `NODE_NOT_FOUND`, `TOOL_DISABLED`
- **Example:** `{"thread_id":"t1","resolved":true}`

### `designos_apply_spec`
- **Purpose:** Build a complete UI subtree from a declarative NodeSpec (supports EaseUI BrainWave 2.0 presets like easeui-screen/easeui-card/easeui-button-primary, $easeui.* tokens, and Auto-Layout 5.0 row/column/wrap/hug/fill) in a single atomic undoable step.
- **Args:** required: `spec` (optional: `lint`, `parent_id`)
- **Errors:** `INVALID_ARGUMENTS`, `UNKNOWN_FIELD`, `NODE_NOT_FOUND`, `NOT_A_CONTAINER`, `UNKNOWN_COMPONENT`, `UNKNOWN_VARIABLE`, `UNKNOWN_SLOT`, `TOOL_DISABLED`
- **Example:** `{"parent_id":1,"spec":{"kind":"frame","name":"Metric Card","preset":"easeui-card","layout":{"direction":"column","gap":12,"padding":[20,20,20,20]},"children":[{"kind":"text","name":"Label","text":"Net Revenue","font_size":13},{"kind":"text","name":"Value","text":"$148,920","font_size":28,"font_weight":600}]}}`

### `designos_export_spec`
- **Purpose:** Export a node and its descendants as a declarative NodeSpec JSON tree, enriched with Dev Mode status/annotations and any canvas comment threads pinned within the subtree.
- **Args:** required: `node_id`
- **Errors:** `INVALID_ARGUMENTS`, `NODE_NOT_FOUND`, `TOOL_DISABLED`
- **Example:** `{"node_id":2}`

### `designos_set_dev_status`
- **Purpose:** Set a layer's Dev Mode handoff status ('ready', 'completed', or 'clear') and/or append a developer annotation. Supports undo.
- **Args:** required: `node_id` (optional: `annotation`, `status`)
- **Errors:** `INVALID_ARGUMENTS`, `NODE_NOT_FOUND`, `TEXT_TOO_LONG`, `TOOL_DISABLED`
- **Example:** `{"node_id":2,"status":"ready","annotation":"Uses semantic spacing tokens"}`

### `designos_export_fig_file`
- **Purpose:** Export the current session document to a Figma binary .fig container (with fig-kiwi header and deflate chunks) inside the workspace.
- **Args:** required: `output_path`
- **Errors:** `INVALID_ARGUMENTS`, `PATH_OUTSIDE_WORKSPACE`, `INTERNAL_ERROR`, `TOOL_DISABLED`
- **Example:** `{"output_path":"handoff.fig"}`

### `designos_query_xpath`
- **Purpose:** Query layers on a page using an XPath-like selector (e.g. //FRAME, //FRAME[@name='Card'], //TEXT[contains(@name, 'Title')]) or a kind/name substring.
- **Args:** required: none (optional: `page_id`, `query`, `selector`)
- **Errors:** `INVALID_ARGUMENTS`, `INVALID_SELECTOR`, `NODE_NOT_FOUND`, `TOOL_DISABLED`
- **Example:** `{"selector":"//FRAME[@name='Metric Card']"}`

### `designos_lint`
- **Purpose:** Run the unified solver-backed geometric, text, token, and structural design linter on a page, specific node subtrees, or the active page.
- **Args:** required: none (optional: `cursor`, `include_ignored`, `limit`, `node_ids`, `page_id`, `preset`, `rules`)
- **Errors:** `INVALID_ARGUMENTS`, `NODE_NOT_FOUND`, `VALUE_OUT_OF_RANGE`, `TOOL_DISABLED`
- **Example:** `{"preset":"agent","limit":100}`

### `designos_get_guidelines`
- **Purpose:** List available design guideline topics or return the guideline document for a topic ('design-system', surface guides, 'archetypes', 'archetype:<id>', or 'rules').
- **Args:** required: none (optional: `max_tokens`, `platform`, `topic`)
- **Errors:** `INVALID_ARGUMENTS`, `UNKNOWN_TOPIC`, `TOPIC_NOT_AVAILABLE`, `TOOL_DISABLED`
- **Example:** `{"topic":"design-system","platform":"web"}`

### `designos_undo`
- **Purpose:** Undo the latest successful edit in this session; returns changed=false if the stack is empty.
- **Args:** required: none
- **Errors:** `INVALID_ARGUMENTS`, `HISTORY_DROPPED`, `TOOL_DISABLED`
- **Example:** `{}`

### `designos_redo`
- **Purpose:** Redo the most recently undone edit in this session; returns changed=false if the stack is empty.
- **Args:** required: none
- **Errors:** `INVALID_ARGUMENTS`, `HISTORY_DROPPED`, `TOOL_DISABLED`
- **Example:** `{}`

### `designos_apply_batch`
- **Purpose:** Execute 1..=200 write operations (apply_spec, insert, update, move, delete, set_variables, set_style) as a single atomic transaction and one undo group. Later ops may reference nodes created by earlier ops via '@<as>' or '@<as>/<name path>'. With dry_run=true, computes and returns created IDs, name_to_id map, touched_roots, and touched_bounds without mutating the document or publishing a live patch. Note: in a live Studio session, if the editor remaps a colliding ID on replay, the reply holds the session IDs until the next canvas reload.
- **Args:** required: `ops` (optional: `diff_image`, `dry_run`, `idempotency_key`, `lint`)
- **Errors:** `INVALID_ARGUMENTS`, `BATCH_OP_FAILED`, `UNKNOWN_REF`, `DUPLICATE_AS`, `BATCH_TOO_LARGE`, `TOOL_DISABLED`
- **Example:** `{"ops":[{"op":"create_node","as":"card","args":{"parent_id":1,"kind":"frame","name":"Summary Card","width":360,"height":180}},{"op":"create_node","args":{"parent_id":"$card","kind":"text","name":"Headline","text":"Q3 Overview"}}]}`

### `designos_configure_session`
- **Purpose:** Configure per-session MCP options for the automatic mutating-call lint issues block (issues.mode: preset | errors_only | off, issues.preset, issues.max_items: 1..=100) and auto_fix settings. Persists across reconnects in the session directory and resets on document switch.
- **Args:** required: none (optional: `auto_fix`, `issues`)
- **Errors:** `INVALID_ARGUMENTS`, `VALUE_OUT_OF_RANGE`, `TOOL_DISABLED`
- **Example:** `{"issues":{"enabled":true,"preset":"agent","max_items":25}}`

### `designos_open_file`
- **Purpose:** Import a .designos, .designos, .fig, SVG or Figma REST JSON file inside the workspace into this isolated session. Does not control an open editor. Unsaved changes require discard_changes=true.
- **Args:** required: `path` (optional: `discard_changes`)
- **Errors:** `INVALID_ARGUMENTS`, `UNSAVED_CHANGES`, `PATH_OUTSIDE_WORKSPACE`, `FILE_TOO_LARGE`, `DOCUMENT_INVALID`, `TOOL_DISABLED`
- **Example:** `{"path":"design.designos","discard_changes":false}`

### `designos_save_file`
- **Purpose:** Save this session to a .designos file inside the workspace. Existing files are not overwritten unless overwrite=true. Parent directory must exist.
- **Args:** required: `path` (optional: `overwrite`)
- **Errors:** `INVALID_ARGUMENTS`, `PATH_OUTSIDE_WORKSPACE`, `FILE_EXISTS`, `TOOL_DISABLED`
- **Example:** `{"path":"design.designos","overwrite":true}`

### `designos_render_png`
- **Purpose:** Rasterize a frame or layer to a NEW .png file inside the workspace using DESIGN:OS Studio's deterministic renderer. Existing files are never overwritten.
- **Args:** required: `path`, `node_id` (optional: `scale`)
- **Errors:** `INVALID_ARGUMENTS`, `NODE_NOT_FOUND`, `VALUE_OUT_OF_RANGE`, `PATH_OUTSIDE_WORKSPACE`, `FILE_EXISTS`, `TOOL_DISABLED`
- **Example:** `{"node_id":2,"path":"preview.png","scale":2.0}`

### `designos_new_document`
- **Purpose:** Start a blank document with one page. Unsaved changes require discard_changes=true.
- **Args:** required: `name` (optional: `discard_changes`)
- **Errors:** `INVALID_ARGUMENTS`, `UNSAVED_CHANGES`, `TEXT_TOO_LONG`, `TOOL_DISABLED`
- **Example:** `{"name":"Fintech Workspace","discard_changes":true}`

## Declarative `NodeSpec` Reference (Generated from Schema)

Used by `designos_apply_spec`, `designos_export_spec`, and `$DESIGNOS_LIVE_SPEC`.

### `NodeSpec` Fields

Declarative node specification used by Agents and CLI/MCP tools (`apply_spec` / `export_spec`).

| Field | Type | Description |
| --- | --- | --- |
| `annotation` | `string` | Single developer handoff annotation string. |
| `annotations` | `[string]` | Developer handoff annotations attached to this node. |
| `children` | `[#]` | Child `NodeSpec` items inside this container. |
| `clips_content` | `boolean` | Clip overflowing child content to this frame's bounds when `true`. |
| `col_span` | `integer` | Grid column span (`&gt;= 1`). |
| `comments` | `[ResolvedCommentThread]` | Attached canvas comments when exporting a subtree spec. |
| `component` | `string` | Component reference for `"instance"` nodes (`"node:&lt;id&gt;"`, component key, or `"&lt;Library&gt;/&lt;Component&gt;"`). |
| `corner_radii` | `array` | Per-corner radii `[top_left, top_right, bottom_right, bottom_left]` in px. |
| `corner_radius` | `number` | Uniform corner radius in px (or `NumberOrVar`). |
| `dev_status` | `string` | `"ready"` (`ReadyForDev`) or `"completed"` (`Completed`). |
| `effect_style` | `string` | Shared effect style reference (`"style:&lt;name&gt;"` or style name). |
| `fill` | `string` | Fill colour (`"#18181B"`), variable (`"var:&lt;collection&gt;/&lt;name&gt;"`), style (`"style:&lt;name&gt;"`), or EaseUI token (`"$easeui.bg.panel"`). |
| `font_size` | `number` | Font size in px (or `NumberOrVar`). |
| `font_weight` | `integer` | Font weight (`100..=900`, e.g. `400`, `500`, `600`, `700`). |
| `height` | `number` | Explicit height in px (or `NumberOrVar`). |
| `height_sizing` | `string` | `"fixed"`, `"hug"`, or `"fill"`. |
| `id` | `integer` | Numeric node ID (assigned on export; optional on input). |
| `kind` | `string` | `"frame"`, `"rectangle"`, `"ellipse"`, `"text"`, `"section"`, `"component"`, or `"instance"`. |
| `layout` | `LayoutSpec` | Auto-Layout 5.0 configuration for container frames. |
| `line_height` | `NumberOrVar` | Line height in px (or `NumberOrVar`). |
| `max_height` | `number` | Maximum height constraint in px. |
| `max_width` | `number` | Maximum width constraint in px. |
| `min_height` | `number` | Minimum height constraint in px. |
| `min_width` | `number` | Minimum width constraint in px. |
| `mode` | `object` | Variable collection mode overrides `{collection_name: mode_name}` on this container. |
| `name` | `string` | Human-readable layer name (never leave default names like `"Frame 1"`). |
| `opacity` | `number` | Layer opacity (`0.0..=1.0`, default `1.0`). |
| `overrides` | `[OverrideSpec]` | Ordered descendant overrides applied inside an `"instance"`. |
| `preset` | `string` | Optional EaseUI BrainWave 2.0 preset: `"easeui-screen"`, `"easeui-panel"`, `"easeui-card"`, `"easeui-button-primary"`, `"easeui-button-secondary"`, `"easeui-input"`, `"easeui-badge"`, `"easeui-row"`, `"easeui-column"`. |
| `props` | `object` | Component property overrides `{prop_name: value}` on an `"instance"` root. |
| `row_span` | `integer` | Grid row span (`&gt;= 1`). |
| `slots` | `object` | Slot fills or disable flags `{slot_name: [NodeSpec...] \| false}` on an `"instance"` root. |
| `stroke` | `string` | Stroke colour (`"#RRGGBB"`), variable (`"var:&lt;collection&gt;/&lt;name&gt;"`), style (`"style:&lt;name&gt;"`), or EaseUI token. |
| `stroke_weight` | `number` | Stroke weight in px (or `NumberOrVar`). |
| `text` | `string` | Text content (for `"text"` nodes, or automatically creates a child label if set on a button/badge frame). |
| `text_color` | `string` | Text colour (`"#RRGGBB"`, `"var:&lt;collection&gt;/&lt;name&gt;"`, or `"$easeui.text.primary"`). |
| `text_style` | `string` | Shared text style reference (`"style:&lt;name&gt;"` or style name). |
| `variant` | `object` | Variant property selection `{property: value}` for `"instance"` nodes. |
| `width` | `number` | Explicit width in px (or `NumberOrVar`). |
| `width_sizing` | `string` | `"fixed"`, `"hug"`, or `"fill"`. |
| `x` | `number` | Parent-relative X coordinate in px. |
| `y` | `number` | Parent-relative Y coordinate in px. |

### `LayoutSpec` Fields

Declarative Auto-Layout 5.0 specification for a container frame.

| Field | Type | Description |
| --- | --- | --- |
| `counter_align` | `string` | `"start"`, `"center"`, `"end"`, or `"baseline"`. |
| `counter_gap` | `number` | Cross-axis spacing between wrapped lines in px. |
| `direction` | `string` | `"row"` / `"horizontal"` or `"column"` / `"vertical"`. |
| `gap` | `number` | Primary-axis spacing between children in px (or `"var:&lt;collection&gt;/&lt;name&gt;"`). |
| `grid_columns` | `integer` | CSS-grid column count when using grid auto-layout. |
| `grid_rows` | `integer` | CSS-grid row count when using grid auto-layout. |
| `padding` | `[number]` | `[top, right, bottom, left]` container padding in px. |
| `primary_align` | `string` | `"start"`, `"center"`, `"end"`, `"space-between"`, `"space-around"`, `"space-evenly"`, or `"baseline"`. |
| `wrap` | `boolean` | Wrap overflowing children onto additional lines when `true`. |

### `OverrideSpec` Fields

Descendant override specification inside a `NodeSpec` of `kind: "instance"`.

| Field | Type | Description |
| --- | --- | --- |
| `fill` | `string` | Override fill colour (`"#RRGGBB"`, `"var:&lt;collection&gt;/&lt;name&gt;"`, or `"style:&lt;name&gt;"`). |
| `id` | `integer` | Numeric descendant node ID inside the instance (alternative to `path`). |
| `opacity` | `number` | Override descendant opacity (`0.0..=1.0`). |
| `path` | `string` | Slash-separated descendant name path inside the instance (`"Header/Title"` or `"Card[1]/Label"`). |
| `props` | `object` | Override component properties `{prop_name: value}` on a nested instance. |
| `slots` | `object` | Populate or disable slots `{slot_name: [NodeSpec...] \| false}` on a nested instance. |
| `stroke` | `string` | Override stroke colour (`"#RRGGBB"`, `"var:&lt;collection&gt;/&lt;name&gt;"`, or `"style:&lt;name&gt;"`). |
| `swap` | `string` | Swap a nested instance to another component (`"&lt;ComponentName&gt;"` or `"&lt;Library&gt;/&lt;Component&gt;"`). |
| `text` | `string` | Override text characters on a descendant `"text"` layer. |
| `variant` | `object` | Switch variant properties `{property: value}` on a nested component instance. |
| `visible` | `boolean` | Toggle descendant visibility (`true` \| `false`). |

### `SlotFillSpec`

Slot fill or disable specification in `NodeSpec::slots` / `OverrideSpec::slots` (`[NodeSpec...] | false`).

| Variant | Description |
| --- | --- |
| `FalseLiteral` | Disable and hide the slot (`false`). |
| `[NodeSpec]` | Populate the slot with an ordered list of child `NodeSpec` items (`[NodeSpec...]`). |

### `NumberOrVar`

A numeric spec field that accepts either a literal `f32` or a `"var:<collection>/<name>"` reference.

| Variant | Description |
| --- | --- |
| `number` | Numeric value in px (or unitless opacity `0.0..=1.0`). |
| `string` | Variable reference string `"var:&lt;collection&gt;/&lt;name&gt;"` or `"var:&lt;Library&gt;/&lt;collection&gt;/&lt;name&gt;"`. |

## Per-Call `issues` Block & Error Recovery

- **Per-call `issues` block (E15-12):** Every mutating response attaches `issues: {total, new_in_this_call, truncated, items, issues_skipped_rules}`. Each item reports `{rule_id, severity, node_id, node_path, message, fix_hint, auto_fixable}`. Always resolve `new_in_this_call > 0` before continuing.
- **Session lint tuning:** Use `designos_configure_session` (`issues: {enabled, preset, max_items, include_warnings}`) or per-call `lint: false` when bulk-loading intermediate scaffolding.
- **Structured errors (E15-5):** Error responses include `code` (`UPPER_SNAKE_CASE`), `message`, optional `path`, `valid_options`, `corrected_example`, `safe_to_retry`, and `next_step`.
- **Retry contract:** When `safe_to_retry: true`, repair the call using `corrected_example` or `valid_options` and retry once. When `safe_to_retry: false` (or `code == "TOOL_DISABLED"`), do not blind-retry; follow `next_step`.

## Design Quality & Anti-Generic Rules

Enforce these qualitative standards alongside automated linting:

- **Visual hierarchy**: Establish one primary focal point per screen. Maintain at least a 3:1 size ratio between display headlines and body copy. Never give primary, secondary, and tertiary actions equal visual weight.
- **Layout rhythm**: Use at most 3 distinct gap values per screen. Padding must increase with container hierarchy (`page >= section >= card >= control`). Never nest cards inside cards, and never mix left-aligned and centred section headers in the same vertical flow.
- **Colour discipline**: Use one primary accent per screen. Reserve status colours (red, amber, green) strictly for semantic status. Let neutral surface elevation carry hierarchy rather than saturated backgrounds; in dark mode, express depth via surface luminance and subtle borders rather than inverted light-mode shadows.
- **Typography**: Use at most 2 font families and 4 font sizes per screen. Keep body copy line length between 45 and 75 characters, and use tabular numerals (`tabular-nums`) for prices, metrics, timestamps, and data tables.
- **Content realism**: Never use `"Lorem ipsum"`, `"Card 1 / Card 2 / Card 3"`, `"John Doe"`, `"$99.99"`, `"123456"`, or identical repeated rows. Populate every screen with domain-specific labels, realistic uneven numbers, and varied states (active, pending, empty, long name).
- **Anti-generic tells**: Avoid gratuitous purple-to-blue gradients, glowing neon borders on enterprise workspaces, 3-equal-column icon-title-blurb grids when a domain-specific layout fits the content, and decorative charts without axes or units.
