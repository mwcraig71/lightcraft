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

## Drone orthomosaics (GeoTIFF overlays)

Public imagery is too coarse to inspect a bridge. A drone survey exported as an **orthomosaic
GeoTIFF** (DJI Terra, Pix4D, WebODM / ODM, Agisoft Metashape) draws over the map at its own
detail, centimetres a pixel, offline.

- **Add:** **Layers ▸ Add Orthomosaic…** on the map (top-left; also the command `map.addOrthomosaic`),
  then pick the `.tif`. The Map view opens fitted to it. The file is used where it is (they are
  large) and never changed.
- **Its map tiles are built in the background**, shown in the activity stack ("Building
  orthomosaic map tiles", with ✕ to stop it; **Rebuild** in the Layers panel starts it again).
  The footprint's outline is on the map at once, the imagery as the tiles are made. A 4000 ×
  4000 px orthomosaic takes a few seconds.
- **Layers** (top-left on the map): each orthomosaic with a checkbox to show or hide it, its
  resolution or build state, opacity (25–100 %), **Fit** (zoom to it), **Remove** (takes it off
  the map and deletes its tiles; the GeoTIFF stays).
- It draws above the base map and below the pins, also with map tiles **Off**. Zoomed out, a thin
  blue outline marks its footprint (with its name when small); the Info panel's mini-map draws
  it too.
- **Where:** the list is kept with the library, in `<library>/Orthomosaics/orthomosaics.json`,
  and the tiles in `<library>/Orthomosaics/<id>/<z>/<x>/<y>.jpg` (opaque) or `.png` (with
  transparent parts). The GeoTIFF's size and modification time are recorded: a changed file builds
  its tiles again on the next open; a moved or deleted one is marked "file missing" (the tiles made
  before still draw).

**What is read.** Classic TIFF or BigTIFF, tiled or in strips, uncompressed, LZW, Deflate or JPEG,
8 or 16 bits, grey, RGB or RGBA; transparency from the alpha band or GDAL's no-data value.
Placement from `ModelPixelScale` + `ModelTiepoint` or a `ModelTransformation` (rotated or skewed
grids too). Coordinate systems: WGS 84 latitude / longitude (EPSG:4326), Web Mercator (3857), WGS 84
UTM zones (32601–32660 north, 32701–32760 south) and NAD83 UTM zones (26901–26923, drawn as WGS 84:
in North America the two differ by 1–2 m). Anything else (State Plane, a local grid, a custom
WKT) is refused with the EPSG code it has: export the orthomosaic in WGS 84 or a UTM zone. Not
read: band-per-plane files, floating-point or signed samples (elevation models), strips over 64 MB
(export tiled: GDAL `-co TILED=YES`), and areas needing more than 4 million map tiles (about
15 km × 15 km at 3 cm).

**How the tiles are made.** Nothing reads the whole raster: an orthomosaic can be several GB. The
deepest zoom is the one whose map pixel is closest to the orthomosaic's (3 cm → zoom 22); each of
its tile pixels is mapped back (Web Mercator → latitude / longitude → the GeoTIFF's CRS → a source
pixel) and sampled bilinearly from the source tiles or strips it falls in, decoded on demand and
cached per worker thread. Coarser zooms, down to 12, are made from the four tiles under each.
Internal overviews are recognised but not needed: every zoom comes from the full resolution.

## Photos without a location

Phones, tablets and most drones record GPS. A DSLR usually doesn't: record a GPX track on a phone
while you shoot (any tracking app that exports GPX), then select the photos and run **Photo ▸
Auto-Tag from Tracklog…**. It matches each photo's capture time to the track (check the camera's
clock first). The Map view's empty state links to it.

## Map tiles: sources, privacy, attribution

LightCraft is local-first: **the map loads no tiles until you turn them on.** The first time the
Map view opens it asks; the answer is kept in **Settings ▸ Map** (or the Off / Street / Imagery / Esri
buttons on the map, plus your own source's button once it is set up). With tiles off the pins draw
on a plain background with a scale bar, so the view works with no connection.

| Choice (`map.setTileSource`) | Server | Real tiles to zoom | Attribution (always shown on the map) |
|---|---|---|---|
| Street (`street`) | OpenStreetMap, `tile.openstreetmap.org` | 19 | © OpenStreetMap contributors |
| Imagery (`imagery`; US and Puerto Rico) | USGS The National Map, `USGSImageryOnly` | 16 (about 60 cm a pixel at 40° N) | USDA, USGS The National Map: Orthoimagery |
| Esri (`esri`; worldwide) | Esri World Imagery, `server.arcgisonline.com/…/World_Imagery/MapServer` | 19 (about 30 cm a pixel; deeper in some metro areas, not used) | Esri — Source: Esri, Vantor, Earthstar Geographics, and the GIS User Community |
| Your own (`custom`) | any XYZ tile service, see below | what you set | what you set (required) |

Every source zooms on to 22 (about 4 cm a point): past its deepest real tiles, those are scaled
up, so pins on neighbouring piers still separate (blurrier, never blank).

**Esri World Imagery** is the sharpest built-in imagery: at zoom 19 over the Golden Gate Bridge's
south tower it shows the lane lines, single cars and the tower's truss shadows, where the USGS
tiles (scaled up from 16) show a blur. Checked against the live service (October 2026):
JPEG tiles, `Cache-Control: max-age=86400` with an `ETag`; over the Golden Gate Bridge real imagery
goes to zoom 21, but in most of the US and Europe (Philadelphia, rural Kansas, Montana, the
Mackinac Bridge, London) it stops at 19, and some regions stop sooner. Where it has no imagery the
server does not answer 404: it sends a grey "Map data not yet available" JPEG, always with the same
`ETag`. LightCraft treats that picture as no tile (a coarser tile stands in) and uses zoom 19 as
Esri's deepest, so a zoomed-in view is never a wall of grey. The attribution is the service's
`copyrightText` (from `…/World_Imagery/MapServer?f=pjson`) after "Esri", as Esri's attribution
guidelines ask.

**Licensing caveat (Esri):** `server.arcgisonline.com` serves these tiles without a key, but Esri's
terms of use expect an application that uses its basemaps to do so through an ArcGIS account
(ArcGIS Location Platform or ArcGIS Online), and to show the attribution. LightCraft includes it
because the user asked for it knowingly, for occasional inspection use, with the same politeness as
the other sources (below). For heavy or commercial use, get an ArcGIS Location Platform key and use
Esri's keyed imagery endpoint as a custom source (example below), which also has deeper tiles in
more places.

Tile requests tell the tile server which area you are looking at, never which photos you have.
They carry a `User-Agent` naming LightCraft and where to find it.

OpenStreetMap's [tile usage policy](https://operations.osmfoundation.org/policies/tiles/) is
followed, for every source: an identifying User-Agent, cache headers honoured (`Cache-Control` /
`ETag`, and tiles kept at least 7 days), at most two connections per server, no prefetching or bulk
download, and no "save this area for offline". USGS imagery is public domain; the attribution above
is still shown.

## Your own tile service (Settings ▸ Map ▸ Custom)

For imagery sharper or more current than the built-in sources (a state's orthoimagery service,
Esri's ArcGIS Location Platform, Mapbox Satellite, Google's Map Tiles API through a proxy, a
company tile server), fill in **Settings ▸ Map ▸ Custom tile source**:

- **Name:** the label of its button on the map (the "Custom" button appears once the source is
  valid).
- **URL template:** `https://` only, with `{z}` (zoom), `{x}` (column) and `{y}` (row) each exactly
  once, in whatever order the service wants, and optionally `{key}`. Anything else in braces (`{s}`
  subdomains, `{-y}`, `{r}`) is refused with a message saying what to change; so is `http://`.
- **API key:** put in place of `{key}` (URL-encoded). Needed only when the template has `{key}`.
- **Deepest zoom:** the deepest zoom the service has real tiles for (0–22); deeper views scale them
  up.
- **Attribution:** required, shown bottom-right on the map like the built-in sources'. Use the
  wording the service's terms ask for.

The URL and key fields take effect when you press Return or leave the field, so a half-typed URL
never loads tiles. The line under the fields says "Ready" or what to fix. Then choose it under
**Under the pins** or with its button on the map (`map.setTileSource {"source": "custom"}`, which
fails with the reason while the source isn't set up). Scripts can set it all at once with
`map.setCustomSource` (below).

Template examples:

| Service | URL template | Deepest zoom |
|---|---|---|
| Esri ArcGIS Location Platform (World Imagery, keyed) | `https://ibasemaps-api.arcgis.com/arcgis/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}?token={key}` | 19 (more in some areas) |
| Mapbox Satellite | `https://api.mapbox.com/v4/mapbox.satellite/{z}/{x}/{y}.jpg90?access_token={key}` | 19–22 depending on area |
| A generic XYZ server | `https://tiles.example.com/imagery/{z}/{x}/{y}.png` | as published |
| An ArcGIS MapServer (state orthoimagery) | `https://gis.example.gov/arcgis/rest/services/Ortho2024/MapServer/tile/{z}/{y}/{x}` | from the service's `?f=pjson` (`lods`) |

The service must use Web Mercator (EPSG:3857) tiles in the standard Google/OSM numbering; a
MapServer's `?f=pjson` says so under `tileInfo` (`wkid` 102100 or 3857).

**The API key is stored in LightCraft's settings file, `ui.json` in the settings folder, in plain
text** (anyone who can read your user account's files can read it; use a key restricted to tile
requests where the service allows it). It is never logged (errors are logged with the key taken
out), never shown by `Debug`, and the control channel and MCP show it as `(hidden)` (`ui.inspect`,
`ui.set`'s reply; writing that state back keeps the real key). It is not part of any export.

Its tiles are cached like the others, in `tile-cache/custom-<hash>/`, where the hash is of the URL
template (not the key): a new template gets its own folder and never shows the old one's tiles, and
changing only the key keeps the cache.

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
| `map.setTileSource` | `{source: off\|street\|imagery\|esri\|custom}` | Tiles (also Settings ▸ Map); `custom` fails with the reason while it isn't set up |
| `map.setCustomSource` | `{name?, url?, apiKey?, maxNativeZoom?, attribution?, use?}` | Change the custom tile source's fields given, check them (error: what to fix, nothing kept) and with `use: true` show it; returns `{name, maxNativeZoom, hasKey, source}` (never the key) |
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
| `orthomosaic.add` | `{path, name?}` | Add a GeoTIFF orthomosaic; returns `{id, name, crs, epsg, resolutionCm, bounds: {north, south, east, west}, corners, minZoom, maxZoom, status, …}` and builds its tiles (activity `orthomosaic`) |
| `orthomosaic.list` | | Every orthomosaic with `visible`, `opacity`, `status` (`{state: building, done, total}`, `ready` or `failed` with `error`), `progress`, `missing` |
| `orthomosaic.set` | `{id, visible?, opacity?: 0..1, name?}` | Show / hide, opacity, rename |
| `orthomosaic.remove` | `{id}` | Off the map, tiles deleted (never the GeoTIFF) |
| `orthomosaic.rebuild` | `{id}` | Build its tiles again |
| `map.addOrthomosaic` | `{path?, name?}` | `orthomosaic.add` with a file dialog (Layers ▸ Add Orthomosaic…), then the Map view fitted to it |
| `map.fitOrthomosaic` | `{id, zoom?}` | Open the Map view fitted to an orthomosaic (`zoom`: at that zoom on its middle) |
| `map.layers` | `{open?}` | The Layers panel |
| `file.exportLocations` | `{path?, ids?}` | The same with a save dialog (File ▸ Export Photo Locations…) |

All go through the control channel and MCP like any UI command, e.g. a headless snapshot script:

```jsonl
{"method":"engine.execute","params":{"command":"photo.setMeta","params":{"ids":[1],"gps":[40.0, -75.0]}}}
{"method":"engine.execute","params":{"command":"view.map"}}
{"method":"ui.settle","params":{"timeoutMs":3000}}
{"method":"ui.screenshot","params":{"path":"map.png"}}
```

## Code

- `crates/ortho` (`lightcraft-ortho`, layer 2): GeoTIFF georeferencing, the CRSs (transverse
  Mercator by Krüger's series), windowed reads, the tile pyramid. `crates/engine/src/orthomosaic.rs`:
  the library's list and the build jobs; `crates/ui-egui/src/panels/map/ortho.rs`: drawing, Layers.
- `crates/tiles` (`lightcraft-tiles`, layer 2): Web Mercator math, the tile sources (built-in
  statics and `TileSource::custom`, which checks a template and keeps the key private), the disk
  cache and the fetch workers (`TilePool`, the native `TileProvider`). No UI.
- `crates/engine/src/cmd/locations.rs`: `photo.exportLocations` (KML / GeoJSON; a point's fields
  are built in one place, `Point::properties`).
- `crates/catalog/src/markers.rs`: the marker record (`Op::SetMarker`, catalog format 10);
  `crates/engine/src/cmd/markers.rs`: the `map.*` marker commands, the copies and the open gate.
- `crates/ui-egui/src/panels/map/`: the view, the map widget, the pins and the markers
  (`markers.rs`: drawing, the card, the context menu, drops). The UI gets tiles
  through `Services::map_tiles` (the desktop app passes a `TilePool`; the web build and tests pass
  none or a fake, so tests never touch the network).
