/*!
# Image Converter `byteshaver`

`byteshaver` is a command-line utility focusing on converting images into other formats,
 specifically focusing on support for modern image standards and encoders.

`byteshaver` simplifies the process of batch converting images,
 optimizing for both performance and storage efficiency.

*/

#![deny(missing_docs)]
/// Command-line interface functionality.
pub mod cli;
/// User-facing configuration types (global + encoder configuration).
pub mod config;
/// Image conversion functionality.
pub mod converter;
/// Error handling for the application.
mod error;
/// Image formats supported by the application.
pub mod format;
/// Decode layer producing source images from files.
pub mod input;
/// Metadata containers decoded from input images.
pub mod metadata;
/// Conversion pipeline (naming, collisions, statistics, entry point).
pub mod pipeline;

/// Utility functions and helpers.
pub mod utils;

pub use config::{ConversionConfig, EncoderConfig};
pub use error::Error;
pub use pipeline::{CollisionPolicy, Outcome, RunStats, run};
