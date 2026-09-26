use crate::{Error, format::ImageFormat};
use glob::glob;
use humansize::{BINARY, FormatSizeOptions, format_size};
use std::{fs, path::Path};

/// Checks if the image format of the given path is supported, ignoring a specific format.
///
/// Only the first bytes of the file are read for header sniffing.
///
/// # Arguments
///
/// * `path` - The path to the image file.
/// * `ignore_format` - The image format to ignore.
///
/// # Returns
///
/// Returns `true` if the image format is supported and not ignored, `false` otherwise.
pub fn is_supported(path: &Path, ignore_format: &ImageFormat) -> bool {
    if let Some(extension) = path.extension()
        && extension == ignore_format.extension()
    {
        return false;
    }

    // sniffing the file header is sufficient to guess the format
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let mut header = [0u8; 512];
    let read = std::io::Read::read(&mut file, &mut header).unwrap_or(0);
    image::guess_format(&header[..read]).is_ok()
}

/// Removes files that match the given pattern.
///
/// # Arguments
///
/// * `pattern` - The glob pattern to match files.
///
/// # Returns
///
/// Returns `Ok(())` if the files are successfully removed, or an `Error` if an error occurs.
pub fn remove_files(pattern: &str) -> Result<(), Error> {
    let mut total_deleted_bytes: usize = 0;
    for entry in glob(pattern)? {
        let path = entry?;
        if path.is_file() {
            total_deleted_bytes += fs::metadata(&path)?.len() as usize;
            fs::remove_file(&path)?;
            println!("Deleted: {}", path.display());
        }
    }
    let format_option_binary_two_nospace = FormatSizeOptions::from(BINARY)
        .decimal_places(2)
        .decimal_zeroes(2)
        .space_after_value(false);
    println!(
        "Deleted {}.",
        format_size(total_deleted_bytes, format_option_binary_two_nospace)
    );

    Ok(())
}
