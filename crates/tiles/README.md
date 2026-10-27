# lightcraft-tiles

Map tiles for LightCraft's Map view (layer 2):

- `mercator`: Web Mercator world coordinates, tile numbering, visible tile ranges (wrapping at the
  antimeridian), fitting a set of points, metres per pixel for the scale bar.
- `source`: the tile servers (OpenStreetMap street map, USGS The National Map imagery), their zoom
  ranges, overzoom past the deepest native zoom, and the attribution each needs.
- `cache` (native): tiles on disk with expiry and `ETag`, bounded in bytes with LRU eviction.
- `pool` (native): `TilePool`, the `TileProvider` the desktop app gives the UI. Two workers per
  server, conditional requests, stale tiles used when offline, requests for tiles that left the
  view dropped or cancelled.

Nothing here goes to the network until the user turns map tiles on. See `docs/map.md`.
