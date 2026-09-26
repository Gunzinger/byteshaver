use std::path::Path;

/// `ImageFormat` defines the various formats that an image can have.
///
/// This enumeration covers a wide range of common and less common image formats.
/// Each variant represents a different format that an image file can be encoded in.
/// The `Unknown` variant is used for formats not explicitly listed here.
///
/// # Examples
///
/// ```
/// use byteshaver::format::ImageFormat;
///
/// let format = ImageFormat::Png;
/// let unknown_format = ImageFormat::Unknown;
/// assert_eq!(format.extension(), "png");
/// assert_eq!(ImageFormat::from_extension("custom-format"), unknown_format);
/// ```
#[derive(Debug, PartialEq)]
pub enum ImageFormat {
    /// AV1 Image File Format, a format designed for high compression efficiency.
    Avif,

    /// Bitmap, a raster graphics image file format used to store bitmap digital images.
    Bmp,

    /// DirectDraw Surface, a container format for storing data compressed with the S3TC algorithm.
    Dds,

    /// Farbfeld, a simple image file format designed to work well for lossless compression.
    Farbfeld,

    /// Graphics Interchange Format, a bitmap image format that supports animation.
    Gif,

    /// High Dynamic Range Image File Format, a raster graphics file format for high dynamic range images.
    Hdr,

    /// HEIF container family (HEIC/HEIF/HIF still images and AVIF stills),
    /// decoded via libheif when the `dec-heif` feature is enabled.
    ///
    /// Input-only: this crate never encodes into the HEIF container, so
    /// [`ImageFormat::extension`] reports `"heif"` and no encoder accepts it
    /// as a target format.
    Heif,

    /// Icon, a bitmap image format used for icons in Microsoft Windows.
    Ico,

    /// Joint Photographic Experts Group, an image compression standard that supports lossy and lossless compression.
    Jpeg,

    /// OpenEXR, a high dynamic range raster file format.
    Exr,

    /// Portable Network Graphics, a raster graphics file format that supports lossless data compression.
    Png,

    /// Portable anymap, a family of file formats to store bitmap images.
    Pnm,

    /// QuickTime Image, a raster graphics file format used by Apple's QuickTime framework.
    Qoi,

    /// Truevision TGA, a raster graphics file format used for storing images.
    Tga,

    /// Tagged Image File Format, a file format for storing raster graphics images.
    Tiff,

    /// WebP, an image format that provides lossless and lossy compression for images on the web.
    Webp,
    /// WebP, but encoded with the lossless VP8L encoder from image crate
    WebpImage,

    /// Represents an image format not explicitly listed here.
    Unknown,
}

impl ImageFormat {
    /// Get the file extension associated with the image format
    pub fn extension(&self) -> &str {
        match self {
            ImageFormat::Avif => "avif",
            ImageFormat::Bmp => "bmp",
            ImageFormat::Dds => "dds",
            ImageFormat::Farbfeld => "ff",
            ImageFormat::Gif => "gif",
            ImageFormat::Hdr => "hdr",
            ImageFormat::Heif => "heif",
            ImageFormat::Ico => "ico",
            ImageFormat::Jpeg => "jpeg",
            ImageFormat::Exr => "exr",
            ImageFormat::Png => "png",
            ImageFormat::Pnm => "pnm",
            ImageFormat::Qoi => "qoi",
            ImageFormat::Tga => "tga",
            ImageFormat::Tiff => "tiff",
            ImageFormat::Webp => "webp",
            ImageFormat::WebpImage => "webp",
            ImageFormat::Unknown => "?",
        }
    }

    /// Map a format of the `image` crate to the internal representation.
    pub fn from_image_format(format: image::ImageFormat) -> Self {
        match format {
            image::ImageFormat::Avif => ImageFormat::Avif,
            image::ImageFormat::Bmp => ImageFormat::Bmp,
            image::ImageFormat::Dds => ImageFormat::Dds,
            image::ImageFormat::Farbfeld => ImageFormat::Farbfeld,
            image::ImageFormat::Gif => ImageFormat::Gif,
            image::ImageFormat::Hdr => ImageFormat::Hdr,
            image::ImageFormat::Ico => ImageFormat::Ico,
            image::ImageFormat::Jpeg => ImageFormat::Jpeg,
            image::ImageFormat::OpenExr => ImageFormat::Exr,
            image::ImageFormat::Png => ImageFormat::Png,
            image::ImageFormat::Pnm => ImageFormat::Pnm,
            image::ImageFormat::Qoi => ImageFormat::Qoi,
            image::ImageFormat::Tga => ImageFormat::Tga,
            image::ImageFormat::Tiff => ImageFormat::Tiff,
            image::ImageFormat::WebP => ImageFormat::Webp,
            // the image crate may grow formats this crate does not know yet
            _ => ImageFormat::Unknown,
        }
    }

    /// Map to the corresponding format of the `image` crate.
    ///
    /// Returns `None` for pseudo formats that the `image` crate does not know.
    pub fn to_image_format(&self) -> Option<image::ImageFormat> {
        match self {
            ImageFormat::Avif => Some(image::ImageFormat::Avif),
            ImageFormat::Bmp => Some(image::ImageFormat::Bmp),
            ImageFormat::Dds => Some(image::ImageFormat::Dds),
            ImageFormat::Farbfeld => Some(image::ImageFormat::Farbfeld),
            ImageFormat::Gif => Some(image::ImageFormat::Gif),
            ImageFormat::Hdr => Some(image::ImageFormat::Hdr),
            // HEIF/HEIC/AVIF input is decoded by libheif (input-only format)
            ImageFormat::Heif => None,
            ImageFormat::Ico => Some(image::ImageFormat::Ico),
            ImageFormat::Jpeg => Some(image::ImageFormat::Jpeg),
            ImageFormat::Exr => Some(image::ImageFormat::OpenExr),
            ImageFormat::Png => Some(image::ImageFormat::Png),
            ImageFormat::Pnm => Some(image::ImageFormat::Pnm),
            ImageFormat::Qoi => Some(image::ImageFormat::Qoi),
            ImageFormat::Tga => Some(image::ImageFormat::Tga),
            ImageFormat::Tiff => Some(image::ImageFormat::Tiff),
            ImageFormat::Webp => Some(image::ImageFormat::WebP),
            ImageFormat::WebpImage | ImageFormat::Unknown => None,
        }
    }

    /// Determine the image format based on the file extension
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_ascii_lowercase().as_str() {
            // avif stills live in the HEIF container family (input decoding
            // via libheif); the `Avif` variant is output-only (ravif encoder)
            "heif" | "heic" | "hif" | "avif" => ImageFormat::Heif,
            "bmp" => ImageFormat::Bmp,
            "dds" => ImageFormat::Dds,
            "ff" | "farbfeld" => ImageFormat::Farbfeld,
            "gif" => ImageFormat::Gif,
            "hdr" => ImageFormat::Hdr,
            "ico" => ImageFormat::Ico,
            "jpeg" | "jpg" | "pjpeg" => ImageFormat::Jpeg,
            "exr" => ImageFormat::Exr,
            "png" | "x-png" => ImageFormat::Png,
            "pnm" => ImageFormat::Pnm,
            "qoi" => ImageFormat::Qoi,
            "tga" => ImageFormat::Tga,
            "tiff" | "tif" => ImageFormat::Tiff,
            "webp" => ImageFormat::Webp,
            _ => ImageFormat::Unknown,
        }
    }
}

impl From<&Path> for ImageFormat {
    fn from(path: &Path) -> Self {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some(ext) => ImageFormat::from_extension(ext),
            None => ImageFormat::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heif_extension_family_maps_to_heif() {
        assert_eq!(ImageFormat::from_extension("heif"), ImageFormat::Heif);
        assert_eq!(ImageFormat::from_extension("heic"), ImageFormat::Heif);
        assert_eq!(ImageFormat::from_extension("hif"), ImageFormat::Heif);
        // avif stills are decoded through the HEIF container path
        assert_eq!(ImageFormat::from_extension("avif"), ImageFormat::Heif);
        // case-insensitive matching
        assert_eq!(ImageFormat::from_extension("HEIC"), ImageFormat::Heif);
        assert_eq!(ImageFormat::from_extension("jxl"), ImageFormat::Unknown);
        assert_eq!(
            ImageFormat::from(Path::new("photos/img.heic")),
            ImageFormat::Heif
        );
        assert_eq!(
            ImageFormat::from(Path::new("photos/no_ext")),
            ImageFormat::Unknown
        );
    }

    #[test]
    fn heif_is_input_only() {
        // reports a container extension, never a target extension
        assert_eq!(ImageFormat::Heif.extension(), "heif");
        // the image crate cannot be asked to decode/encode HEIF containers
        assert_eq!(ImageFormat::Heif.to_image_format(), None);
    }

    #[test]
    fn avif_remains_the_output_format_of_the_ravif_encoder() {
        assert_eq!(ImageFormat::Avif.extension(), "avif");
        assert_eq!(
            ImageFormat::from_extension("png"),
            ImageFormat::Png,
            "non-heif extension mapping is untouched"
        );
    }
}
