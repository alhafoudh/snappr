//! Fits one photo into the overlay's `#FF00FF` window, composites the overlay
//! over it, and writes `<basename>.overlay.png` beside the original.
//!
//! **The photo is never scaled.** The output canvas is sized so the window lands
//! at the photo's own resolution, which makes the fit a pure centre crop - and
//! keeps the photo bit-for-bit intact everywhere the overlay is clear. See
//! "Nothing is allowed to degrade the photo" in CLAUDE.md; most of the
//! non-obvious decisions in this file exist to protect that property.

use std::path::{Path, PathBuf};
use std::time::Instant;

use rs_vips::enums::{BandFormat, BlendMode, Extend};
use rs_vips::voption::{Setter, VOption};
use rs_vips::VipsImage;

use crate::config::{Orientation, PipelineConfig};
use crate::exif::{self, Exif};
use crate::overlay_svg::{self, Window};
use crate::problem::{name_of, warn_into, Problem, Warning};
use crate::vips;

/// The photo is laid down first and the overlay goes over it, but the overlay has
/// a hole where the window is, so a rounding disagreement between the two would
/// show the background through the join. Growing the photo past the window costs
/// a few pixels of resolution and cannot leave a gap.
///
/// Two pixels rather than one, for headroom: when the window's edge falls on a
/// fractional coordinate librsvg spreads the anti-aliased transition over as much
/// as three pixels. Measured leakage is zero at a margin of 1, 2 or 3, so this is
/// insurance rather than a fix, bought for 0.1% of the photo's width.
pub const SEAM_MARGIN: i32 = 2;

/// The window used when an orientation has no overlay: the whole frame, in the
/// same fractions-of-the-frame form [`overlay_svg::photo_window`] returns.
pub const FULL_FRAME: Window = Window { x: 0.0, y: 0.0, width: 1.0, height: 1.0 };

/// The canvas size, the crop taken from the photo, and where that crop is placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Layout {
    pub canvas_width: i32,
    pub canvas_height: i32,
    pub photo_left: i32,
    pub photo_top: i32,
    pub photo_width: i32,
    pub photo_height: i32,
    pub crop_left: i32,
    pub crop_top: i32,
}

/// Everything known about one composite.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Outcome {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub source_width: i32,
    pub source_height: i32,
    pub orientation: Orientation,
    /// The overlay that was composited, or `None` when the slot was empty or its
    /// file had gone.
    pub svg: Option<PathBuf>,
    pub layout: Layout,
    pub output_width: i32,
    pub output_height: i32,
    /// Follows the source and is never cast, so it is worth reporting: an 8-bit
    /// JPEG stays 8-bit and a 10-bit HEIC comes out as a 16-bit PNG.
    pub depth: u8,
    /// Pixels per mm, which is how libvips holds resolution.
    pub dpi: (f64, f64),
    pub icc: bool,
    pub bytes: u64,
    pub elapsed_secs: f64,
    pub warnings: Vec<Warning>,
    /// Camera, lens and exposure, for the UI to put under the photo. Absent
    /// entirely when the photo carries none - a screenshot or a scan should give
    /// the caller nothing rather than an empty object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exif: Option<Exif>,
}

pub struct Overlay {
    config: PipelineConfig,
}

impl Overlay {
    pub fn new(config: PipelineConfig) -> Self {
        Self { config }
    }

    /// Composites one photo and returns everything known about the operation.
    pub fn process(&self, source: &Path) -> Result<Outcome, Problem> {
        let source = absolute(source);
        if !source.is_file() {
            return Err(Problem::PhotoMissing {
                file: name_of(&source),
                path: source.clone(),
            });
        }

        // Each job starts with an empty error buffer, because libvips accumulates
        // and nothing clears it on read - so a warning from the last photo would
        // otherwise turn up in this one's console half.
        vips::clear_errors();

        let started = Instant::now();
        let name = name_of(&source);
        let mut warnings: Vec<Warning> = Vec::new();

        // **Read the dimensions after `autorot`, never before**, and read the
        // profile and the EXIF from the same rotated image. Anything measured
        // from the pre-rotation header is transposed on every portrait phone
        // photo, and that decides the orientation, and so which overlay is used
        // and which way the photo is cropped. libvips is lazy, so this is a
        // header read, not a decode.
        let image = VipsImage::new_from_file(&source)
            .and_then(|image| image.autorot())
            .map_err(vips::failed)?;

        let exif = exif::read(&image);
        let (width, height) = (image.get_width(), image.get_height());
        let orientation = Orientation::of(width, height);

        // `None` is a legitimate answer: the operator can clear either slot, and
        // a cleared slot prints the crop with nothing composited over it.
        let svg = self.overlay_for_orientation(orientation, &mut warnings);

        log::info!(
            "{name}: {width}x{height}, {} - {}",
            orientation.as_str(),
            match &svg {
                Some(path) => format!("using {}", name_of(path)),
                None => "no overlay, printing the crop".to_string(),
            }
        );

        if let Some(svg) = &svg {
            self.warn_on_aspect_mismatch(svg, orientation, &mut warnings);
        }

        let layout = self.layout_for(width, height, svg.as_deref(), orientation)?;
        log::info!(
            "{name}: cropping {}x{} at ({}, {}), placing on a {}x{} canvas",
            layout.photo_width,
            layout.photo_height,
            layout.crop_left,
            layout.crop_top,
            layout.canvas_width,
            layout.canvas_height
        );

        let overlay = match &svg {
            Some(svg) => Some(self.overlay_image(
                &image,
                svg,
                layout.canvas_width,
                layout.canvas_height,
                orientation,
                &mut warnings,
            )?),
            None => None,
        };

        let destination = self.destination_for(&source);
        let temp = self.temp_path_for(&source);
        log::info!("{name}: writing {}", name_of(&destination));

        let result = self.write(&image, &layout, overlay.as_ref(), &temp);
        match result {
            Ok(()) => {
                std::fs::rename(&temp, &destination).map_err(|error| Problem::PhotoUnusable {
                    detail: format!("could not rename into place: {error}"),
                })?;
            }
            Err(problem) => {
                let _ = std::fs::remove_file(&temp);
                return Err(problem);
            }
        }

        let elapsed_secs = started.elapsed().as_secs_f64();
        let bytes = std::fs::metadata(&destination).map(|meta| meta.len()).unwrap_or(0);
        log::info!(
            "{name}: wrote {} ({}, {:.1}s)",
            name_of(&destination),
            crate::human_size(bytes),
            elapsed_secs
        );

        Ok(Outcome {
            source,
            destination,
            source_width: width,
            source_height: height,
            orientation,
            svg,
            layout,
            output_width: layout.canvas_width,
            output_height: layout.canvas_height,
            depth: if image.get_format().map_err(vips::failed)? == BandFormat::Ushort {
                16
            } else {
                8
            },
            dpi: (image.get_xres(), image.get_yres()),
            icc: has_profile(&image),
            bytes,
            elapsed_secs,
            warnings,
            exif: if exif.is_empty() { None } else { Some(exif) },
        })
    }

    /// The path written.
    pub fn call(&self, source: &Path) -> Result<PathBuf, Problem> {
        Ok(self.process(source)?.destination)
    }

    /// `IMG_1234.heic` -> `IMG_1234.overlay.png`, beside the original.
    ///
    /// Only the last extension is stripped, so `a.b.c.jpg` becomes
    /// `a.b.c.overlay.png`.
    pub fn destination_for(&self, source: &Path) -> PathBuf {
        let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        source.with_file_name(format!("{stem}{}", self.config.output_suffix))
    }

    /// The in-progress name.
    ///
    /// Three things are load-bearing about its shape. It ends in a **real image
    /// extension**, because libvips picks its saver from the destination
    /// extension and a bare `.tmp` fails with "not a known file format". It
    /// carries the **output suffix**, so the watcher's self-output guard skips it.
    /// And it carries the **pid**, so two watchers cannot collide.
    pub fn temp_path_for(&self, source: &Path) -> PathBuf {
        let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        source.with_file_name(format!(
            "{stem}.tmp-{}{}",
            std::process::id(),
            self.config.output_suffix
        ))
    }

    /// Works out the canvas size, the crop taken from the photo, and where that
    /// crop is placed.
    ///
    /// The canvas is derived from the photo rather than fixed, so the window comes
    /// out at exactly the size of the largest crop the photo can supply at the
    /// window's shape. That makes the fit a pure centre crop - object-fit: cover
    /// with a scale factor of 1 - and keeps the photo bit-for-bit intact. The
    /// canvas therefore comes out *larger* than the photo whenever the window is
    /// smaller than the frame, which is the intended trade: bigger file,
    /// untouched pixels.
    ///
    /// A `None` svg means no overlay: the window is the whole frame and there is
    /// no seam to hide, so this comes out as a plain centre crop to the print
    /// ratio with the canvas equal to the crop. Nothing special-cases it further
    /// down.
    pub fn layout_for(
        &self,
        width: i32,
        height: i32,
        svg: Option<&Path>,
        orientation: Orientation,
    ) -> Result<Layout, Problem> {
        let window = match svg {
            Some(svg) => overlay_svg::photo_window(svg, Some(&orientation.slot()))?,
            None => FULL_FRAME,
        };
        let margin = if svg.is_some() { SEAM_MARGIN } else { 0 };

        // The window's shape in output pixels depends on the canvas shape, which
        // is the paper's ratio. The parenthesisation is deliberate: reordering
        // this into `w * ratio / h` differs in the last bit and can flip a
        // half-way rounding below.
        let window_ratio = (window.width / window.height) * self.config.print_ratio(orientation);
        let crop = crop_area(width, height, window_ratio);

        // Hold back the seam margin *before* deriving the canvas, so growing the
        // photo by it later can never ask for more pixels than the photo has.
        let canvas_width = ((crop.width - 2 * margin) as f64 / window.width).floor() as i32;
        let canvas_height = ((crop.height - 2 * margin) as f64 / window.height).floor() as i32;

        let left = (window.x * canvas_width as f64).round() as i32;
        let top = (window.y * canvas_height as f64).round() as i32;
        // Note this is `round(x) + round(w)`, not `round(x + w)`: the two differ
        // by a pixel, and the first is what the window's own edges are.
        let right = left + (window.width * canvas_width as f64).round() as i32;
        let bottom = top + (window.height * canvas_height as f64).round() as i32;

        // Grow by the seam margin, clamped to the canvas.
        let left = (left - margin).max(0);
        let top = (top - margin).max(0);
        let right = (right + margin).min(canvas_width);
        let bottom = (bottom + margin).min(canvas_height);
        let photo_width = right - left;
        let photo_height = bottom - top;

        // Always non-negative: the canvas was floored from the crop with the
        // margin already held back, so `photo_width <= crop.width <= width`. It
        // is asserted rather than assumed because the centring below divides by
        // two, and a negative there would be silently off by one.
        debug_assert!(width >= photo_width && height >= photo_height);

        Ok(Layout {
            canvas_width,
            canvas_height,
            photo_left: left,
            photo_top: top,
            photo_width,
            photo_height,
            crop_left: (width - photo_width) / 2,
            crop_top: (height - photo_height) / 2,
        })
    }

    fn overlay_for_orientation(
        &self,
        orientation: Orientation,
        warnings: &mut Vec<Warning>,
    ) -> Option<PathBuf> {
        let wanted = self.config.overlay(orientation)?;
        if wanted.is_file() {
            return Some(wanted.to_path_buf());
        }

        // A slot that is set but whose file has gone warns and degrades to no
        // overlay. Someone moves a branding folder mid-event; a kiosk that keeps
        // printing plain photos is recoverable, one that errors on every photo is
        // not. There is deliberately **no fall back to the other orientation's
        // overlay** - wrong branding on a print is worse than none.
        warn_into(
            Some(warnings),
            Problem::OverlayMissing {
                slot: orientation.slot(),
                file: name_of(wanted),
                path: wanted.to_path_buf(),
                orientation: orientation.as_str(),
            },
        );
        None
    }

    /// The overlay, rendered at the canvas size and converted into the photo's
    /// colour space.
    ///
    /// **librsvg renders in sRGB.** If the photo is in another space - iPhones
    /// shoot Display P3 - the overlay's colours have to be converted into it, or
    /// they are reinterpreted and come out wrong: an sRGB brand red of
    /// (200, 30, 40) lands as P3 (184, 50, 49). White and black are identical in
    /// both, so this is a no-op for today's achromatic branding and insurance for
    /// the moment someone adds a coloured logo. It must not be removed as dead
    /// code.
    fn overlay_image(
        &self,
        image: &VipsImage,
        svg: &Path,
        width: i32,
        height: i32,
        orientation: Orientation,
        warnings: &mut Vec<Warning>,
    ) -> Result<VipsImage, Problem> {
        let slot = orientation.slot();
        let overlay = overlay_svg::render(svg, width, height, Some(warnings), Some(&slot))?;

        if !has_profile(image) {
            return Ok(overlay);
        }
        let Ok(profile) = image.get_blob("icc-profile-data") else {
            vips::clear_errors();
            return Ok(overlay);
        };

        log::debug!("converting the overlay into the photo's colour space");

        // **The temp file must outlive the transform.** libvips resolves the
        // profile with lcms while *building* the operation, but keeping the file
        // alive until after the pixels are written costs nothing and does not
        // depend on that staying true. Ruby unlinked it at the end of a block and
        // got away with it by accident.
        let transformed = (|| {
            let mut file = tempfile::Builder::new()
                .prefix("snappr-profile")
                .suffix(".icc")
                .tempfile()
                .map_err(|error| Problem::ProfileUnusable { detail: error.to_string() })?;
            {
                use std::io::Write;
                file.write_all(&profile)
                    .and_then(|()| file.as_file().sync_all())
                    .map_err(|error| Problem::ProfileUnusable { detail: error.to_string() })?;
            }
            let path = file.path().to_string_lossy().into_owned();
            let out = overlay
                .icc_transform_with_opts(&path, VOption::new().set("input_profile", "srgb"))
                .map_err(|error| Problem::ProfileUnusable { detail: vips::first_line(error) })?;
            // Force the pixels while the profile is still on disk. libvips is
            // demand-driven, so without this the transform would be evaluated
            // later - during `pngsave`, by which time the temp file is gone.
            let out = out
                .copy_memory()
                .map_err(|error| Problem::ProfileUnusable { detail: vips::first_line(error) })?;
            drop(file);
            Ok(out)
        })();

        match transformed {
            Ok(image) => Ok(image),
            Err(problem) => {
                // A profile that cannot be used is a degradation, not a failure:
                // the branding may print slightly off, and that beats losing the
                // photo.
                warn_into(Some(warnings), problem);
                Ok(overlay)
            }
        }
    }

    /// The chain. Every step here is argued in CLAUDE.md and none of it is
    /// incidental.
    fn write(
        &self,
        image: &VipsImage,
        layout: &Layout,
        overlay: Option<&VipsImage>,
        temp: &Path,
    ) -> Result<(), Problem> {
        let background = self.background_for(image)?;

        // object-fit: cover, with nothing to scale. `extract_area`, never
        // anything thumbnail-based: **thumbnail downcasts 16-bit to 8-bit**, and
        // since the crop always fits inside the photo there is never anything to
        // scale anyway. Do not "tidy" this into a resize because it reads more
        // like object-fit: cover - it *is* object-fit: cover, and this is the
        // version that keeps the bit depth.
        let cropped = image
            .extract_area(layout.crop_left, layout.crop_top, layout.photo_width, layout.photo_height)
            .map_err(vips::failed)?;

        // Drop the photo onto the full canvas at the window's position. With no
        // overlay the canvas is the crop and the position is (0, 0), so this is a
        // no-op by value - left unconditional because one chain shape is easier
        // to follow than two.
        let placed = cropped
            .embed_with_opts(
                layout.photo_left,
                layout.photo_top,
                layout.canvas_width,
                layout.canvas_height,
                VOption::new()
                    .set("extend", Extend::Background as i32)
                    // Three components, matching the bands of the photo. libvips
                    // pads a short vector by repeating the last element, so a
                    // fourth entry would set alpha from the blue channel.
                    .set("background", &background[..]),
            )
            .map_err(vips::failed)?;

        let composited = match overlay {
            // The overlay is already exactly canvas-sized, so it goes down at
            // (0, 0). Ruby reached the same picture through `gravity` first,
            // which was a no-op by construction.
            Some(overlay) => {
                placed.composite2(overlay, BlendMode::Over).map_err(vips::failed)?
            }
            None => placed,
        };

        // The canvas is opaque, so this only drops a redundant alpha channel.
        // **Conditional because `flatten` is not a no-op on an image with no
        // alpha**: it fails with "linear: vector must have 1 or 2 elements",
        // which points nowhere near the cause. Alpha is here only if the overlay
        // brought it or the source carried it, so ask about exactly that.
        let flattened = if overlay.is_some() || image.hasalpha() {
            composited
                .flatten_with_opts(VOption::new().set("background", &background[..]))
                .map_err(vips::failed)?
        } else {
            composited
        };

        // PNG, not JPEG: exactly lossless, keeps 16 bits, keeps the ICC profile,
        // keeps the resolution tag. **`compression` and nothing else** - libvips
        // 8.15 swapped `strip` for `keep`, and setting either would drop the
        // profile and colour-shift the print, invisibly on screen.
        flattened
            .pngsave_with_opts(
                &temp.to_string_lossy(),
                VOption::new().set("compression", self.config.png_compression),
            )
            .map_err(vips::failed)
    }

    /// Whatever the overlay does not cover and the photo does not fill. White by
    /// default, which is what unprinted photo paper looks like anyway.
    fn background_for(&self, image: &VipsImage) -> Result<[f64; 3], Problem> {
        let scale = if image.get_format().map_err(vips::failed)? == BandFormat::Ushort {
            257.0
        } else {
            1.0
        };
        let rgb = self.config.background_rgb()?;
        Ok([rgb[0] * scale, rgb[1] * scale, rgb[2] * scale])
    }

    /// The overlay is stretched to fill the frame, so one drawn at the wrong shape
    /// is silently distorted. Say so rather than letting it look like a rendering
    /// bug.
    fn warn_on_aspect_mismatch(
        &self,
        svg: &Path,
        orientation: Orientation,
        warnings: &mut Vec<Warning>,
    ) {
        if let Some(problem) = aspect_mismatch(&self.config, svg, orientation) {
            warn_into(Some(warnings), problem);
        }
    }
}

/// Whether `svg` was drawn at a different shape from the paper `config` prints on,
/// as the problem to report, or `None` when it matches within 1%.
///
/// Free rather than a method so the settings window can ask it about a paper the
/// operator has picked but not saved yet - which is the moment the answer is
/// still cheap to act on.
pub fn aspect_mismatch(config: &PipelineConfig, svg: &Path, orientation: Orientation) -> Option<Problem> {
    let Ok(Some(drawn)) = overlay_svg::aspect_ratio(svg, Some(&orientation.slot())) else {
        return None;
    };
    let wanted = config.print_ratio(orientation);
    if (drawn - wanted).abs() <= wanted * 0.01 {
        return None;
    }

    // 20 px per mm, about 508 dpi - a size worth redrawing at, not the size
    // of any particular print.
    let (width, height) = match orientation {
        Orientation::Landscape => (config.print_long_mm, config.print_short_mm),
        Orientation::Portrait => (config.print_short_mm, config.print_long_mm),
    };

    Some(Problem::AspectMismatch {
        slot: orientation.slot(),
        file: name_of(svg),
        width: (width * 20.0).round() as i32,
        height: (height * 20.0).round() as i32,
        drawn,
        wanted,
        orientation: orientation.as_str(),
    })
}

/// A box of the target ratio inscribed in `width` x `height`, centred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crop {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

/// The largest box of the given ratio that fits inside `width` x `height`,
/// centred.
///
/// Taking the largest *inscribed* box is what keeps the fit a pure crop: exactly
/// one side is kept whole and the other is shortened, so the scale factor is 1
/// and libvips carries `xres`/`yres` through untouched. "Keep the dpi of the
/// source" is the requirement; this is how it is met.
///
/// **`round()`, never a cast.** `crop_area(1200, 900, 1.48)` gives a height of
/// 811; truncation would give 810, which moves the crop origin by a pixel and
/// changes every dimension derived from it.
pub fn crop_area(width: i32, height: i32, ratio: f64) -> Crop {
    let (crop_width, crop_height) = if (width as f64 / height as f64) > ratio {
        ((height as f64 * ratio).round() as i32, height)
    } else {
        (width, (width as f64 / ratio).round() as i32)
    };

    Crop {
        left: (width - crop_width) / 2,
        top: (height - crop_height) / 2,
        width: crop_width,
        height: crop_height,
    }
}

/// Just the dimensions of [`crop_area`].
pub fn crop_dimensions(width: i32, height: i32, ratio: f64) -> (i32, i32) {
    let crop = crop_area(width, height, ratio);
    (crop.width, crop.height)
}

fn has_profile(image: &VipsImage) -> bool {
    image.get_typeof("icc-profile-data").map(|kind| kind != 0).unwrap_or(false)
}

/// Absolute, without resolving symlinks.
///
/// `canonicalize` would resolve them, which changes the identity of a photo and
/// therefore its preview key - the stale-preview bug this pipeline has already
/// fixed once.
fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir().map(|dir| dir.join(path)).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> PipelineConfig {
        PipelineConfig::default()
    }

    #[test]
    fn the_destination_replaces_only_the_last_extension() {
        let overlay = Overlay::new(config());

        assert_eq!(
            overlay.destination_for(Path::new("/tmp/IMG_1234.heic")),
            PathBuf::from("/tmp/IMG_1234.overlay.png")
        );
        assert_eq!(
            overlay.destination_for(Path::new("/tmp/photo.JPEG")),
            PathBuf::from("/tmp/photo.overlay.png")
        );
        assert_eq!(
            overlay.destination_for(Path::new("/tmp/a.b.c.jpg")),
            PathBuf::from("/tmp/a.b.c.overlay.png")
        );
    }

    #[test]
    fn the_temp_path_is_a_png_and_carries_the_output_suffix() {
        let overlay = Overlay::new(config());
        let temp = overlay.temp_path_for(Path::new("/tmp/IMG_1234.heic"));
        let name = temp.file_name().unwrap().to_string_lossy();

        // `.png` so libvips picks a saver, the suffix so the watcher skips it,
        // the pid so two processes cannot collide.
        assert!(name.ends_with(".overlay.png"), "{name}");
        assert!(name.contains("tmp-"), "{name}");
        assert_ne!(temp, overlay.destination_for(Path::new("/tmp/IMG_1234.heic")));
    }

    #[test]
    fn crop_area_is_centred_and_hits_the_ratio() {
        // The worked case that catches a truncating port: 1200/1.48 is 810.81,
        // which rounds to 811 and truncates to 810.
        assert_eq!(
            crop_area(1200, 900, 1.48),
            Crop { left: 0, top: 44, width: 1200, height: 811 }
        );
        assert_eq!(
            crop_area(900, 1200, 100.0 / 148.0),
            Crop { left: 44, top: 0, width: 811, height: 1200 }
        );
    }

    #[test]
    fn crop_area_never_upscales_and_is_maximal() {
        let sizes = [(4032, 3024), (3024, 4032), (2000, 2000), (1200, 900), (6000, 4000)];
        let ratios = [1.48, 100.0 / 148.0, 1.0, 1.3333];

        for (width, height) in sizes {
            for ratio in ratios {
                let crop = crop_area(width, height, ratio);

                assert!(crop.width <= width, "{crop:?} for {width}x{height} @ {ratio}");
                assert!(crop.height <= height, "{crop:?} for {width}x{height} @ {ratio}");

                let got = crop.width as f64 / crop.height as f64;
                assert!((got - ratio).abs() < 0.005, "{crop:?} is not {ratio}");

                // Maximal: one side is always kept whole. This is the half that
                // stops someone "fixing" the crop into a fixed-dpi target.
                assert!(
                    crop.width == width || crop.height == height,
                    "{crop:?} for {width}x{height} @ {ratio} is not the largest inscribed box"
                );
            }
        }
    }

    #[test]
    fn with_no_overlay_the_canvas_is_the_crop_and_the_photo_fills_it() {
        let overlay = Overlay::new(config());
        let layout = overlay.layout_for(4032, 3024, None, Orientation::Landscape).unwrap();

        assert_eq!((layout.canvas_width, layout.canvas_height), (4032, 2724));
        assert_eq!(
            (layout.canvas_width, layout.canvas_height),
            crop_dimensions(4032, 3024, 1.48)
        );
        // No seam margin, so the photo fills the canvas exactly.
        assert_eq!((layout.photo_left, layout.photo_top), (0, 0));
        assert_eq!((layout.photo_width, layout.photo_height), (4032, 2724));
        assert_eq!(layout.crop_left, 0);
        assert_eq!(layout.crop_top, (3024 - 2724) / 2);
    }

    #[test]
    fn with_no_overlay_portrait_crops_the_other_way() {
        let overlay = Overlay::new(config());
        let layout = overlay.layout_for(3024, 4032, None, Orientation::Portrait).unwrap();

        assert_eq!((layout.canvas_width, layout.canvas_height), (2724, 4032));
    }
}
