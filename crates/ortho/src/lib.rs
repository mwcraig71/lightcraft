//! Drone orthomosaics on the Map view (layer 2, no UI): the coordinate systems an orthomosaic
//! may be in ([`crs`], [`tm`]).
//!
//! Sources: Krüger / Karney transverse Mercator series (see [`tm`]). Own implementation.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod crs;
pub mod tm;

pub use crs::Crs;
