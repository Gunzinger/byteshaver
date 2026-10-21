use crate::{Error, format::ImageFormat};
use glob::glob;
use humansize::{BINARY, FormatSizeOptions, format_size};
use std::path::PathBuf;
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
/// Aborts on the first error, reporting the partial progress of the run
/// (M-8).
///
/// # Arguments
///
/// * `pattern` - The glob pattern to match files.
///
/// # Returns
///
/// Returns `Ok(())` if the files are successfully removed, or an `Error` if an error occurs.
pub fn remove_files(pattern: &str) -> Result<(), Error> {
    let format_option_binary_two_nospace = FormatSizeOptions::from(BINARY)
        .decimal_places(2)
        .decimal_zeroes(2)
        .space_after_value(false);

    let matched: Vec<PathBuf> = glob(pattern)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|path| path.is_file())
        .collect();
    let mut deleted_files: u64 = 0;
    let mut total_deleted_bytes: u64 = 0;
    #[allow(clippy::explicit_counter_loop)]
    for path in &matched {
        let file_bytes = match fs::metadata(path).map(|meta| meta.len()) {
            Ok(file_bytes) => file_bytes,
            Err(err) => {
                return Err(clean_aborted(
                    deleted_files,
                    matched.len() as u64,
                    total_deleted_bytes,
                    &err,
                    format_option_binary_two_nospace,
                ));
            }
        };
        if let Err(err) = fs::remove_file(path) {
            return Err(clean_aborted(
                deleted_files,
                matched.len() as u64,
                total_deleted_bytes,
                &err,
                format_option_binary_two_nospace,
            ));
        }
        total_deleted_bytes += file_bytes;
        deleted_files += 1;
        println!("Deleted: {}", path.display());
    }
    println!(
        "Deleted {}.",
        format_size(total_deleted_bytes, format_option_binary_two_nospace)
    );

    Ok(())
}

/// Reports the partial state of an aborted [`remove_files`] run: how many
/// of the matched files were already deleted and how many bytes went
/// with them.
fn clean_aborted(
    deleted_files: u64,
    matched_files: u64,
    deleted_bytes: u64,
    err: &impl std::error::Error,
    format: FormatSizeOptions,
) -> Error {
    Error::from_string(format!(
        "deleted {} of {} matched files, {}; aborted: {err}",
        deleted_files,
        matched_files,
        format_size(deleted_bytes, format),
    ))
}
