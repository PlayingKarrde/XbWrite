//! Core library for turning Xbox disc images into trimmed XISO (or CSO) images.
//!
//! Redump-style ISOs carry the DVD video partition and the random filler the pressing plant
//! put between files. The game itself only needs its XDVDFS file system, so a conversion
//! rebuilds the image from the files alone with [xdvdfs](https://github.com/antangelo/xdvdfs).

pub mod convert;
pub mod error;
pub mod extract;
pub mod scan;
pub mod source;
pub mod util;
pub mod xbe;
pub mod xpr;

mod split;
mod trim;

#[cfg(test)]
pub(crate) mod testutil;

pub use error::{Error, Result};
