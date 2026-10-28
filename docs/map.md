# Map view

**View ▸ Map** shows every photo in the current folder, album or filter that has a location, as a
pin on a map. The pins follow the grid's own list (same source, filter and sort), in capture order,
joined by a thin track line. It was built for site work such as bridge inspections, where a set is
a few hundred photos along one structure.

- **Open:** View ▸ Map, or the map button in the bottom bar. The map fits every pin on opening and
  whenever the folder or filter changes; **Fit All** does it again.
- **Pan** by dragging. **Zoom** with the wheel or a pinch (about the pointer), or **+ / −**.
- **Click a pin** to select its photo (Shift: a range, Cmd/Ctrl: add or remove, as in the grid).
  **Double-click** opens it in Detail. Selecting a photo in the filmstrip brings its pin on screen,
  highlighted.
- **Zoomed far out**, each site (photos taken close together, such as along one bridge) is one
  dot labelled with its name and photo count. The name is the album's when an album is shown,
  otherwise the folder all the site's photos share (`Bridge 1234/Span 1` and `Bridge 1234/Span 2`
  → "Bridge 1234"). A click zooms in to the site.
- **Clusters:** closer in, pins closer than about 40 points merge into a circle with a count (one
  per pier or span). A click zooms in until they separate; photos taken at one spot stay a
  cluster at the deepest zoom, and a click then selects them all. Closer still, every photo has
  its own pin.
- **Hover** a pin for its thumbnail, file name and capture time.
- **Heading:** a photo that recorded which way the camera pointed gets a 60° view cone: phones and
  tablets write `GPSImgDirection` (EXIF or XMP; true or magnetic north), DJI drones the gimbal's
  yaw (`drone-dji:GimbalYawDegree`, read only, used when there is no `GPSImgDirection`). The hover
  card gives the heading, "magnetic" when it is from magnetic north, and the altitude. Headings and
  altitude are kept in the catalog, written to XMP sidecars and exported files; **Photo ▸ Reload
  from Disk** reads them for photos imported before LightCraft kept them.
- The status line counts the photos and those with a location. **"N without location"** selects
  the photos that have none.
- **Info panel mini-map:** under the GPS field, a photo with a location gets a small map of its
  place (the Map view's closest photo zoom), with its pin and view cone. It doesn't pan or zoom;
  a click opens the Map view centred on that photo (`view.map {"photo": id}`), with the photo
  selected. It draws tiles only once they are turned on, with their attribution, and never asks
  for them itself. **Show on Map** still opens the place in OpenStreetMap in the browser.

## Photos without a location

Phones, tablets and most drones record GPS. A DSLR usually doesn't: record a GPX track on a phone
while you shoot (any tracking app that exports GPX), then select the photos and run **Photo ▸
Auto-Tag from Tracklog…**. It matches each photo's capture time to the track (check the camera's
clock first). The Map view's empty state links to it.

## Map tiles: sources, privacy, attribution

LightCraft is local-first: **the map loads no tiles until you turn them on.** The first time the
Map view opens it asks; the answer is kept in **Settings ▸ Map** (or the Off / Street / Imagery
buttons on the map). With tiles off the pins draw on a plain background with a scale bar, so the
view works with no connection.

| Choice | Server | Zooms | Attribution (always shown on the map) |
|---|---|---|---|
| Street | OpenStreetMap, `tile.openstreetmap.org` | 0–19 | © OpenStreetMap contributors |
| Imagery (US and Puerto Rico) | USGS The National Map, `USGSImageryOnly` | 0–16, scaled up to 19 | USDA, USGS The National Map: Orthoimagery |

Tile requests tell the tile server which area you are looking at, never which photos you have.
They carry a `User-Agent` naming LightCraft and where to find it.

OpenStreetMap's [tile usage policy](https://operations.osmfoundation.org/policies/tiles/) is
followed: an identifying User-Agent, cache headers honoured (`Cache-Control` / `ETag`, and tiles
kept at least 7 days), at most two connections per server, no prefetching or bulk download, and no
"save this area for offline". USGS imagery is public domain; the attribution above is still shown.

## Tile cache

Tiles are kept on disk in `<settings folder>/tile-cache/<source>/<z>/<x>/<y>.tile` (the settings
folder is `%APPDATA%\LightCraft`, `~/Library/Application Support/LightCraft` or
`~/.config/lightcraft`). A stale tile is checked with the server (`If-None-Match`); if the server
can't be reached the old tile is still drawn, so areas viewed before stay usable on site with
poor signal. **Settings ▸ Map** sets the cache size (default 500 MB; the least recently used tiles
go first) and has **Clear Cache** (runs in the activity stack).

## Commands

| Command | Params | |
|---|---|---|
| `view.map` | `{photo?}` | Open the Map view; returns `{photos, located}`. With `photo`: select it and centre on it at the closest photo zoom instead of fitting every pin |
| `map.fitAll` | | Fit every pin on screen |
| `map.toggleTrack` | `{on?}` | The track line |
| `map.setTileSource` | `{source: off\|street\|imagery}` | Tiles (also Settings ▸ Map) |
| `map.clearCache` | | Delete the cached tiles (background) |

All go through the control channel and MCP like any UI command, e.g. a headless snapshot script:

```jsonl
{"method":"engine.execute","params":{"command":"photo.setMeta","params":{"ids":[1],"gps":[40.0, -75.0]}}}
{"method":"engine.execute","params":{"command":"view.map"}}
{"method":"ui.settle","params":{"timeoutMs":3000}}
{"method":"ui.screenshot","params":{"path":"map.png"}}
```

## Code

- `crates/tiles` (`lightcraft-tiles`, layer 2): Web Mercator math, the tile sources, the disk
  cache and the fetch workers (`TilePool`, the native `TileProvider`). No UI.
- `crates/ui-egui/src/panels/map/`: the view, the map widget and the pins. The UI gets tiles
  through `Services::map_tiles` (the desktop app passes a `TilePool`; the web build and tests pass
  none or a fake, so tests never touch the network).
