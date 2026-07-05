//! Tools for working with Husqvarna/Viking Designer 1 embroidery media.
//!
//! The crate provides the shared library used by the CLI and GUI frontends:
//! - [`inkstitch`] loads and normalizes Ink/Stitch JSON into the internal model.
//! - [`model`] contains shared domain types for stitches, threads, and options.
//! - [`preview`] renders SVG previews and machine-style preview bitmaps.
//! - [`shv`] writes and validates Designer 1 `SHV` files.
//! - [`disk`] exports Designer 1 disk/menu assets such as `PHV` and `MHV`.
//! - [`gotek`] builds FAT12 floppy images and manages Gotek slot workflows.
//! - [`cli`] and [`gui`] provide reusable entrypoints for the binary frontends.
//!
//! Format behavior in this project is empirical. When changing machine-facing output,
//! prefer byte comparison against known-good files and on-machine validation over
//! inference from rendered previews.

pub mod cli;
pub mod disk;
pub mod gotek;
pub mod gui;
pub mod inkstitch;
pub mod model;
pub mod preview;
pub mod shv;
