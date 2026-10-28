//! Drone orthomosaics on the Map view (layer 2, no UI): read a GeoTIFF's georeferencing
//! ([`georef`]) in one of the supported coordinate systems ([`crs`], [`tm`]), read its pixels a
//! strip or tile at a time ([`raster`]), and turn it into Web Mercator map tiles on disk
//! ([`pyramid`]) that the map draws over its base tiles, offline, down to centimetres.
//!
//! An orthomosaic can be several GB (BigTIFF, 50 000 px a side): nothing here reads more than
//! the strips or tiles a map tile needs, a bounded number of them cached per worker.
//!
//! Sources: GeoTIFF 1.0 / OGC GeoTIFF 1.1 (tags and keys), the TIFF 6.0 specification, Krüger /
//! Karney transverse Mercator series (see [`tm`]), the slippy-map tile scheme. Own implementation;
//! TIFF decoding by the `tiff` crate (MIT).

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod crs;
pub mod georef;
pub mod raster;
pub mod tm;

#[cfg(any(test, feature = "testdata"))]
pub mod testdata;

pub use crs::Crs;

/// Why an orthomosaic can't be read or tiled.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(String),
    /// A TIFF without georeferencing.
    #[error("{0}")]
    NotGeo(String),
    /// A CRS, sample format or layout LightCraft doesn't read.
    #[error("{0}")]
    Unsupported(String),
    #[error("the orthomosaic is damaged or not a TIFF: {0}")]
    Invalid(String),
    #[error("the orthomosaic is too large to tile: {0}")]
    TooLarge(String),
    #[error("cancelled")]
    Cancelled,
}

/// Run `f`, turning a panic in a decoder into an error (a last-resort guard: a hostile file must
/// never take the app down).
pub fn guarded<T>(what: &str, f: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(_) => Err(Error::Invalid(format!("{what} failed unexpectedly"))),
    }
}
