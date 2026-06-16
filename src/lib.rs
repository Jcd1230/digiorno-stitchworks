//! Experimental tools for converting Ink/Stitch JSON into Husqvarna/Viking Designer 1 `.SHV` files.
//!
//! This crate intentionally separates the converter core from the CLI and egui app:
//! - [`inkstitch`] parses and normalizes Ink/Stitch JSON.
//! - [`preview`] renders a small 4bpp preview bitmap or SVG path preview.
//! - [`shv`] writes and validates the empirical SHV layout.
//! - [`model`] contains the shared domain model.

pub mod disk;
pub mod inkstitch;
pub mod model;
pub mod preview;
pub mod shv;
