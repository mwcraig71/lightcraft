//! Map tiles for the Map view: Web Mercator math ([`mercator`]), the tile servers LightCraft can
//! show ([`source`]), and, natively, a disk cache ([`cache`]) and a small pool of fetch workers
//! ([`pool`]) behind the [`TileProvider`] trait the UI draws from. See docs/map.md.
//!
//! Nothing here touches the network on its own: the UI asks for the tiles it shows, only after the
//! user has turned map tiles on. On wasm32 only the math, the sources and the trait are built (the
//! web build draws pins without tiles).
//!
//! Sources: Web Mercator / slippy-map tile numbering (OpenStreetMap wiki, prose); the OSM tile
//! usage policy; USGS The National Map and Esri World Imagery service descriptions (their
//! `?f=pjson` JSON). Own implementation.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod mercator;
pub mod source;

#[cfg(not(target_arch = "wasm32"))]
pub mod cache;
#[cfg(not(target_arch = "wasm32"))]
pub mod pool;

use std::sync::Arc;

pub use mercator::TileId;
pub use source::TileSource;

/// A decoded tile: 8-bit sRGB RGBA, row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// What a provider has for a tile right now.
#[derive(Clone, Debug)]
pub enum TileState {
    Ready(Arc<TileImage>),
    /// Asked for; still loading.
    Pending,
    /// The server has no tile here, or it failed (retried later).
    Missing,
}

/// Where the Map view gets its tiles. Every call returns at once; loading happens elsewhere.
pub trait TileProvider: Send + Sync {
    /// The tile, or [`TileState::Pending`] after queueing it. Call it for every tile the view
    /// shows this frame, then [`TileProvider::end_frame`].
    fn get(&self, source: &TileSource, tile: TileId) -> TileState;
    /// Drop the queued and in-flight requests not asked for since the last call (tiles that left the view).
    fn end_frame(&self);
    /// Network requests sent so far (tests check that none are made with tiles off).
    fn requests(&self) -> u64;
    /// Bytes in the disk cache.
    fn cache_bytes(&self) -> u64;
    /// Change the disk cache's size limit (evicts the least recently used tiles over it).
    fn set_cache_limit(&self, bytes: u64);
    /// Delete every cached tile; returns the bytes freed.
    fn clear_cache(&self) -> Result<u64, String>;
}

/// The User-Agent tile requests send, naming the app and where to find it (OSM tile usage policy).
pub fn user_agent() -> String {
    format!("LightCraft/{} (+https://github.com/mwcraig71/lightcraft)", env!("CARGO_PKG_VERSION"))
}
