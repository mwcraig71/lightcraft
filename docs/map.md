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
  per pier or span). A click zooms in until they separate, selects their photos and **narrows the
  filmstrip to them** ("Filmstrip: 8 photos here"; **Show All** in the status line, Fit All, or a
  click on a pin elsewhere brings every photo back). Closer still, every photo has its own pin.
- **Zoom** goes to 22 (about 4 cm a point); past a map's deepest tiles they are scaled up.
- **Hover** a pin for its thumbnail, file name and capture time.
- **Heading:** a photo that recorded which way the camera pointed gets a 60° view cone: phones and
  tablets write `GPSImgDirection` (EXIF or XMP; true or magnetic north), DJI drones the gimbal's
  yaw (`drone-dji:GimbalYawDegree`, read only, used when there is no `GPSImgDirection`). The hover
  card gives the heading, "magnetic" when it is from magnetic north, the altitude and the accuracy
  ("±6 m"). Headings, altitude and accuracy are kept in the catalog, written to XMP sidecars and
  exported files; **Photo ▸ Reload from Disk** reads them for photos imported before LightCraft
  kept them.
- **Accuracy:** a photo whose file says how far off its position may be (`GPSHPositioningError`,
  EXIF or XMP, in metres; phones write it) gets a translucent blue circle of that radius under its
  pin, once the circle is wider than the pin. Near a bridge a phone's fix is often 5–10 m off,
  which matters at short spans. A position typed in Edit Info or set from a tracklog has no
  accuracy; values that are not a positive number of metres up to 10 km are ignored.
- The status line counts the photos and those with a location. **"N without location"** selects
  the photos that have none.
- **Info panel mini-map:** under the GPS field, a photo with a location gets a small map of its
  place (the Map view's closest photo zoom), with its pin and view cone. It doesn't pan or zoom;
  a click opens the Map view centred on that photo (`view.map {"photo": id}`), with the photo
  selected. It draws tiles only once they are turned on, with their attribution, and never asks
  for them itself. **Show on Map** still opens the place in OpenStreetMap in the browser.

## Markers and attachments

A **marker** is a point you place on the map (a pier, a joint, a bearing) to keep the documents
about that spot with the photos: an inspection PDF, a spreadsheet, Markdown or text notes, more
photos. It makes the Map view a light record of the site, kept with the library.

- **Add one:** right-click the map ▸ **Add Marker Here**. A marker draws as a teal diamond with a
  count of what is attached, above the photo pins and never merged into their clusters; zoomed
  far out, where a site is one named dot, it is drawn small without its count.
- **Attach files:** right-click ▸ **Attach Files Here…** (a file dialog, several files at once;
  on a marker: **Attach Files to This Marker…**), the card's **Attach Files…**, or **drop files on
  the map**: they attach to the marker whose card is open, otherwise to a new marker at the point
  they are dropped on. (Dropping files anywhere else still imports them as before.)
- **Files are copied into the library**, to `<library>/Attachments/<marker id>/<file name>`, so
  they travel with the library and the original can move or be deleted. A name already there
  gets `-1`, `-2`… (nothing is ever overwritten). A missing or unreadable file, a folder, or a full
  disk is reported for that file; the others are still attached. Documents over 16 MB in all are
  copied in the background (in the activity stack, where they can be cancelled).
- **A photo becomes a photo at that spot:** an image LightCraft reads is imported like any photo
  (copied into the library's `Originals/`), its location is set to the marker's point and it is
  listed on the marker. The location is written to the catalog and, with XMP sidecars on, to the
  sidecar; the original file's EXIF is never changed. A photo already in the library is linked
  and moved to the point instead of imported twice. It shows as an ordinary pin and in the
  filmstrip when it is in the folder, album or filter on screen.
- **The card:** a click on a marker opens it, on the right of the map. Edit the **title** and
  **note** (plain text; saved when you press Return or leave the field). Each attachment shows its
  type, name and size with **Open** (the library's copy in its default app), **Show** (in Finder /
  Explorer / the file manager) and **Remove**. **Photos here (N)** narrows the filmstrip to the
  marker's photos (`map.focus`). **Delete Marker** asks first when something is attached.
- **Opening is limited to documents and pictures** (PDF, Office and OpenDocument files, CSV, text,
  Markdown, JSON/XML/KML/GeoJSON/GPX, DXF/DWG/IFC, images, MP4/MOV) inside the marker's folder.
  Anything else, such as a program or a script, can only be shown in its folder, so a click never
  runs one.
- **Undo and removed files:** adding, editing, attaching, removing an attachment and deleting a
  marker are each one undo step. **Removing an attachment or a marker removes only the record**:
  the copy stays in `Attachments/<marker id>/`, so undo and redo always find it, and an attached
  photo stays in the library. LightCraft never deletes an attached file; to reclaim the space,
  delete the folder of a marker you no longer need yourself.
- Moving a marker (`map.updateMarker` with `lat`, `lon`) doesn't move its photos.
- **Export:** File ▸ Export Photo Locations… includes the markers (below).

From the CLI, control channel or MCP:

```sh
lightcraft-cli run --library ~/Pictures/LightCraft map.attach lat=37.8199 lon=-122.4783 \
  'paths=["inspection.pdf", "deck.xlsx", "pier-3.jpg"]'
```

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

## Export photo locations (KML, GeoJSON)

**File ▸ Export Photo Locations…** writes where the photos were taken, for an inspection report,
Google Earth or a GIS. It takes the selected photos when several are selected, otherwise every
photo the grid (and the map) shows. The file type follows the name: `.kml` (KML 2.2, Google Earth)
or `.geojson` (RFC 7946, QGIS, ArcGIS, geojson.io). Photos without a usable location (none, 0, 0,
or out of range) are left out and counted.

Each photo is one point, in capture order, with:

| Field | KML | GeoJSON |
|---|---|---|
| Name: the photo's title, else its file name | `<name>` | `name` |
| Capture time (ISO 8601, the camera's clock; zone when recorded) | `<TimeStamp><when>` and `captured` | `captured` |
| Latitude, longitude | `<coordinates>lon,lat[,alt]</coordinates>` | `coordinates: [lon, lat, alt?]` |
| Altitude (metres above sea level), when known | in the coordinates (`altitudeMode` absolute) and `altitude` | in the coordinates and `altitude` |
| Heading (degrees from north) and `headingRef` (`true` or `magnetic`), when recorded | `ExtendedData` | properties |
| `accuracy`: how far off the position may be (metres, from `GPSHPositioningError`), when recorded | `ExtendedData` | properties |
| Photo id, file name and the original's path | `ExtendedData` (`id`, `fileName`, `path`) | `id`, `fileName`, `path` |

Every photo point has `kind: photo`. The map's **markers** are added as points with `kind:
marker` (in KML, a `Markers` folder): `name` (the title, else "Marker N"), `id`, `note` (also the
KML `description`), `attachments` (the attached files' names) and `photos` (the ids of the photos
attached there); `markers: false` leaves them out. A **track** through the points in capture order is added
(turn it off with `track: false`): in KML a `Track` Placemark with a LineString, in GeoJSON a
LineString feature; both have `kind: track`, so a GIS layer can be filtered to the photos alone.
The track has altitudes only when every photo has one. The file is written whole or not at all
(a temporary file renamed into place), and never over an original or its sidecar.

From the CLI, control channel or MCP (`cmd_photo_exportLocations`):

```sh
lightcraft-cli run --library ~/Pictures/LightCraft photo.exportLocations path=bridge-1234.kml
```

Without `path` the text comes back in the result (`text`) instead.

## Commands

| Command | Params | |
|---|---|---|
| `view.map` | `{photo?}` | Open the Map view; returns `{photos, located}`. With `photo`: select it and centre on it at the closest photo zoom instead of fitting every pin |
| `map.fitAll` | | Fit every pin on screen |
| `map.focus` | `{ids}` | Narrow the filmstrip to these photos and select them (`[]`: all again) |
| `map.toggleTrack` | `{on?}` | The track line |
| `map.setTileSource` | `{source: off\|street\|imagery}` | Tiles (also Settings ▸ Map) |
| `map.clearCache` | | Delete the cached tiles (background) |
| `photo.exportLocations` | `{path?, format?: kml\|geojson, ids?, track?, markers?, name?}` | Photo locations (and the markers) as KML or GeoJSON; returns `{written, markers, skipped, format, track, path \| text}` |
| `map.addMarker` | `{lat, lon, title?, note?}` | A new marker; returns `{id, marker}` |
| `map.updateMarker` | `{id, title?, note?, lat?, lon?}` | Edit a marker; returns `{changed, marker}` |
| `map.removeMarker` | `{id}` | Delete a marker's record (the copied files stay); returns `{removed, attachments, photos}` |
| `map.attach` | `{marker \| lat, lon, paths: [file]}` | Copy files into the library and attach them (images: imported as photos at the point; a new marker at `lat`, `lon` without `marker`); returns `{marker, attached, photos, errors, card}` |
| `map.detach` | `{marker, attachment}` \| `{marker, photo}` | Take an attachment or a photo off a marker (the copy and the photo stay) |
| `map.markers` | `{id?}` | Every marker with its attachments (`path` in the library, `file`, `exists`) and photos |
| `map.attachmentPath` | `{marker, attachment}` | Where the library's copy is, and whether it is a type that opens: `{path, exists, openable, reason?}` |
| `map.openMarker` | `{id}` | Open a marker's card on the Map view (`null`: close it) |
| `map.attachFiles` | `{marker \| lat, lon, paths?}` | `map.attach` from the UI: a file dialog without `paths`; big documents copied in the background |
| `map.openAttachment` | `{marker, attachment}` | Open the library's copy with the system's default app (documents and pictures only) |
| `map.revealAttachment` | `{marker, attachment}` | Show the library's copy in its folder |
| `file.exportLocations` | `{path?, ids?}` | The same with a save dialog (File ▸ Export Photo Locations…) |

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
- `crates/engine/src/cmd/locations.rs`: `photo.exportLocations` (KML / GeoJSON; a point's fields
  are built in one place, `Point::properties`).
- `crates/catalog/src/markers.rs`: the marker record (`Op::SetMarker`, catalog format 10);
  `crates/engine/src/cmd/markers.rs`: the `map.*` marker commands, the copies and the open gate.
- `crates/ui-egui/src/panels/map/`: the view, the map widget, the pins and the markers
  (`markers.rs`: drawing, the card, the context menu, drops). The UI gets tiles
  through `Services::map_tiles` (the desktop app passes a `TilePool`; the web build and tests pass
  none or a fake, so tests never touch the network).
