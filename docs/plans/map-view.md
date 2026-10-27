# LightCraft: Map view and photo location map — implementation plan

For Claude Code, working in Michael Craig's private fork `mwcraig71/lightcraft`. Claude Code also reads the repo's `CLAUDE.md` automatically; this plan fits its code rules (no panics in product code, pure Rust, layering, `cargo xtask ci`).

## Notes for this fork (read first)

- **Private fork, not going upstream.** Follow `CLAUDE.md`'s code and quality rules, but:
  - `plan/STATUS.md`, `plan/execution-plan.md` and `plan/architecture.md` don't exist here (`plan/` is gitignored upstream). This file replaces them for this work.
  - Skip the upstream progress-doc bookkeeping (`ROADMAP.md`, `docs/target-app-parity.md`, `docs/gaps.md` numbers). Only touch `docs/parity-checklist.md` if `cargo xtask parity` requires it.
  - English UI text is enough. Use `crate::i18n::tr` for every string (it falls back to English); don't translate into the other locale catalogs unless a test requires an entry.
- **Keep upstream merges easy.** Upstream lands dozens of commits a day. Put new code in new files/modules (`crates/tiles`, `crates/ui-egui/src/panels/map/`) and keep edits to existing files to the few wiring lines needed. To pull upstream later: `git remote add upstream https://github.com/storytold/lightcraft && git fetch upstream && git merge upstream/main`.
- **Who takes the photos:** mostly phones and tablets, sometimes drones, sometimes DSLR/SLR cameras. Phones and tablets record a compass heading, so the heading cone is in Phase 1, not later. DSLRs usually have no GPS at all; those photos get a location from the existing *Photo ▸ Auto-Tag from Tracklog* (a GPX track recorded on a phone during the inspection). The Map view should point to that when photos lack a location.
- **Long-running work rule:** tile downloads are many short requests tied to the visible map, not a task the user starts or cancels. Don't give them an activity-stack `TaskGuard`; do give one to anything the user launches that runs longer (clearing a large cache, Phase 3 offline download).
- **New icon:** any icon drawn in `icons.rs` needs its `assets/ATTRIBUTION.md` entry ("original work") if `cargo xtask assets` asks for one.

## Goal

See on a map where each photo was taken, fast enough to use during and after a bridge inspection. A typical set is a few hundred photos taken along one long bridge, so the map must:

- show every photo in the current folder/album as a pin, in capture order, and fit them on screen in one click;
- zoom close enough that pins at adjacent piers or spans separate;
- jump between map and photo both ways: click a pin to select the photo, select a photo in the filmstrip to highlight its pin;
- show which way the camera was pointing when the photo has a compass heading (phones and drones usually record it);
- offer aerial imagery, not only a street map, because a bridge and its spans are recognizable from above.

## What already exists (verified in the source, upstream commit 3a3e49af)

| Piece | Where | Note |
|---|---|---|
| GPS read from EXIF | `crates/meta/src/exif.rs` `read_gps` (~line 157) | lat, lon, altitude. Heading is **not** read. |
| GPS read/write in XMP | `crates/meta/src/xmp.rs` (~600–675) | lat, lon, altitude |
| GPS value type | `crates/meta/src/lib.rs` `pub struct Gps` | `latitude`, `longitude`, `altitude` |
| Heading tag ids | `crates/tiff/src/tags.rs` `GPS_IMG_DIRECTION_REF = 16`, `GPS_IMG_DIRECTION = 17` | defined, unused |
| GPS stored per photo | `crates/catalog/src/model.rs` `Meta.gps: Option<(f64, f64)>` | lat/lon only; altitude dropped at import (`crates/engine/src/files/probe.rs:22`) |
| `Meta` is `#[serde(default)]` | same file | new optional fields load fine from old catalogs |
| Info panel GPS field + "Show on Map" (opens OSM in browser) | `crates/ui-egui/src/panels/right/info.rs` (~136–170) | place for an inline mini-map |
| View modes | `crates/ui-egui/src/state/view.rs` `ViewMode` | `People` is the best template to copy |
| View wiring for `People` | `menus/ui_commands.rs:73`, `menus/run.rs:301`, `menus/menubar.rs:193,401`, `docking.rs:436`, `panels/bottombar.rs:19` | do the same for `Map` |
| Thumbnails for the UI | `crates/ui-egui/src/render.rs` `thumb(id)` | reuse for pin hover previews |
| HTTPS client (pure Rust, rustls, redirect and size limits, cancel flag) | `crates/fetch/src/http.rs` `get(url, headers, limits)` | reuse for tiles; it sends `User-Agent: LightCraft/<ver>` |
| GPX tracklog tagging, offline reverse geocoding | `crates/meta/src/gpx.rs`, `cmd:metadata.reverseGeocode` | already done |
| Layer rules | `xtask/src/layers.rs` `TABLE` | a new crate must be added here or CI fails |
| Parity rows to update | `docs/parity-checklist.md` `LR-MAP-INFO`, `LR-MAP-MODULE`, `LRC-MAP-VIEW` | `cargo xtask parity` checks cited ids exist |

## Design decisions

1. **Write a small map widget in-house; don't add `walkers` or another map crate.** The app runs a patched egui 0.37 fork and already has its own HTTPS client. A map crate would bring tokio/reqwest and pin an egui version. The needed widget is modest: Web Mercator tile math, pan/zoom, tile textures, pins.
2. **Two tile sources, selectable in Settings:**
   - **Street:** OpenStreetMap, `https://tile.openstreetmap.org/{z}/{x}/{y}.png`, zoom 0–19. Must follow the OSM tile usage policy: a User-Agent naming the app with a contact URL, honour `Cache-Control`/`ETag` (or keep tiles at least 7 days), no bulk download or prefetch, no "save area for offline", visible attribution "© OpenStreetMap contributors".
   - **Imagery (US and Puerto Rico):** USGS The National Map, `https://basemap.nationalmap.gov/arcgis/rest/services/USGSImageryOnly/MapServer/tile/{z}/{y}/{x}` (note: row = y comes before column = x). Web Mercator, 256 px tiles, public domain NAIP ~1 m imagery. The service shows tiles to about zoom 16; overzoom (scale up the zoom-16 tile) to zoom 19 so pins separate. Attribution: "USDA, USGS The National Map: Orthoimagery".
   - Verify both endpoints and their current terms before shipping.
3. **Network is opt-in.** LightCraft promises "local-first, no cloud". Tiles reveal which area you are viewing (not your photos), so the map loads no tiles until the user turns them on: an inline "Load map tiles from OpenStreetMap / USGS?" prompt the first time, remembered in Settings. With tiles off, the map still draws pins on a plain background with a scale bar, so it works with no connection.
4. **Disk tile cache** in the app's cache folder (use the same location helper the preview cache uses), keyed by source/z/x/y, storing `ETag`/expiry; size cap (default 500 MB, LRU eviction) and a Clear button in Settings. Cached tiles keep the map usable on site with poor signal for areas already viewed.
5. **New crate `crates/tiles`** (package `lightcraft-tiles`) for everything that isn't UI: tile math, sources, cache, fetch worker pool, PNG/JPEG decode to RGBA. Depends on `lightcraft-fetch` (L0) and the decoder already used in the workspace (`lightcraft-codecs`, L1), so it is **Layer 2**. Add `("tiles", Class::Layer(2))` to `xtask/src/layers.rs`. Not built for wasm in phase 1 (the web build shows pins without tiles).
6. **UI gets tiles through `services`**, like `check_editor`: `services.map_tiles: Option<Box<dyn TileProvider>>`. The desktop app sets the real provider; the web build and headless/kittest tests get `None` or a fake provider that returns solid-colour tiles. Tests never touch the network.
7. **Everything is a command**, per the repo's architecture: `view.map`, `map.fitAll`, `map.toggleTrack`, `map.setTileSource`. Agents and the control channel get the map for free.

## Phase 1 — Map view with pins (the core)

**`crates/tiles` (new)**
- `lib.rs` with `#![forbid(unsafe_code)]` and the repo's deny-unwrap attribute set (copy from `crates/fetch/src/lib.rs`).
- `mercator.rs`: lat/lon ↔ world pixels at zoom z (256·2^z), tile x/y for a point, visible tile range for a viewport, clamp latitude to ±85.0511°.
- `source.rs`: `TileSource { id, url_template, min_zoom, max_zoom, max_native_zoom, attribution, xy_order }` for OSM and USGS.
- `cache.rs`: disk cache with expiry/ETag, conditional requests (`If-None-Match`), size cap with LRU eviction.
- `pool.rs`: 2 worker threads per host, dedupe in-flight tiles, drop requests for tiles that left the viewport (use the `cancel` flag in `fetch::http::Limits`), 404/no-tile response cached as empty.
- User-Agent: `LightCraft/<version> (+<fork or project URL>)`; add a header parameter rather than changing `fetch`'s default.

**`crates/ui-egui`**
- `state/view.rs`: add `ViewMode::Map`; map state (centre, zoom, track on/off) in UI state, saved in prefs like other view state.
- `panels/map/` (new module):
  - `widget.rs`: pan (drag), zoom (wheel/pinch around the cursor, +/− buttons), draw visible tiles from an LRU of egui textures (parent-tile fallback while children load), scale bar, attribution label bottom-right (always visible), tile-source switcher.
  - `pins.rs`: one pin per photo with GPS in the photos the grid currently shows (reuse the same visible-photo list the grid and Survey use; don't build a second filter). Sort by capture time; draw a thin track line in that order (toggle). Screen-space clustering (~40 px cells) with a count badge; clicking a cluster zooms to it. Selected photo's pin highlighted; hover shows the thumbnail (`render.rs thumb`) plus capture time and file name.
  - `view.rs`: the Map view: map fills the centre, filmstrip stays at the bottom. "Fit all" on open and on folder change. Status line: "312 photos · 287 with location". Photos without GPS are listed as a count, with a link to filter them.
- Click pin → select the photo (dispatch the same selection command a grid click uses). Double-click → Detail view. Selecting in the filmstrip pans the map to that pin if it's off-screen.
- Wire `Map` exactly like `People`: `menus/ui_commands.rs` (`("view.map", "Map", <free shortcut or None>, "View")` — check for shortcut conflicts with the existing keymap tests), `menus/run.rs`, `menus/menubar.rs` (checked state), `docking.rs` (dispatch to `panels::map::view::show`), `panels/bottombar.rs` (icon; add a map-pin icon to the icons module if none fits).
- Settings: a Map section in the settings dialog (`dialogs/settings/`): Tiles Off / Street / Imagery, cache size, Clear cache.
- All new strings through `crate::i18n::tr`; follow `docs/localization.md` for how new strings are added for the other languages.

**`apps/lightcraft`**
- Construct the real `TileProvider` in `src/session.rs` where `check_editor` is set; `apps/lightcraft-web/src/web.rs` sets `None`.

## Phase 1b — compass heading and altitude (do right after Phase 1; phones and tablets are the main cameras)

**Read and keep the compass heading and altitude**
- `crates/meta/src/lib.rs`: add `direction: Option<f64>` (degrees, 0 = north) to `Gps`; update every struct literal (`psd.rs`, `gpx.rs`, tests).
- `crates/meta/src/exif.rs` `read_gps`: read `GPS_IMG_DIRECTION` (rational) and `GPS_IMG_DIRECTION_REF` (`T` true / `M` magnetic; keep the ref so the UI can say "magnetic"). Reject non-finite or out-of-range values.
- `crates/meta/src/xmp.rs`: read/write `exif:GPSImgDirection` and `exif:GPSImgDirectionRef`.
- **Drones:** many drones (DJI in particular) put the camera's direction in their own XMP tags instead of `GPSImgDirection`, e.g. `drone-dji:GimbalYawDegree` (camera) and `drone-dji:FlightYawDegree` (aircraft), and height above takeoff in `drone-dji:RelativeAltitude`. Read `GimbalYawDegree` as the heading when `GPSImgDirection` is missing (yaw is −180…180; normalise to 0…360). Confirm the tag names against a real drone photo before relying on them, and keep this read-only.
- `crates/catalog/src/model.rs` `Meta`: add `gps_altitude: Option<f64>` and `gps_direction: Option<f32>` with `#[serde(default, skip_serializing_if = "Option::is_none")]` so old catalogs load and unchanged photos serialize identically.
- `crates/engine/src/files/probe.rs:22`, `sidecar.rs`, `cmd/convert.rs`, `export/encoding.rs:311`: carry the new fields through import, sidecar sync, convert, and export (stop dropping altitude on export).
- Existing photos: confirm the existing "read metadata from files" command refreshes these fields; if it doesn't, extend it rather than adding a new one.

**Draw it**
- Pins with a heading get a view cone (about 60°) pointing the camera direction; a "magnetic" note in the hover card when the ref is `M`.

## Phase 2 — Info-panel mini-map and inspection extras

- Info panel (`panels/right/info.rs`): a ~180 px mini-map under the GPS field for the active photo (pin plus cone, zoom 17), click to open the Map view centred there. Keep the existing "Show on Map" browser button.
- Export photo locations as KML/GeoJSON (pins named by file name or title, with capture time and heading) for inspection reports and GIS, as an engine command (`photo.exportLocations`) so it also works from the CLI and MCP.
- GPS accuracy circle from `GPSHPositioningError` when present (phones write it; at a bridge, phone GPS is often off by 5–10 m, which matters at short spans).

## Phase 3 — later, optional

- Drag photos onto the map to set their location (parity row `LRC-MAP-GEOTAG`).
- "Photos in the visible map area" filter (`LRC-MAP-FILTER`).
- Download an area of **USGS imagery only** for offline use (never OSM; their policy forbids it).
- Web build: tiles through the browser's `fetch`.

## Tests (each would fail without the feature)

- `tiles`: mercator round-trip property test (lat/lon → pixels → lat/lon), tile ranges at the antimeridian and poles, USGS y/x order, overzoom picks the right parent tile and sub-rectangle, cache honours `max-age`/`ETag` and the size cap (file-system test, no network).
- `meta`: EXIF and XMP heading parse (true, magnetic, missing ref, zero-denominator rational, out of range).
- `catalog`: an old catalog JSON without the new fields loads; a photo with `None` serializes byte-identical to before.
- `ui-egui` (kittest/headless with the fake tile provider): Map view with 3 GPS photos and 1 without shows 3 pins and "1 without location"; clicking a pin selects that photo; at low zoom 3 close pins become one cluster of 3; with tiles off, zero tile requests are made and the prompt is shown; attribution text is present.
- `xtask layers` passes with the new crate.

## Docs

- New `docs/map.md` (sources, attribution, privacy, cache, commands, how to geotag DSLR photos from a phone GPX track).
- `docs/architecture.md` layer table: add `tiles`.
- Upstream bookkeeping docs: skip (see "Notes for this fork").

## Acceptance check (manual, on a real inspection set)

1. Import a folder of bridge photos taken with a phone or drone.
2. Open View ▸ Map: all located photos appear, fitted to the screen, in capture order with the track line.
3. Switch to Imagery and zoom in: pins at neighbouring piers are separate and the bridge deck is visible.
4. Click a pin: the filmstrip jumps to that photo. Arrow through the filmstrip: the highlighted pin follows.
5. Phone, tablet and drone photos show a heading cone; DSLR photos without GPS are counted as "without location", and after Auto-Tag from Tracklog with a phone GPX track they appear as plain pins.
6. Turn off Wi-Fi: previously viewed areas still draw from the cache; new areas show the plain background with pins.

## Prompt to give Claude Code

> Read `docs/plans/map-view.md`, starting with "Notes for this fork". Implement Phase 1 and Phase 1b on a new branch `feature/map-view`, in small commits. Before each commit run `cargo xtask ci --quick`; before you say you're done, run the full `cargo xtask ci` and report any skipped tests. Don't add a map crate dependency; build the widget as the plan describes. Stop after Phase 1b and show me screenshots of the Map view (headless snapshot flow in `CLAUDE.md`) before starting Phase 2. Don't push to `main`.

Building on Windows needs Rust 1.95 or newer (`rustup update stable`).
