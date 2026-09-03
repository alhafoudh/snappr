//! The overlay panel's card: the artwork, and where the photo window landed.
//!
//! **A check, not a decoration.** The card paints a grey rectangle with a photo
//! icon at the window this reports and draws the render over it, so the operator
//! sees where the photo will land before a print is made. That only means
//! anything because the render comes back through librsvg - the same renderer the
//! print uses. Handing the SVG to the webview instead would show what WebKit
//! makes of the file, complete with the magenta marker the pipeline strips, and
//! would prove nothing about what will actually be composited.
//!
//! Two differences from the pipeline's own use of an overlay, both deliberate:
//!
//! - **A missing window is a warning here and an error there.** An overlay with
//!   no marker cannot be printed, but it can and must still be *shown*: this is
//!   the screen where the operator finds out, so a card that failed to draw would
//!   hide the very thing it exists to reveal.
//! - **The output is PNG**, because JPEG has no alpha and would flatten the photo
//!   window to black. The panel paints the background behind it.

use std::path::{Path, PathBuf};

use rs_vips::VipsImage;

use crate::config::{Orientation, PipelineConfig};
use crate::overlay_svg::{self, Window};
use crate::problem::{name_of, warn_into, Problem, Warning};
use crate::vips;

/// Twice the panel's 220 px card width, for retina.
pub const BOX_PX: i32 = 440;

/// What one overlay looks like, and what it would do to a photo.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Card {
    pub thumb: PathBuf,
    pub width: i32,
    pub height: i32,
    /// The shape the overlay was *drawn* at, not the paper's. The card is sized
    /// to it, so an overlay drawn at the wrong ratio looks wrong on its card -
    /// a second check for nothing.
    pub aspect: f64,
    /// `None` when the overlay has no marker, which is a warning rather than a
    /// failure.
    pub window: Option<Window>,
    pub warnings: Vec<Warning>,
}

/// Renders one overlay's card into `dir` as `<key>.png`.
pub fn render(
    config: &PipelineConfig,
    svg: &Path,
    dir: &Path,
    key: &str,
) -> Result<Card, Problem> {
    let mut warnings = Vec::new();

    let aspect = ratio_of(config, svg, &mut warnings);
    let (width, height) = dimensions(aspect, BOX_PX);

    // `slot: None`, so the messages read "the overlay ..." - the panel belongs to
    // no orientation, and a card is shown for overlays that are not live at all.
    let image = overlay_svg::render(svg, width, height, Some(&mut warnings), None)?;
    let thumb = write(&image, dir, key)?;

    Ok(Card { thumb, width, height, aspect, window: window_of(svg, &mut warnings), warnings })
}

/// The overlay's own shape, falling back to the paper's when it does not say.
fn ratio_of(config: &PipelineConfig, svg: &Path, warnings: &mut Vec<Warning>) -> f64 {
    let drawn = overlay_svg::aspect_ratio(svg, None).ok().flatten();

    match drawn {
        Some(aspect) if aspect.is_finite() && aspect > 0.0 => aspect,
        _ => {
            warn_into(
                Some(warnings),
                Problem::OverlayNoShape { file: name_of(svg), path: svg.to_path_buf() },
            );
            config.print_ratio(Orientation::Landscape)
        }
    }
}

/// Fit inside a box, rather than scaling by width.
///
/// A tall portrait overlay would otherwise ask for a render several thousand
/// pixels high to fill a 440 px-wide card.
fn dimensions(aspect: f64, box_px: i32) -> (i32, i32) {
    if aspect >= 1.0 {
        (box_px, ((box_px as f64 / aspect).round() as i32).max(1))
    } else {
        (((box_px as f64 * aspect).round() as i32).max(1), box_px)
    }
}

/// Where the photo would land, or `None` with a warning when the overlay has no
/// marker.
fn window_of(svg: &Path, warnings: &mut Vec<Warning>) -> Option<Window> {
    match overlay_svg::photo_window(svg, None) {
        Ok(window) => Some(window),
        Err(problem) => {
            warn_into(Some(warnings), problem);
            None
        }
    }
}

fn write(image: &VipsImage, dir: &Path, key: &str) -> Result<PathBuf, Problem> {
    std::fs::create_dir_all(dir).map_err(|error| Problem::CannotDrawOverlays {
        detail: format!("could not create {}: {error}", dir.display()),
    })?;

    let target = dir.join(format!("{key}.png"));
    let temp = dir.join(format!("{key}.tmp-{}.png", std::process::id()));

    // No `compression` option, unlike the print: a 440 px card is not worth
    // tuning, and the default is what the Ruby wrote too.
    image.pngsave(&temp.to_string_lossy()).map_err(vips::failed)?;

    match std::fs::rename(&temp, &target) {
        Ok(()) => Ok(target),
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(Problem::CannotDrawOverlays {
                detail: format!("could not rename {}: {error}", temp.display()),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures").join(name)
    }

    fn dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("snappr-cards-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_landscape_overlay_fills_the_box_across() {
        assert_eq!(dimensions(2960.0 / 2000.0, 440), (440, 297));
    }

    #[test]
    fn a_portrait_overlay_fills_the_box_down() {
        assert_eq!(dimensions(2000.0 / 2960.0, 440), (297, 440));
    }

    #[test]
    fn an_extreme_ratio_never_asks_for_a_zero_pixel_render() {
        assert_eq!(dimensions(0.0001, 440), (1, 440));
        assert_eq!(dimensions(10000.0, 440), (440, 1));
    }

    #[test]
    fn a_card_carries_the_artwork_and_the_window() {
        vips::init().unwrap();
        let config = PipelineConfig::default();

        let card = render(&config, &fixture("window-landscape.svg"), &dir(), "landscape").unwrap();

        assert!(card.thumb.is_file());
        assert_eq!(card.width, BOX_PX);
        assert!(card.height <= BOX_PX);
        assert!((card.aspect - 2960.0 / 2000.0).abs() < 0.001);
        assert!(card.warnings.is_empty(), "{:?}", card.warnings);

        let window = card.window.unwrap();
        assert!((window.x - 80.0 / 2960.0).abs() < 0.005, "{window:?}");
        assert!((window.width - 1616.0 / 2960.0).abs() < 0.005, "{window:?}");
    }

    /// The case the panel exists for: the operator finds out here rather than on
    /// a failed print, so the card must still draw.
    #[test]
    fn an_overlay_with_no_marker_still_draws_and_says_why() {
        vips::init().unwrap();
        let config = PipelineConfig::default();

        let card = render(&config, &fixture("absolute.svg"), &dir(), "no-marker").unwrap();

        assert!(card.thumb.is_file(), "the card still rendered");
        assert!(card.window.is_none());
        assert_eq!(
            card.warnings.iter().map(|w| w.kind.as_str()).collect::<Vec<_>>(),
            ["overlay_window_missing"]
        );
        // And the sentence is one an operator can act on.
        assert!(!card.warnings[0].message.contains("#FF00FF"));
        assert!(card.warnings[0].message.to_lowercase().contains("magenta"));
    }

    #[test]
    fn a_marker_in_a_style_attribute_needs_no_safety_net() {
        vips::init().unwrap();
        let config = PipelineConfig::default();

        let card = render(&config, &fixture("window-styled.svg"), &dir(), "styled").unwrap();

        // Found by the element walk, so `marker_survived` never fires.
        assert!(card.window.is_some());
        assert!(
            card.warnings.iter().all(|w| w.kind != "marker_survived"),
            "{:?}",
            card.warnings
        );
    }

    #[test]
    fn the_card_has_a_transparent_window() {
        vips::init().unwrap();
        let config = PipelineConfig::default();

        let card = render(&config, &fixture("window-landscape.svg"), &dir(), "hole").unwrap();
        let image = VipsImage::new_from_file(&card.thumb).unwrap();

        assert!(image.hasalpha(), "PNG, so the window can be a hole");
        assert!(image.extract_band(3).unwrap().avg().unwrap() < 200.0, "and it is one");
    }

    #[test]
    fn it_leaves_no_temp_file_behind() {
        vips::init().unwrap();
        let config = PipelineConfig::default();
        let dir = dir();

        render(&config, &fixture("window-landscape.svg"), &dir, "tidy").unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains("tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
}
