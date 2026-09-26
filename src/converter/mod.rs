/// This module provides webp conversion via the webp crate
pub mod webp;
/// This module provides avif conversion via the ravif crate
pub mod avif;
/// This module provides webp conversion via the image crate
pub mod webp_image;
/// This module provides png conversion via the image crate
pub mod png;
mod mozjpeg;

use crate::{
    converter::avif::encode_avif,
    converter::avif::{AlphaColorMode, BitDepth, ColorModel},
    converter::webp::encode_webp,
    converter::webp_image::encode_webp_image,
    converter::png::encode_png,
    converter::png::{CompressionType, FilterType},
    converter::mozjpeg::encode_mozjpeg,
    format::ImageFormat,
    Error,
};
use std::{
    collections::{LinkedList},
    fs,
    path::{Path, PathBuf},
    error::Error as StdError,
    sync::{Arc, atomic::AtomicBool},
    panic
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use image::{ImageReader, ImageFormat as ImageImageFormat, DynamicImage, RgbImage, Limits};
use rayon::prelude::*;
use humansize::{format_size, FormatSizeOptions, BINARY};
use indicatif::{HumanDuration, ProgressBar, ProgressStyle};
use jpeg_decoder::Decoder;

// Include dependency version numbers
include!(concat!(env!("OUT_DIR"), "/versions.rs"));

/// Configuration parameters shared across all encoders
#[derive(Clone)]
pub struct CommonConfig {
    /// Glob pattern to match images to convert.
    /// Example: `images/**/*.png`
    pub pattern: String,

    /// Output directory (flat) of processed images.
    /// Defaults to the same location as the original images with the new file extension.
    pub output: String,

    /// By default, byteshaver will process input files in lexicographical order after expanding the pattern.
    /// Setting this starts the process from the back.
    /// Defaults to false.
    pub reverse_processing_order: bool,

    /// Overwrite the existing output file if the current conversion resulted in a smaller file.
    /// Defaults to false.
    pub overwrite_if_smaller: bool,

    /// Overwrite existing outputs?
    /// Defaults to false. (Determined by filename match)
    pub overwrite_existing: bool,

    /// Discards the encoding result if it is larger than the input file (does not create an output file).
    /// Defaults to false.
    pub discard_if_larger_than_input: bool,

    /// Discards the alpha channel of the image if it is present.
    /// Defaults to false.
    pub discard_input_alpha_channel: bool,
}

fn handle_conversion_error(path: PathBuf, err: Box<dyn StdError + Send + Sync>) -> (i32, i32, i32) {
    // carriage return and clear line contents (do not spam screen content with logger bar states)
    println!("\r\x1b[2KFile {}: could not be converted, error: {}", path.display() , err);
    (-2, 0, 0)
}

fn base_from_pattern(pattern: &str) -> String {
    let mut base = PathBuf::new();

    for part in Path::new(pattern) {
        let s = part.to_string_lossy();
        if s.contains('*') || s.contains('?') || s.contains('[') {
            break;
        }
        base.push(part);
    }

    base.to_string_lossy().to_string()
}

/// Processes and encodes images in a given directory to the specified image format.
pub fn convert_images(
    conf: CommonConfig,
    img_format: &ImageFormat,
    option_lossless: &Option<bool>,
    option_quality: &Option<f32>,
    option_speed: &Option<u8>,
    option_png_compression_type: &Option<CompressionType>,
    option_png_filter_type: &Option<FilterType>,
    option_avif_bit_depth: &Option<BitDepth>,
    option_avif_color_model: &Option<ColorModel>,
    option_avif_alpha_color_mode: &Option<AlphaColorMode>,
    option_avif_alpha_quality: &Option<f32>,
) -> Result<(), Error> {
    let mut paths: Vec<PathBuf> = glob::glob(&*conf.pattern)?
        .filter_map(|entry| entry.ok())
        .filter(|path|{
            let format = ImageFormat::from(path.as_path());
            format != ImageFormat::Unknown
                && format != ImageFormat::Avif // disable reading avif (FIXME: re-enable with reliable build+integration for reader)
        })
        .collect();
    // sort paths lexicographically, not only filenames
    paths.sort_by(|a, b| {
        let dir_cmp = a.parent().cmp(&b.parent());
        let cmp = if dir_cmp != std::cmp::Ordering::Equal {
            dir_cmp
        } else {
            a.file_name().cmp(&b.file_name())
        };

        if conf.reverse_processing_order {
            cmp.reverse()
        } else {
            cmp
        }
    });
    // TODO: check for collision candidates (same filename but different extensions => same encoded output filename format...)
    //  and come up with a solution
    let pattern_base = base_from_pattern(&conf.pattern);

    if paths.is_empty() {
        println!("No images to convert, check input glob pattern and supported input formats.");
        return Ok(());
    }

    // create output directory if it does not exist
    if ! conf.output.is_empty() {
        let output_directory = Path::new(&conf.output);
        if ! fs::exists(output_directory)? {
            // is it possible to warn in docker if the target output directory is not host mounted?
            println!("Creating output directory {:?}", output_directory);
            fs::create_dir_all(output_directory).unwrap_or_else(|err| {
                eprintln!("Error creating the output directory: {err}");
                std::process::exit(1);
            });
        }
    }
    // IDEA: create output filename from configurable regex

    println!("Converting {} files...", paths.len());
    let encoder_data = match img_format {
        ImageFormat::Webp => webp::encoder_info(option_lossless.unwrap_or(false), option_quality.unwrap_or(90.)),
        ImageFormat::WebpImage => webp_image::encoder_info(),
        ImageFormat::Avif => avif::encoder_info(option_quality.unwrap_or(90.), option_speed.unwrap_or(3), None, None),
        ImageFormat::Png => png::encoder_info(),
        ImageFormat::Jpeg => mozjpeg::encoder_info(),
        _ => "unknown encoder".parse().unwrap(),
    };
    println!("{}", encoder_data);

    let global_stop = Arc::new(AtomicBool::new(false));
    let stop_signal = global_stop.clone();
    let mut ctrlc_counter = 0;
    ctrlc::set_handler(move || {
        if !global_stop.load(std::sync::atomic::Ordering::Relaxed) {
            println!("received Ctrl+C, stopping further queue processing!");
            global_stop.store(true, std::sync::atomic::Ordering::Relaxed);
        } else {
            println!("an encoding task is still active!{} processing will end afterwards.", str::repeat("!", ctrlc_counter));
        }
        ctrlc_counter += 1;
    }).expect("Error setting Ctrl-C handler");


    let (tx, rx) = mpsc::channel::<PathBuf>();
    let input_file_count = paths.len() as u64;
    // producer thread: feed paths in lexicographic order
    std::thread::spawn(move || {
        for path in paths {
            if tx.send(path).is_err() {
                break; // consumer dropped, exit
            }
        }
        // close the channel
        drop(tx);
    });

    let pb = ProgressBar::new(input_file_count);
    let style = ProgressStyle::with_template("[{elapsed_precise}/~{duration_precise} ({eta_precise} rem.)] {wide_bar:.cyan/blue} {pos:>7}/{len:7} | {msg}").unwrap();
    pb.set_style(style);
    let encode_successful = Arc::new(AtomicUsize::new(0));
    let encode_skipped = Arc::new(AtomicUsize::new(0));
    let encode_discarded = Arc::new(AtomicUsize::new(0));
    let encode_errors = Arc::new(AtomicUsize::new(0));
    let size_input_total = Arc::new(AtomicUsize::new(0));
    let size_output_total = Arc::new(AtomicUsize::new(0));
    let size_input_preexisting = Arc::new(AtomicUsize::new(0));
    let size_output_preexisting = Arc::new(AtomicUsize::new(0));
    let size_input_discarded = Arc::new(AtomicUsize::new(0));
    let size_output_discarded = Arc::new(AtomicUsize::new(0));
    let format_option_binary_two_nospace = FormatSizeOptions::from(BINARY)
        .decimal_places(2).decimal_zeroes(2).space_after_value(false);

    let _results: LinkedList<(isize, usize, usize)> = rx.into_iter()
        .par_bridge()
        .map(|path| {
            let res = if stop_signal.load(std::sync::atomic::Ordering::Relaxed) {
                return (-2, 0, 0);
            } else {
                convert_image(
                    &*path, img_format,
                    conf.output.clone(), pattern_base.clone(), conf.overwrite_if_smaller,
                    conf.overwrite_existing, conf.discard_if_larger_than_input,
                    conf.discard_input_alpha_channel,
                    option_lossless, option_quality, option_speed,
                    option_png_compression_type, option_png_filter_type,
                    option_avif_bit_depth, option_avif_color_model, option_avif_alpha_color_mode, option_avif_alpha_quality
                )
            }.map_err(|err| handle_conversion_error(path, err)).unwrap_or_else(|_| (-1, 0, 0));
            pb.inc(1); // increment progress bar counter
            match res.0 {
                0 => {
                    encode_successful.fetch_add(1, Ordering::SeqCst);
                    size_input_total.fetch_add(res.1, Ordering::SeqCst);
                    size_output_total.fetch_add(res.2, Ordering::SeqCst);
                }, // improve: track input/output size here and show interactively
                1 => {
                    encode_skipped.fetch_add(1, Ordering::SeqCst);
                    size_input_total.fetch_add(res.1, Ordering::SeqCst);
                    size_output_total.fetch_add(res.2, Ordering::SeqCst);
                    size_input_preexisting.fetch_add(res.1, Ordering::SeqCst);
                    size_output_preexisting.fetch_add(res.2, Ordering::SeqCst);
                },
                2 => {
                    encode_discarded.fetch_add(1, Ordering::SeqCst);
                    size_input_discarded.fetch_add(res.1, Ordering::SeqCst);
                    size_output_discarded.fetch_add(res.2, Ordering::SeqCst);
                },
                -1 => {
                    encode_errors.fetch_add(1, Ordering::SeqCst);
                },
                _ => {}
            }
            pb.set_message(
                if size_input_preexisting.load(Ordering::Relaxed) > 0 {
                    format!(
                        "{} ➜ {} ({} ➜ {} preexisting) | ✔ {} — {} ✖ {}",
                        format_size(size_input_total.load(Ordering::Relaxed), format_option_binary_two_nospace),
                        format_size(size_output_total.load(Ordering::Relaxed), format_option_binary_two_nospace),
                        format_size(size_input_preexisting.load(Ordering::Relaxed), format_option_binary_two_nospace),
                        format_size(size_output_preexisting.load(Ordering::Relaxed), format_option_binary_two_nospace),
                        encode_successful.load(Ordering::Relaxed),
                        encode_skipped.load(Ordering::Relaxed),
                        encode_errors.load(Ordering::Relaxed)
                    )
                } else {
                    format!(
                        "{} ➜ {} | ✔ {} — {} ✖ {}",
                        format_size(size_input_total.load(Ordering::Relaxed), format_option_binary_two_nospace),
                        format_size(size_output_total.load(Ordering::Relaxed), format_option_binary_two_nospace),
                        encode_successful.load(Ordering::Relaxed),
                        encode_skipped.load(Ordering::Relaxed),
                        encode_errors.load(Ordering::Relaxed)
                    )
                }
            );
            res
        })
        .collect();

    // use a return carriage feed to clear the remnants of the progress bar off the screen
    pb.finish_with_message("finished!");
    // \r\x1b[2K is the sequence to clear the current row content (if manual way is intended)
    println!("Encode statistics:");
    println!("Time taken:  {}", HumanDuration(pb.elapsed()));
    println!("Input files: {}", input_file_count);
    println!("Successful:  {}", encode_successful.load(Ordering::Relaxed));
    println!("Skipped:     {}", encode_skipped.load(Ordering::Relaxed));
    println!("Errors:      {}", encode_errors.load(Ordering::Relaxed));
    if conf.discard_if_larger_than_input && encode_discarded.load(Ordering::Relaxed) > 0 {
        println!("Discarded:   {} (due to the encode being larger than the input; {} ➜ {})",
                 encode_discarded.load(Ordering::Relaxed),
                 format_size(size_input_discarded.load(Ordering::Relaxed), format_option_binary_two_nospace),
                 format_size(size_output_discarded.load(Ordering::Relaxed), format_option_binary_two_nospace));
        println!("Please note that discarded in- and outputs do not count into the total in-/output statistics below.")
    }
    if size_input_total.load(Ordering::Relaxed) > 0 && size_output_total.load(Ordering::Relaxed) > 0 {
        // show total stats
        println!("Total input size:  {}", format_size(size_input_total.load(Ordering::Relaxed), format_option_binary_two_nospace));
        println!("Total output size: {}", format_size(size_output_total.load(Ordering::Relaxed), format_option_binary_two_nospace));
        println!("Total comp. ratio: {:.02}%", size_output_total.load(Ordering::Relaxed) as f64 / size_input_total.load(Ordering::Relaxed) as f64 * 100.0);
        if size_input_preexisting.load(Ordering::Relaxed) > 0 && size_output_preexisting.load(Ordering::Relaxed) > 0 {
            if size_input_total.load(Ordering::Relaxed) - size_input_preexisting.load(Ordering::Relaxed) > 0 {
                // if we have new encodes and preexisting images, first show the stats for the new encodes, then for the preexisting ones
                println!("New encodes input size:  {}", format_size(size_input_total.load(Ordering::Relaxed) - size_input_preexisting.load(Ordering::Relaxed), format_option_binary_two_nospace));
                println!("New encodes output size: {}", format_size(size_output_total.load(Ordering::Relaxed) - size_output_preexisting.load(Ordering::Relaxed), format_option_binary_two_nospace));
                println!("New encodes comp. ratio: {:.02}%", size_output_preexisting.load(Ordering::Relaxed) as f64 / size_input_preexisting.load(Ordering::Relaxed) as f64 * 100.0);
            }
            // if we have preexisting images, show these stats
            println!("Preexisting input size:  {}", format_size(size_input_preexisting.load(Ordering::Relaxed), format_option_binary_two_nospace));
            println!("Preexisting output size: {}", format_size(size_output_preexisting.load(Ordering::Relaxed), format_option_binary_two_nospace));
            println!("Preexisting comp. ratio: {:.02}%", size_output_preexisting.load(Ordering::Relaxed) as f64 / size_input_preexisting.load(Ordering::Relaxed) as f64 * 100.0);
        }
    } else {
        if (encode_successful.load(Ordering::Relaxed) + encode_skipped.load(Ordering::Relaxed) + encode_errors.load(Ordering::Relaxed)) > 1 {
            println!("Input and output size could not be determined, please try using OS-native binaries.");
        }
    }
    Ok(())
}

fn fallback_retry_read_image(input_path: &Path, input_error: Box<dyn StdError + Send + Sync>)
    -> Result<DynamicImage, Box<dyn StdError + Send + Sync>> {
    let err = input_error;
    let ext = input_path
        .extension().and_then(|e| e.to_str())
        .unwrap_or("").to_ascii_lowercase();

    // try jpeg-decoder to support loading progressive jpegs
    if ext == "pjpeg" || ext == "jpg" || ext == "jpeg" {
        if let Ok(file) = fs::File::open(input_path) {
            let mut decoder = Decoder::new(file);
            if let Ok(pixels) = decoder.decode() {
                if let Some(info) = decoder.info() {
                    // Convert raw pixels to RgbImage
                    let img = RgbImage::from_raw(
                        info.width.into(),
                        info.height.into(),
                        pixels,
                    )
                        .ok_or("Failed to convert jpeg-decoder output to RgbImage")?;
                    return Ok(DynamicImage::ImageRgb8(img));
                }
            }
        }
    }

    let mut reader = ImageReader::open(input_path)?;
    // disable image-rs decoding limits
    reader.limits(Limits::no_limits());
    match ext.as_str() {
        "pjpeg" | "jpg" | "jpeg" => reader.set_format(ImageImageFormat::Jpeg),
        "x-png" | "png" => reader.set_format(ImageImageFormat::Png),
        _ => return Err(err), // nothing else to try
    }

    if let Ok(decoded) = reader.decode() {
        Ok(decoded)
    } else {
        Err(err)
    }
}

fn try_read_image(input_path: &Path)
    -> Result<DynamicImage, Box<dyn StdError + Send + Sync>> {
    // first try with autodetection, unfortunately zune panics on one of the input images...
    let mut result = panic::catch_unwind(|| {
        let mut reader = ImageReader::open(input_path)?;
        // disable image-rs decoding limits
        //alternative: let mut limits = Limits::default(); limits.max_alloc = Some(4 * 1024 * 1024 * 1024);
        reader.limits(Limits::no_limits());
        Ok(reader.decode()?)
    });

    if let Ok(inner) = result {
        if let Ok(img) = inner {
            return Ok(img); // ✅ move out
        }
    }

    // retry with guessed format (we have pngs hiding in jpeg extension files, jpg inside bmp, etc. ...)
    result = panic::catch_unwind(|| {
        let mut reader = ImageReader::open(input_path)?;
        reader.limits(Limits::no_limits());
        Ok(reader.with_guessed_format()?.decode()?)
    });

    if let Ok(inner) = result {
        if let Ok(img) = inner {
            return Ok(img); // ✅ move out
        } else {
            fallback_retry_read_image(input_path, inner.err().unwrap())
        }
    } else {
        fallback_retry_read_image(input_path, result.unwrap().err().unwrap())
    }
}

fn normalize_prefix<P: AsRef<Path>>(p: P) -> PathBuf {
    let path = p.as_ref();

    let mut components = path.components().peekable();
    let mut normalized = PathBuf::new();

    // Skip leading CurrentDir (`.`) if present
    while let Some(c) = components.peek() {
        if c.as_os_str() == "." {
            components.next();
        } else {
            break;
        }
    }

    for c in components {
        normalized.push(c);
    }

    normalized
}

/// trait to enable discard_input_alpha_channel functionality
trait IntoWithoutAlpha {
    fn into_without_alpha(self) -> DynamicImage;
}

impl IntoWithoutAlpha for DynamicImage {
    fn into_without_alpha(self) -> DynamicImage {
        match self {
            DynamicImage::ImageLumaA8(_) =>
                DynamicImage::ImageLuma8(self.into_luma8()),
            DynamicImage::ImageLumaA16(_) =>
                DynamicImage::ImageLuma16(self.into_luma16()),

            DynamicImage::ImageRgba8(_) =>
                DynamicImage::ImageRgb8(self.into_rgb8()),
            DynamicImage::ImageRgba16(_) =>
                DynamicImage::ImageRgb16(self.into_rgb16()),
            DynamicImage::ImageRgba32F(_) =>
                DynamicImage::ImageRgb32F(self.into_rgb32f()),

            // otherwise input already has no alpha channel
            other => other,
        }
    }
}

/// returns the buffer size of the DynamicImage in B
pub fn buffer_size_bytes(img: &DynamicImage) -> u64 {
    let (w, h) = (img.width(), img.height());
    let bytes_per_pixel = img.color().bytes_per_pixel() as u64;

    w as u64 * h as u64 * bytes_per_pixel
}

/// Encodes an image to the specified image format and saves it to the specified output directory.
///
/// Returns tuple (isize, usize, usize), (status, input_size (B), output_size (B))
///
/// Status codes:
/// 2 = encode larger than input, output file not saved;
/// 1 = skipped;
/// 0 = success;
/// -1 = error;
/// -2 = aborted (interrupt / ctrl+c received)
fn convert_image(
    input_path: &Path,
    img_format: &ImageFormat,
    output: String,
    pattern_base: String,
    overwrite_if_smaller: bool,
    overwrite_existing: bool,
    discard_if_larger_than_input: bool,
    discard_input_alpha_channel: bool,
    option_lossless: &Option<bool>,
    option_quality: &Option<f32>,
    option_speed: &Option<u8>,
    option_png_compression_type: &Option<CompressionType>,
    option_png_filter_type: &Option<FilterType>,
    option_avif_bit_depth: &Option<BitDepth>,
    option_avif_color_model: &Option<ColorModel>,
    option_avif_alpha_color_mode: &Option<AlphaColorMode>,
    option_avif_alpha_quality: &Option<f32>,
) -> Result<(isize, usize, usize), Box<dyn StdError + Send + Sync>> {
    // returns tuple (status, input_size (B), output_size (B))
    // status:
    // 2 = would have been larger than input or existing file, output file not saved (show as skipped, but seperate statistics
    // 1 = skipped,
    // 0 = success,
    // -1 = error,
    // -2 = aborted (interrupt / ctrl+c received)
    let ext = img_format.extension();
    let output_path;
    if output.is_empty() {
        output_path = input_path.with_extension(ext)
    } else {
        let pattern_base_norm = normalize_prefix(&pattern_base);
        let input_path_norm = normalize_prefix(&input_path);
        let rel_path = input_path_norm
            .strip_prefix(&pattern_base_norm)
            .unwrap_or_else(|_| Path::new(&input_path_norm));

        output_path = Path::new(&output)
            .join(rel_path.parent().unwrap_or_else(|| Path::new("")))
            .join(input_path_norm.file_stem().unwrap())
            .with_extension(ext);

        fs::create_dir_all(Path::new(&output).join(rel_path.parent().unwrap_or_else(|| Path::new(""))))?;
    };

    let input_size = fs::metadata(&input_path)?.len() as usize;
    if fs::exists(output_path.clone())? && !overwrite_existing && !overwrite_if_smaller {
        // file exists, and we do not have any overwrite flag on? => return early
        //println!("skipped because output path exists and overwrite options are unset {}", input_path.display());
        return Ok((1, input_size, fs::metadata(output_path.clone())?.len() as usize))
    }

    let image = if discard_input_alpha_channel {
        try_read_image(input_path)?.into_without_alpha()
    } else {
        try_read_image(input_path)?
    };

    let encode_lossless = option_lossless.unwrap_or(false);
    let encode_quality: f32 = option_quality.unwrap_or(90.);
    let encode_speed: u8 = option_speed.unwrap_or(3);

    const HUGE_IMAGE_DIMENSION_LIMIT: u32 = 8192;
    if image.height() > HUGE_IMAGE_DIMENSION_LIMIT || image.height() > HUGE_IMAGE_DIMENSION_LIMIT {
        let format_option_binary_two_nospace = FormatSizeOptions::from(BINARY)
            .decimal_places(2).decimal_zeroes(2).space_after_value(false);
        println!("Trying to encode huge image \"{}\" (filesize: {}, dimensions: {}x{}px, decoded buffer: {})...",
                 input_path.display(),
                 format_size(input_size, format_option_binary_two_nospace),
                 image.width(), image.height(),
                 format_size(buffer_size_bytes(&image), format_option_binary_two_nospace)
                 );
    }

    let image_data = match img_format {
        // TODO: more PNG lossless optimizers, jpeg xl
        ImageFormat::Webp => encode_webp(&image, encode_lossless, encode_quality),
        ImageFormat::WebpImage => encode_webp_image(&image),
        ImageFormat::Avif => encode_avif(
            &image, encode_quality, encode_speed,
            *option_avif_bit_depth, *option_avif_color_model,
            *option_avif_alpha_color_mode, option_avif_alpha_quality.unwrap_or(90.)),
        ImageFormat::Png => encode_png(&image, *option_png_compression_type, *option_png_filter_type),
        ImageFormat::Jpeg => encode_mozjpeg(&image),
        _ => return Err(Box::new(Error::from_string("Unsupported image format".to_string()))),
    };

    match image_data {
        Ok(image_data) => {
            let output_size =  image_data.len();
            if fs::exists(output_path.clone())? &&
                output_size >= fs::metadata(output_path.clone())?.len() as usize &&
                overwrite_if_smaller {
                // overwrite if smaller flag is on, but output exists and is already smaller than our encode
                //  => abort
                // TODO: how to propagate this information upwards into statistics? i am not happy with the current handling
                //println!(
                //    "skipped because output path exists,\
                //      overwrite_if_smaller is active,\
                //      but new output is larger than the existing one {}",
                //    input_path.display());
                return Ok((1, input_size, fs::metadata(output_path.clone())?.len() as usize));
            }

            if discard_if_larger_than_input && output_size >= input_size {
                // TODO: how to propagate this information upwards into statistics?
                //println!(
                //    "skipped because the output is larger than the input,\
                //      and discard_if_larger_than_input is active {}",
                //    input_path.display());
                return Ok((2, input_size, output_size));
            }

            fs::write(output_path.clone(), image_data)?;
            Ok((0, input_size, output_size))
        }
        Err(e) => {
            Err(Box::new(Error::from_string(format!("Image encoding failed: {:?}", e))))
        }
    }
}
