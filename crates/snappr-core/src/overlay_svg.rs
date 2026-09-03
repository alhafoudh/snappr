//! Renders an overlay SVG at an exact pixel size, and locates the photo window
//! inside it.
//!
//! Two rules govern this file:
//!
//! 1. **The authored view box is preserved.** It is what maps the artwork's own
//!    coordinate system onto the output, so setting width and height is enough to
//!    scale a whole design to any photo size. Overwriting it strands the artwork
//!    at its authored coordinates - which was got wrong once, and reads as "the
//!    overlay is broken" rather than as a bug here. This is why overlays can be
//!    drawn in any tool.
//!
//! 2. **A `#FF00FF` shape marks where the photo goes**, and is never rendered. It
//!    is a guide for the designer, not part of the artwork.
//!
//! **The document is rewritten by streaming, not through a DOM**, and that is a
//! decision rather than a convenience. The result goes straight to librsvg, so it
//! has to come back out as it went in: namespaces, `<defs>`, `clip-path`, an
//! embedded `<style>`, CDATA, comments, the declaration. Every DOM crate
//! normalises some of that - attribute order into a hash map, comments dropped,
//! `xmlns` rewritten. `quick-xml` hands back the raw bytes of every event this
//! code did not ask to change, so only the four root attributes and the deleted
//! marker elements differ from the file on disk.

use std::path::{Path, PathBuf};

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer, XmlVersion};
use regex::Regex;
use rs_vips::enums::{BandFormat, OperationBoolean, OperationRelational};
use rs_vips::voption::{Setter, VOption};
use rs_vips::VipsImage;
use std::sync::OnceLock;

use crate::problem::{name_of, warn_into, Problem, Warning};
use crate::vips;

/// Magenta, because nothing in real branding is this colour by accident.
pub const MARKER: [f64; 3] = [255.0, 0.0, 255.0];
/// Generous, to catch a marker that has been through a colour-managed export.
pub const MARKER_TOLERANCE: f64 = 60.0;

/// The four spellings of magenta an exporter might write. Anchored at both ends,
/// like the Ruby it replaces, so `fill="#ff00ff00"` is not a marker.
fn marker_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?i)^\s*(#ff00ff|#f0f|magenta|rgb\(\s*255\s*,\s*0\s*,\s*255\s*\))\s*$")
            .expect("a literal pattern")
    })
}

/// `fill:` inside a `style` attribute. No word boundary, matching Ruby - which
/// means it also matches inside `stroke-fill:`; tightening that would be a
/// behaviour change, not a tidy-up.
fn style_fill_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"(?i)fill\s*:\s*([^;]+)").expect("a literal pattern"))
}

/// Where the photo goes inside an overlay, as fractions of the frame in 0..1.
///
/// Fractions rather than pixels because the canvas size is derived from them
/// later, at whatever size the photo turns out to be.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Window {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The arguments every overlay message takes.
///
/// `slot` is "landscape overlay" when the caller said which setting this file
/// came from, and plain "overlay" when it did not, so the sentences read the same
/// either way. `file` is the basename the operator sees in the panel; `path` is
/// the full one, which only the console gets.
struct About {
    slot: String,
    file: String,
    path: PathBuf,
}

fn about(svg_path: &Path, slot: Option<&str>) -> About {
    About {
        slot: slot.unwrap_or("overlay").to_string(),
        file: name_of(svg_path),
        path: svg_path.to_path_buf(),
    }
}

/// Returns an RGBA image of exactly `width` x `height`, with the photo window
/// left transparent.
///
/// `warnings` collects anything worth showing an operator as well as logging.
pub fn render(
    svg_path: &Path,
    width: i32,
    height: i32,
    warnings: Option<&mut Vec<Warning>>,
    slot: Option<&str>,
) -> Result<VipsImage, Problem> {
    let source = stripped_source(svg_path, width, height, slot)?;
    let image = load_svg(&source)?;
    let image = if image.hasalpha() { image } else { image.addalpha().map_err(vips::failed)? };
    key_out_marker(image, svg_path, warnings, slot)
}

/// Where the photo goes, as fractions of the frame.
///
/// Found by rendering and matching the colour, not by reading the shape's
/// geometry. Exporters wrap shapes in groups, transforms and clip paths, and
/// express rectangles as `<path>` data; a rendered pixel is the one thing that
/// accounts for all of it. The detection render is at the authored size, so it
/// costs nothing.
pub fn photo_window(svg_path: &Path, slot: Option<&str>) -> Result<Window, Problem> {
    let reference = view_box_numbers(svg_path, slot)?;
    let width = reference[2].round() as i32;
    let height = reference[3].round() as i32;

    // The marker is *kept* for this render - that is the whole difference from
    // `render`, and getting the two the wrong way round either breaks detection
    // or puts a magenta block on the print.
    let source = source_for(svg_path, width, height, slot)?;
    let image = load_svg(&source)?;

    let Some(box_) = marker_bounds(&image)? else {
        let About { slot, file, path } = about(svg_path, slot);
        return Err(Problem::OverlayWindowMissing { slot, file, path });
    };

    Ok(Window {
        x: box_.0 as f64 / width as f64,
        y: box_.1 as f64 / height as f64,
        width: box_.2 as f64 / width as f64,
        height: box_.3 as f64 / height as f64,
    })
}

/// The aspect ratio the overlay was drawn at, or `None` when its height is zero.
pub fn aspect_ratio(svg_path: &Path, slot: Option<&str>) -> Result<Option<f64>, Problem> {
    let numbers = view_box_numbers(svg_path, slot)?;
    let (width, height) = (numbers[2], numbers[3]);
    Ok(if height == 0.0 { None } else { Some(width / height) })
}

/// The document resized but otherwise untouched - marker included.
pub fn source_for(
    svg_path: &Path,
    width: i32,
    height: i32,
    slot: Option<&str>,
) -> Result<Vec<u8>, Problem> {
    rewrite(svg_path, width, height, slot, false)
}

/// The document resized with every marker element removed.
pub fn stripped_source(
    svg_path: &Path,
    width: i32,
    height: i32,
    slot: Option<&str>,
) -> Result<Vec<u8>, Problem> {
    rewrite(svg_path, width, height, slot, true)
}

fn load_svg(source: &[u8]) -> Result<VipsImage, Problem> {
    // Format-sniffed rather than calling `svgload_buffer` directly, so a gzipped
    // SVGZ export still works. This is the call that fails with "buffer is not in
    // a known format" if the untrusted-loader exemption in `vips::init` is gone.
    VipsImage::new_from_buffer(source, "").map_err(vips::failed)
}

// --- The document rewrite ---------------------------------------------------

/// Reads the file and writes it back with the four root attributes set, and
/// optionally with every marker element removed.
///
/// One function for both because they differ by exactly that flag, and having the
/// two variants share a rewriter is what stops them drifting apart in *any* other
/// respect - which is the failure mode that matters, since one is what gets
/// composited and the other is what the window is measured from.
fn rewrite(
    svg_path: &Path,
    width: i32,
    height: i32,
    slot: Option<&str>,
    strip_marker: bool,
) -> Result<Vec<u8>, Problem> {
    let source = read_source(svg_path, slot)?;
    let view_box = view_box_of(&source, svg_path, slot)?;

    let mut reader = Reader::from_str(&source);
    // A design tool's export is not a document to validate; it is one to pass
    // through. An unclosed tag librsvg is happy with must not become a failed
    // print.
    reader.config_mut().check_end_names = false;
    let mut writer = Writer::new(Vec::with_capacity(source.len() + 128));

    let mut depth: usize = 0;
    let mut seen_root = false;
    // The depth of the marker subtree currently being swallowed, if any.
    let mut skipping: Option<usize> = None;

    loop {
        let event = reader.read_event().map_err(|error| {
            let problem = Problem::OverlayUnreadable {
                slot: slot.unwrap_or("overlay").to_string(),
                file: name_of(svg_path),
                path: svg_path.to_path_buf(),
            };
            log::debug!("{}: {error}", svg_path.display());
            problem
        })?;

        match event {
            Event::Start(element) => {
                depth += 1;
                if skipping.is_some() {
                    continue;
                }
                if !seen_root {
                    seen_root = true;
                    // The root is never tested for the marker: several real
                    // overlays carry `fill="none"` on it, and a root that happened
                    // to be magenta would otherwise delete the whole document.
                    let root = with_root_attributes(&element, width, height, &view_box);
                    writer.write_event(Event::Start(root)).expect("writing to a Vec");
                } else if strip_marker && is_marker(&element) {
                    skipping = Some(depth);
                } else {
                    writer.write_event(Event::Start(element)).expect("writing to a Vec");
                }
            }
            Event::End(element) => {
                if skipping == Some(depth) {
                    // The marker's own closing tag: the subtree ends here.
                    skipping = None;
                    depth -= 1;
                    continue;
                }
                let inside = skipping.is_some();
                depth -= 1;
                if !inside {
                    writer.write_event(Event::End(element)).expect("writing to a Vec");
                }
            }
            // Self-closing, so there is no subtree and no depth to track. This is
            // the shape every committed overlay's marker actually has.
            Event::Empty(element) => {
                if skipping.is_some() {
                    continue;
                }
                if !seen_root {
                    seen_root = true;
                    let root = with_root_attributes(&element, width, height, &view_box);
                    writer.write_event(Event::Empty(root)).expect("writing to a Vec");
                } else if strip_marker && is_marker(&element) {
                    // Dropped outright.
                } else {
                    writer.write_event(Event::Empty(element)).expect("writing to a Vec");
                }
            }
            Event::Eof => break,
            // Text, CDATA, comments, processing instructions, the declaration and
            // the doctype all pass through byte for byte.
            other => {
                if skipping.is_none() {
                    writer.write_event(other).expect("writing to a Vec");
                }
            }
        }
    }

    Ok(writer.into_inner())
}

/// The root with `width`, `height`, `viewBox` and `preserveAspectRatio` set, and
/// every other attribute left exactly as authored.
///
/// Existing attributes are replaced **in place**, so attribute order survives;
/// absent ones are appended. `preserveAspectRatio` is overwritten rather than
/// merely added, which matters: an Illustrator export carrying
/// `xMidYMid meet` would otherwise letterbox the rounding residue and leave a
/// background-coloured line down one edge of every print.
fn with_root_attributes(
    element: &BytesStart<'_>,
    width: i32,
    height: i32,
    view_box: &str,
) -> BytesStart<'static> {
    let mut root = BytesStart::new(element.name().as_ref().to_string());

    let width = width.to_string();
    let height = height.to_string();
    let mut written = [false; 4];

    let mut attributes = element.attributes();
    attributes.with_checks(false);
    for attribute in attributes.flatten() {
        match attribute.key.as_ref() {
            "width" => {
                root.push_attribute(("width", width.as_str()));
                written[0] = true;
            }
            "height" => {
                root.push_attribute(("height", height.as_str()));
                written[1] = true;
            }
            "viewBox" => {
                root.push_attribute(("viewBox", view_box));
                written[2] = true;
            }
            "preserveAspectRatio" => {
                root.push_attribute(("preserveAspectRatio", "none"));
                written[3] = true;
            }
            _ => root.push_attribute(attribute),
        }
    }

    if !written[0] {
        root.push_attribute(("width", width.as_str()));
    }
    if !written[1] {
        root.push_attribute(("height", height.as_str()));
    }
    if !written[2] {
        root.push_attribute(("viewBox", view_box));
    }
    if !written[3] {
        // Fill the frame exactly. The crop already gives the photo the print's
        // ratio, so the only stretch is rounding residue; the default "meet"
        // would letterbox that residue and show the photo through what should be
        // a solid panel.
        root.push_attribute(("preserveAspectRatio", "none"));
    }

    root.to_owned()
}

/// Whether this element is the photo-window marker.
///
/// Exactly two attributes are inspected, on the element itself: no inheritance,
/// no CSS cascade, no `<use>` resolution. Anything the walk cannot see is caught
/// afterwards by [`key_out_marker`], which warns.
fn is_marker(element: &BytesStart<'_>) -> bool {
    let mut fill = None;
    let mut style = None;

    let mut attributes = element.attributes();
    attributes.with_checks(false);
    for attribute in attributes.flatten() {
        // Unescaped, because that is what a DOM hands back and `fill="&#35;f0f"`
        // is still a marker.
        match attribute.key.as_ref() {
            "fill" => fill = attribute.normalized_value(XmlVersion::default()).ok().map(|v| v.into_owned()),
            "style" => style = attribute.normalized_value(XmlVersion::default()).ok().map(|v| v.into_owned()),
            _ => {}
        }
    }

    if let Some(fill) = fill {
        if marker_pattern().is_match(&fill) {
            return true;
        }
    }
    if let Some(style) = style {
        if let Some(captures) = style_fill_pattern().captures(&style) {
            return marker_pattern().is_match(&captures[1]);
        }
    }
    false
}

// --- The view box ------------------------------------------------------------

/// The file as text. Lossy, like Ruby's `File.read` with a UTF-8 external
/// encoding: an exporter that wrote a stray byte should not cost a print.
fn read_source(svg_path: &Path, slot: Option<&str>) -> Result<String, Problem> {
    let bytes = std::fs::read(svg_path).map_err(|_| {
        let About { slot, file, path } = about(svg_path, slot);
        Problem::OverlayFileMissing { slot, file, path }
    })?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The authored view box, or one synthesised from `width` and `height`.
///
/// A file with neither cannot be scaled at all, which is the one thing this
/// module needs, so that is an error rather than a guess.
fn view_box_of(source: &str, svg_path: &Path, slot: Option<&str>) -> Result<String, Problem> {
    let root = root_of(source, svg_path, slot)?;

    let mut authored = None;
    let mut width = None;
    let mut height = None;
    let mut attributes = root.attributes();
    attributes.with_checks(false);
    for attribute in attributes.flatten() {
        let value = attribute.normalized_value(XmlVersion::default()).ok().map(|v| v.into_owned());
        match attribute.key.as_ref() {
            "viewBox" => authored = value,
            "width" => width = value,
            "height" => height = value,
            _ => {}
        }
    }

    if let Some(authored) = authored.filter(|value| !value.trim().is_empty()) {
        return Ok(authored);
    }

    let width = width.as_deref().and_then(length);
    let height = height.as_deref().and_then(length);
    match (width, height) {
        // Formatted the way Ruby's Float interpolation did - "0 0 1000 1480"
        // would be equally correct, but the trailing `.0` is what the fixtures
        // and one test are written against, and it costs nothing to keep.
        (Some(width), Some(height)) => Ok(format!("0 0 {width:?} {height:?}")),
        _ => {
            let About { slot, file, path } = about(svg_path, slot);
            Err(Problem::OverlayNoSize { slot, file, path })
        }
    }
}

/// The view box as four numbers.
fn view_box_numbers(svg_path: &Path, slot: Option<&str>) -> Result<[f64; 4], Problem> {
    let source = read_source(svg_path, slot)?;
    let view_box = view_box_of(&source, svg_path, slot)?;

    // Trimmed before splitting, unlike the Ruby, which split the raw attribute on
    // `/[\s,]+/` and so read a *leading* space as an empty first field - shifting
    // every index and silently measuring a zero-width window. A leading space is
    // legal SVG, so this is a fix rather than a divergence, and the test below
    // pins it.
    let mut numbers = [0.0; 4];
    let fields = view_box
        .trim()
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|field| !field.is_empty());

    for (slot_, field) in numbers.iter_mut().zip(fields) {
        // Lenient like Ruby's `to_f`: a leading numeric run, and 0.0 for anything
        // that does not start with one.
        *slot_ = length(field).unwrap_or(0.0);
    }
    Ok(numbers)
}

fn root_of(
    source: &str,
    svg_path: &Path,
    slot: Option<&str>,
) -> Result<BytesStart<'static>, Problem> {
    let mut reader = Reader::from_str(source);
    reader.config_mut().check_end_names = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                return Ok(element.into_owned())
            }
            Ok(Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
    }

    let About { slot, file, path } = about(svg_path, slot);
    Err(Problem::OverlayUnreadable { slot, file, path })
}

/// `"2000"`, `"2000px"`, `"2000.5pt"` -> 2000.0. Percentages are meaningless
/// without a parent viewport, so they are refused outright rather than guessed at.
///
/// Deliberately does not accept a leading sign or exponent notation: neither
/// appears in a real export, and accepting them would be inventing behaviour the
/// Ruby never had.
fn length(value: &str) -> Option<f64> {
    let value = value.trim();
    if value.is_empty() || value.ends_with('%') {
        return None;
    }
    let numeric: String =
        value.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    numeric.parse().ok()
}

// --- The marker, in rendered pixels -----------------------------------------

/// Safety net for a marker whose colour arrives through something the element
/// walk cannot see - a CSS class, a gradient, a `<use>`.
///
/// Better a transparent hole than a magenta block on the print, and it warns
/// either way, because a design that reaches this has something to fix.
fn key_out_marker(
    image: VipsImage,
    svg_path: &Path,
    warnings: Option<&mut Vec<Warning>>,
    slot: Option<&str>,
) -> Result<VipsImage, Problem> {
    let mask = marker_mask(&image)?;
    if mask.avg().map_err(vips::failed)? == 0.0 {
        return Ok(image);
    }

    let About { slot, file, path } = about(svg_path, slot);
    warn_into(warnings, Problem::MarkerSurvived { slot, file, path });

    // 255 everywhere the mask is *not* set. Ruby expressed this as
    // `relational_const(:more, 128).ifthenelse(0, 255)`; the inversion is the
    // whole point of the step, and `lesseq` is the same thing in one call. Get
    // the sense of it backwards and the alpha is ANDed with the complement - the
    // entire overlay goes transparent and the branding vanishes from every print,
    // while this function still reports success.
    let keep = mask
        .relational_const(OperationRelational::Lesseq, &[128.0])
        .map_err(vips::failed)?;

    // Bitwise AND on uchar, which is why 255 is the "keep" value: 0xff & a == a.
    let alpha = image
        .extract_band(3)
        .map_err(vips::failed)?
        .boolean(&keep, OperationBoolean::And)
        .map_err(vips::failed)?;

    let rgb = image
        .extract_band_with_opts(0, VOption::new().set("n", 3))
        .map_err(vips::failed)?;

    VipsImage::bandjoin(&[rgb, alpha]).map_err(vips::failed)
}

/// The bounding box of the marker, in pixels, or `None` when there is none.
fn marker_bounds(image: &VipsImage) -> Result<Option<(i32, i32, i32, i32)>, Problem> {
    let mask = marker_mask(image)?;
    if mask.avg().map_err(vips::failed)? == 0.0 {
        return Ok(None);
    }

    // **Both options are required.** libvips defaults to `background: 255,
    // threshold: 10`, which on a 0/255 mask trims against white and returns the
    // bounding box of the *complement* of the marker: a plausible-looking window,
    // a wrong canvas, and the photo placed in the wrong part of the print, with
    // no error anywhere.
    let (left, top, width, height) = mask
        .find_trim_with_opts(
            VOption::new().set("background", &[0.0][..]).set("threshold", 128.0),
        )
        .map_err(vips::failed)?;

    if width == 0 || height == 0 {
        return Ok(None);
    }
    Ok(Some((left, top, width, height)))
}

/// 255 where the pixel is marker-coloured and opaque, 0 elsewhere.
fn marker_mask(image: &VipsImage) -> Result<VipsImage, Problem> {
    let scale = if image.get_format().map_err(vips::failed)? == BandFormat::Ushort {
        257.0
    } else {
        1.0
    };

    let near = |band: i32, target: f64| -> Result<VipsImage, Problem> {
        let channel = image.extract_band(band).map_err(vips::failed)?;
        // Inclusive bounds: the marker's own channels are exactly 0 and 255,
        // which exclusive comparisons would reject outright.
        let low = (target - MARKER_TOLERANCE).clamp(0.0, 255.0) * scale;
        let high = (target + MARKER_TOLERANCE).clamp(0.0, 255.0) * scale;
        let above = channel
            .relational_const(OperationRelational::Moreeq, &[low])
            .map_err(vips::failed)?;
        let below = channel
            .relational_const(OperationRelational::Lesseq, &[high])
            .map_err(vips::failed)?;
        above.boolean(&below, OperationBoolean::And).map_err(vips::failed)
    };

    let mut mask = near(0, MARKER[0])?
        .boolean(&near(1, MARKER[1])?, OperationBoolean::And)
        .map_err(vips::failed)?
        .boolean(&near(2, MARKER[2])?, OperationBoolean::And)
        .map_err(vips::failed)?;

    if image.hasalpha() {
        let opaque = image
            .extract_band(3)
            .map_err(vips::failed)?
            .relational_const(OperationRelational::Moreeq, &[128.0 * scale])
            .map_err(vips::failed)?;
        mask = mask.boolean(&opaque, OperationBoolean::And).map_err(vips::failed)?;
    }

    // Ruby finished with `ifthenelse(255, 0)`. It is identity here - every input
    // is a `relational_const` result, which libvips defines as 255 or 0, and a
    // bitwise AND of those stays 255 or 0 - and rs-vips has no constant form of
    // `ifthenelse`, so it is dropped. `mask_is_only_ever_zero_or_255` pins the
    // claim rather than leaving it as an argument.
    Ok(mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures").join(name)
    }

    fn write(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("snappr-svg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn length_reads_units_and_refuses_percentages() {
        assert_eq!(length("2000"), Some(2000.0));
        assert_eq!(length("2000px"), Some(2000.0));
        assert_eq!(length("2000.5pt"), Some(2000.5));
        assert_eq!(length(" 100mm "), Some(100.0));
        assert_eq!(length("100%"), None);
        assert_eq!(length(""), None);
        assert_eq!(length("auto"), None);
        // No sign, no exponent - neither appears in a real export.
        assert_eq!(length("-10"), None);
        assert_eq!(length("1e3"), Some(1.0));
    }

    #[test]
    fn a_leading_space_in_the_view_box_still_reads() {
        // The Ruby split the raw attribute and read the leading space as an empty
        // first field, so this file measured a zero-width window. Legal SVG, so
        // it is fixed here.
        let path = write("leading-space.svg", r#"<svg viewBox=" 0 0 1000 1480"></svg>"#);
        assert_eq!(view_box_numbers(&path, None).unwrap(), [0.0, 0.0, 1000.0, 1480.0]);
    }

    #[test]
    fn a_comma_separated_view_box_reads() {
        let path = write("commas.svg", r#"<svg viewBox="0,0,740,500"></svg>"#);
        assert_eq!(view_box_numbers(&path, None).unwrap(), [0.0, 0.0, 740.0, 500.0]);
    }

    #[test]
    fn a_missing_view_box_is_synthesised_from_width_and_height() {
        let path = write("no-viewbox.svg", r#"<svg width="1000" height="1480"></svg>"#);
        let source = read_source(&path, None).unwrap();
        assert_eq!(view_box_of(&source, &path, None).unwrap(), "0 0 1000.0 1480.0");
    }

    #[test]
    fn percentage_geometry_with_no_view_box_is_refused() {
        let path = write("percent-root.svg", r#"<svg width="100%" height="100%"></svg>"#);
        let source = read_source(&path, None).unwrap();
        let problem = view_box_of(&source, &path, None).unwrap_err();

        assert_eq!(problem.code(), "overlay_no_size");
        // The jargon belongs in the console half only.
        assert!(!problem.ui().to_lowercase().contains("view box"));
        assert!(problem.log_line().contains("view box"));
    }

    #[test]
    fn a_missing_file_is_reported_with_the_basename_only() {
        let problem = view_box_numbers(Path::new("/nope/brand.svg"), Some("landscape overlay"))
            .unwrap_err();

        assert_eq!(problem.code(), "overlay_file_missing");
        assert!(problem.ui().contains("brand.svg"));
        assert!(problem.ui().contains("landscape overlay"));
        assert!(!problem.ui().contains("/nope"));
        assert!(problem.log_line().contains("/nope/brand.svg"));
    }

    #[test]
    fn the_rewrite_sets_four_attributes_and_keeps_the_authored_view_box() {
        let path = write(
            "keeps.svg",
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1000 1480" width="1000" height="1480" fill="none"><rect x="0" y="0" width="10" height="10"/></svg>"#,
        );

        let out = String::from_utf8(source_for(&path, 2724, 4032, None).unwrap()).unwrap();

        assert!(out.contains(r#"viewBox="0 0 1000 1480""#), "{out}");
        assert!(out.contains(r#"width="2724""#), "{out}");
        assert!(out.contains(r#"height="4032""#), "{out}");
        assert!(out.contains(r#"preserveAspectRatio="none""#), "{out}");
        // Everything else survives, including attributes this module has no
        // opinion about and the whole child tree.
        assert!(out.contains(r#"xmlns="http://www.w3.org/2000/svg""#), "{out}");
        assert!(out.contains(r#"fill="none""#), "{out}");
        assert!(out.contains(r#"<rect x="0" y="0" width="10" height="10"/>"#), "{out}");
    }

    #[test]
    fn an_authored_preserve_aspect_ratio_is_overwritten_not_kept() {
        // An Illustrator export carrying `meet` would letterboxes the rounding
        // residue and leave a line down one edge of every print.
        let path = write(
            "illustrator.svg",
            r#"<svg viewBox="0 0 100 50" preserveAspectRatio="xMidYMid meet"></svg>"#,
        );

        let out = String::from_utf8(source_for(&path, 200, 100, None).unwrap()).unwrap();
        assert!(out.contains(r#"preserveAspectRatio="none""#), "{out}");
        assert!(!out.contains("meet"), "{out}");
    }

    #[test]
    fn the_declaration_comments_and_cdata_survive_the_rewrite() {
        let path = write(
            "rich.svg",
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!-- drawn by hand -->\n\
             <svg viewBox=\"0 0 10 10\"><style><![CDATA[.a{fill:#000}]]></style></svg>",
        );

        let out = String::from_utf8(source_for(&path, 20, 20, None).unwrap()).unwrap();
        assert!(out.contains("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"), "{out}");
        assert!(out.contains("<!-- drawn by hand -->"), "{out}");
        assert!(out.contains("<![CDATA[.a{fill:#000}]]>"), "{out}");
    }

    #[test]
    fn a_self_closing_marker_is_removed_and_everything_else_is_not() {
        let path = write(
            "marker.svg",
            r##"<svg viewBox="0 0 100 50"><rect fill="#FF00FF" width="50" height="50"/><rect fill="#000" width="10" height="10"/></svg>"##,
        );

        let stripped = String::from_utf8(stripped_source(&path, 100, 50, None).unwrap()).unwrap();
        assert!(!stripped.contains("FF00FF"), "{stripped}");
        assert!(stripped.contains(r##"fill="#000""##), "{stripped}");

        // ... and the un-stripped variant keeps it, which is what the window is
        // measured from.
        let kept = String::from_utf8(source_for(&path, 100, 50, None).unwrap()).unwrap();
        assert!(kept.contains("FF00FF"), "{kept}");
    }

    #[test]
    fn a_marker_group_is_removed_with_its_whole_subtree() {
        let path = write(
            "marker-group.svg",
            r##"<svg viewBox="0 0 100 50"><g fill="#f0f"><path d="M0 0H50V50H0V0Z"/><rect width="1" height="1"/></g><rect fill="#000" width="10" height="10"/></svg>"##,
        );

        let stripped = String::from_utf8(stripped_source(&path, 100, 50, None).unwrap()).unwrap();
        assert!(!stripped.contains("M0 0H50V50H0V0Z"), "the subtree went too: {stripped}");
        assert!(!stripped.contains("#f0f"), "{stripped}");
        assert!(stripped.contains(r##"fill="#000""##), "and its sibling stayed: {stripped}");
    }

    #[test]
    fn a_marker_declared_in_a_style_attribute_is_removed() {
        let path = write(
            "styled.svg",
            r#"<svg viewBox="0 0 100 50"><rect style="fill:#FF00FF; opacity:1" width="50" height="50"/></svg>"#,
        );

        let stripped = String::from_utf8(stripped_source(&path, 100, 50, None).unwrap()).unwrap();
        assert!(!stripped.contains("FF00FF"), "{stripped}");
    }

    #[test]
    fn a_magenta_root_is_never_treated_as_the_marker() {
        // Deleting the root would empty the document. Real overlays carry
        // `fill="none"` on the root, and one carrying magenta must still render.
        let path = write(
            "magenta-root.svg",
            r##"<svg viewBox="0 0 100 50" fill="#ff00ff"><rect fill="#000" width="10" height="10"/></svg>"##,
        );

        let stripped = String::from_utf8(stripped_source(&path, 100, 50, None).unwrap()).unwrap();
        assert!(stripped.contains("<svg"), "{stripped}");
        assert!(stripped.contains(r##"fill="#000""##), "{stripped}");
    }

    #[test]
    fn near_magenta_spellings_are_matched_and_near_misses_are_not() {
        let matches = ["#ff00ff", "#FF00FF", "#f0f", "magenta", "rgb(255, 0, 255)", "  #f0f  "];
        for value in matches {
            assert!(marker_pattern().is_match(value), "{value} should match");
        }
        let misses = ["#ff00fe", "#ff00ff00", "rgba(255,0,255,1)", "none", "#fff"];
        for value in misses {
            assert!(!marker_pattern().is_match(value), "{value} should not match");
        }
    }

    // --- The rendered half. These need libvips.

    #[test]
    fn the_marker_mask_is_only_ever_zero_or_255() {
        // This is what makes Ruby's trailing `ifthenelse(255, 0)` identity, and
        // therefore safe to drop.
        vips::init().unwrap();

        let path = write(
            "mask.svg",
            r##"<svg viewBox="0 0 100 50"><rect fill="#FF00FF" width="50" height="50"/></svg>"##,
        );
        let image = load_svg(&source_for(&path, 100, 50, None).unwrap()).unwrap();
        let mask = marker_mask(&image).unwrap();

        assert_eq!(mask.min().unwrap(), 0.0);
        assert_eq!(mask.max().unwrap(), 255.0);
    }

    #[test]
    fn the_window_is_found_through_a_transform() {
        // Geometry parsing would have to understand `transform` and path data;
        // a rendered pixel accounts for both.
        vips::init().unwrap();

        let window = photo_window(&fixture("window-portrait.svg"), None).unwrap();

        assert!((window.x - 80.0 / 2000.0).abs() < 0.005, "{window:?}");
        assert!((window.y - 80.0 / 2960.0).abs() < 0.005, "{window:?}");
        assert!((window.width - 1840.0 / 2000.0).abs() < 0.005, "{window:?}");
        assert!((window.height - 1616.0 / 2960.0).abs() < 0.005, "{window:?}");
    }

    #[test]
    fn an_overlay_with_no_marker_is_reported_clearly() {
        vips::init().unwrap();

        let problem =
            photo_window(&fixture("absolute.svg"), Some("landscape overlay")).unwrap_err();

        assert_eq!(problem.code(), "overlay_window_missing");
        assert!(problem.ui().contains("landscape overlay"));
        assert!(problem.ui().contains("absolute.svg"));
        // The operator is told to draw a magenta rectangle, not given a hex code.
        assert!(!problem.ui().contains("#FF00FF"));
        assert!(problem.ui().to_lowercase().contains("magenta"));
    }

    #[test]
    fn the_render_is_exactly_the_requested_size_and_has_alpha() {
        vips::init().unwrap();

        for (width, height) in [(600, 888), (2724, 4032)] {
            let image =
                render(&fixture("window-portrait.svg"), width, height, None, None).unwrap();
            assert_eq!((image.get_width(), image.get_height()), (width, height));
            assert!(image.hasalpha());
        }
    }

    #[test]
    fn the_marker_is_never_rendered() {
        vips::init().unwrap();

        let image = render(&fixture("window-landscape.svg"), 740, 500, None, None).unwrap();
        let mask = marker_mask(&image).unwrap();

        assert_eq!(mask.avg().unwrap(), 0.0, "no marker-coloured pixels survive the render");
    }

    #[test]
    fn the_window_really_is_a_hole() {
        vips::init().unwrap();

        let image = render(&fixture("window-landscape.svg"), 740, 500, None, None).unwrap();
        let alpha = image.extract_band(3).unwrap();

        assert!(alpha.min().unwrap() < 128.0, "the window is transparent");
        assert!(alpha.max().unwrap() > 200.0, "and the artwork is not");
    }

    #[test]
    fn artwork_scales_with_the_frame_in_every_dialect() {
        // The regression test. All three dialects - absolute coordinates, no view
        // box, and percentage geometry - must land the band in the same place,
        // because the authored view box is what maps one onto the other.
        vips::init().unwrap();

        let mut bounds = Vec::new();
        for dialect in ["absolute.svg", "no-viewbox.svg", "percent.svg"] {
            let image = render(&fixture(dialect), 2724, 4032, None, None).unwrap();
            let opaque = image
                .extract_band(3)
                .unwrap()
                .relational_const(OperationRelational::Moreeq, &[128.0])
                .unwrap();
            bounds.push(
                opaque
                    .find_trim_with_opts(
                        VOption::new().set("background", &[0.0][..]).set("threshold", 128.0),
                    )
                    .unwrap(),
            );
        }

        let (left, top, width, height) = bounds[0];
        assert_eq!(left, 0);
        assert_eq!(width, 2724, "the band spans the frame");
        assert!((top as f64 - 4032.0 * 0.6).abs() < 3.0, "top was {top}");
        assert!((height as f64 - 4032.0 * 0.4).abs() < 3.0, "height was {height}");

        for other in &bounds[1..] {
            assert_eq!(*other, bounds[0], "every dialect renders identically");
        }
    }
}
