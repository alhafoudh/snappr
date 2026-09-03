//! What libvips makes of a file.
//!
//! This exists for `bin/verify-bundle`, and the reason it is a real feature
//! rather than a test helper is worth stating: the gate has to check the bundle
//! with **the libvips the bundle actually ships**, not with whatever is installed
//! on the build machine. A binary inside `Snappr.app` reporting on a photo is the
//! only thing that can answer that, and it is a useful debugging command in its
//! own right.
//!
//! It reports the **loader** as well as the dimensions, because "the file reads"
//! and "the HEIC loader decoded it" are different claims - a libvips built
//! without HEVC support answers the first and fails the second.

use std::path::{Path, PathBuf};

use rs_vips::enums::BandFormat;
use rs_vips::VipsImage;

use crate::problem::Problem;
use crate::vips;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Report {
    pub path: PathBuf,
    pub width: i32,
    pub height: i32,
    pub bands: i32,
    pub format: String,
    pub depth: u8,
    pub xres: f64,
    pub yres: f64,
    pub has_alpha: bool,
    pub icc: bool,
    /// Which libvips loader read it - `heifload`, `jpegload`, `pngload`.
    pub loader: String,
    pub vips: String,
    /// The mean of a requested region. Proves the pixels really decoded rather
    /// than only the header having been read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean: Option<f64>,
}

/// Reads `path`'s header and reports it. `mean_of` is `left,top,width,height`.
pub fn describe(path: &Path, mean_of: Option<&str>) -> Result<Report, Problem> {
    let image = VipsImage::new_from_file(path)
        .and_then(|image| image.autorot())
        .map_err(vips::failed)?;

    let format = image.get_format().map_err(vips::failed)?;
    let mean = match mean_of {
        Some(region) => Some(mean_of_region(&image, region)?),
        None => None,
    };

    Ok(Report {
        path: path.to_path_buf(),
        width: image.get_width(),
        height: image.get_height(),
        bands: image.get_bands(),
        format: format!("{format:?}").to_lowercase(),
        depth: if format == BandFormat::Ushort { 16 } else { 8 },
        xres: image.get_xres(),
        yres: image.get_yres(),
        has_alpha: image.hasalpha(),
        icc: image.get_typeof("icc-profile-data").map(|kind| kind != 0).unwrap_or(false),
        loader: image.get_string("vips-loader").unwrap_or_default(),
        vips: rs_vips::Vips::version_string().unwrap_or_default(),
        mean,
    })
}

fn mean_of_region(image: &VipsImage, region: &str) -> Result<f64, Problem> {
    let numbers: Vec<i32> =
        region.split(',').filter_map(|part| part.trim().parse().ok()).collect();
    if numbers.len() != 4 {
        return Err(Problem::PhotoUnusable {
            detail: format!("a region wants four numbers, got {region:?}"),
        });
    }

    image
        .extract_area(numbers[0], numbers[1], numbers[2], numbers[3])
        .and_then(|patch| patch.avg())
        .map_err(vips::failed)
}
