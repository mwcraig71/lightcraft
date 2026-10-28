# lightcraft-ortho

Drone orthomosaics for the Map view (layer 2, no UI):

- `georef`: GeoTIFF georeferencing: `ModelPixelScale` + `ModelTiepoint` or `ModelTransformation`
  (rotated / skewed affine), the GeoKey directory, pixel-is-point.
- `crs`, `tm`: the coordinate systems an orthomosaic may be in: WGS 84 (EPSG:4326), Web Mercator
  (3857), WGS 84 UTM (32601–32660, 32701–32760) and NAD83 UTM (26901–26923, treated as WGS 84).
  Transverse Mercator by Krüger's series (tested against Snyder's series and worked example).
  Anything else is an error naming the EPSG code.
- `raster`: the full-resolution image read a strip or tile at a time (classic TIFF or BigTIFF;
  uncompressed, LZW, Deflate or JPEG; 8 or 16 bit; grey, RGB, RGBA, YCbCr), no-data from
  `GDAL_NODATA` or the alpha band. Never the whole raster.
- `pyramid`: Web Mercator tiles on disk (`<dir>/<z>/<x>/<y>.jpg|png`), the native zoom from the
  source by inverse mapping and bilinear sampling, coarser zooms down to 12 from the tiles under
  them; progress and cancel for the activity stack.
- `testdata` (feature): a minimal GeoTIFF writer for tests and demos. No media in the repo.

See `docs/map.md` → Orthomosaics.
