//! Conversion core: encoder implementations behind the [`ImageEncoder`]
//! trait; orchestration (pipeline, stats, progress, ctrl+c) lives in
//! `crate::pipeline`.

/// This module provides avif conversion via the ravif crate
pub mod avif;
/// This module provides jpeg-xl conversion via in-tree libjxl FFI bindings (WS2)
#[cfg(feature = "jxl")]
pub mod jxl;
/// This module provides optimized jpeg conversion via the mozjpeg crate
pub mod mozjpeg;
/// This module provides optimal png conversion via the oxipng crate
#[cfg(feature = "opt-oxipng")]
pub mod oxipng;
/// This module provides png conversion via the image crate
pub mod png;
/// This module defines the encoder abstraction
pub mod traits;
/// This module provides webp conversion via the webp crate
pub mod webp;
/// This module provides webp conversion via the image crate
pub mod webp_image;

// Include dependency version numbers
include!(concat!(env!("OUT_DIR"), "/versions.rs"));

pub use traits::{EncoderRegistry, ImageEncoder, ThreadBudget};
