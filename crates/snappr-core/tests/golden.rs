//! Diffs this pipeline against the one it replaces.
//!
//! `build/goldens/` was captured by running the **Ruby** pipeline over a fixture
//! matrix before any of it was deleted: the sources are kept, so this reads the
//! identical bytes, and the manifest records each case's layout, its output's
//! pixels as a SHA-256, its bit depth, its ICC profile, its resolution and its
//! warnings. Reproducing all of that is what "the port is faithful" means, and it
//! is the only check here that could catch a one-pixel rounding difference -
//! which would not fail loudly, it would move the seam.
//!
//! **Skipped, not failed, when the goldens are absent.** They are gitignored
//! build output, so a fresh clone has none, and the capture needs a Ruby
//! pipeline that will not exist for much longer. Once Ruby is gone this file
//! becomes a regression test against a frozen snapshot rather than a comparison
//! between two implementations - which is the point of committing the manifest
//! when the time comes.

use std::path::{Path, PathBuf};

use rs_vips::VipsImage;
use serde_json::Value;
use sha2::{Digest, Sha256};
use snappr_core::config::PipelineConfig;
use snappr_core::overlay::Overlay;
use snappr_core::problem::name_of;
use snappr_core::vips;

/// Cases still in a capture whose overlay is no longer in the repo. Their SVG was
/// a real client's branding and was removed before the repo went public, so an
/// older capture names a file that cannot be read.
const RETIRED: &[&str] = &["landscape_8bit_share_overlay"];

fn goldens() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../build/goldens");
    dir.join("manifest.json").is_file().then_some(dir)
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Everything measurable about a written image, in the same shape the Ruby
/// capture recorded.
struct Measured {
    width: i32,
    height: i32,
    bands: i32,
    format: String,
    xres: f64,
    yres: f64,
    icc_sha: Option<String>,
    pixels_sha: String,
}

fn measure(path: &Path) -> Measured {
    let image = VipsImage::new_from_file(path).expect("the output loads");
    let icc_sha = image.get_blob("icc-profile-data").ok().map(|blob| sha256(&blob));

    Measured {
        width: image.get_width(),
        height: image.get_height(),
        bands: image.get_bands(),
        format: format!("{:?}", image.get_format().unwrap()).to_lowercase(),
        xres: image.get_xres(),
        yres: image.get_yres(),
        icc_sha,
        // The pixels alone, independent of any PNG encoding choice. This is the
        // assertion that matters: the file's own SHA depends on zlib and on
        // pngsave's defaults, the pixels do not.
        pixels_sha: sha256(&image.write_to_memory()),
    }
}

fn config_for(case: &Value) -> PipelineConfig {
    let path_of = |key: &str| {
        case.get(key).and_then(Value::as_str).map(PathBuf::from)
    };

    let mut config = PipelineConfig {
        overlay_landscape: path_of("overlay_landscape"),
        overlay_portrait: path_of("overlay_portrait"),
        ..Default::default()
    };

    if let Some(background) = case
        .get("overrides")
        .and_then(|o| o.get("background"))
        .and_then(Value::as_str)
    {
        config.background = background.to_string();
    }
    config
}

#[test]
fn the_rust_pipeline_reproduces_the_ruby_output() {
    let Some(goldens) = goldens() else {
        eprintln!("skipped: no build/goldens - run the capture against the Ruby pipeline first");
        return;
    };
    vips::init().unwrap();

    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(goldens.join("manifest.json")).unwrap())
            .unwrap();
    let cases = manifest["cases"].as_object().expect("cases");
    assert!(!cases.is_empty());

    let work = std::env::temp_dir().join(format!("snappr-golden-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);

    let mut checked = 0;
    let mut failures: Vec<String> = Vec::new();

    for (name, case) in cases {
        if RETIRED.contains(&name.as_str()) {
            continue;
        }
        if case.get("error").is_some() {
            // The Ruby run failed this case on purpose; nothing to compare.
            continue;
        }

        // Run on a *copy*, in a directory of its own: the pipeline writes beside
        // the source, and writing beside the golden would overwrite the thing
        // being compared against.
        let source = goldens.join(case["source"].as_str().unwrap());
        let dir = work.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let staged = dir.join(source.file_name().unwrap());
        std::fs::copy(&source, &staged).unwrap();

        let overlay = Overlay::new(config_for(case));
        let outcome = match overlay.process(&staged) {
            Ok(outcome) => outcome,
            Err(problem) => {
                failures.push(format!("{name}: failed - {}", problem.log_line()));
                continue;
            }
        };

        let expected = &case["result"];
        let want = &case["output_measured"];
        let got = measure(&outcome.destination);
        // Collected per case and merged at the end, so the closure below owns the
        // only mutable borrow while it is alive.
        let mut mismatches: Vec<String> = Vec::new();
        let mut wrong = |what: &str, got: String, want: String| {
            if got != want {
                mismatches.push(format!("{name}: {what} was {got}, Ruby gave {want}"));
            }
        };

        // 1. The layout, first and on its own. Pure integers, no encoder
        //    involved, so a rounding difference shows up here and nowhere else.
        let layout = &expected["layout"];
        for (field, value) in [
            ("canvas_width", outcome.layout.canvas_width),
            ("canvas_height", outcome.layout.canvas_height),
            ("photo_left", outcome.layout.photo_left),
            ("photo_top", outcome.layout.photo_top),
            ("photo_width", outcome.layout.photo_width),
            ("photo_height", outcome.layout.photo_height),
            ("crop_left", outcome.layout.crop_left),
            ("crop_top", outcome.layout.crop_top),
        ] {
            wrong(
                &format!("layout.{field}"),
                value.to_string(),
                layout[field].as_i64().unwrap().to_string(),
            );
        }

        // 2. The pixels.
        wrong(
            "pixels",
            got.pixels_sha,
            want["pixels_sha"].as_str().unwrap().to_string(),
        );

        // 3. Everything else the print depends on.
        wrong("width", got.width.to_string(), want["width"].to_string());
        wrong("height", got.height.to_string(), want["height"].to_string());
        wrong("bands", got.bands.to_string(), want["bands"].to_string());
        wrong("format", got.format, want["format"].as_str().unwrap().to_string());
        wrong("depth", outcome.depth.to_string(), expected["depth"].to_string());
        wrong(
            "icc profile",
            format!("{:?}", got.icc_sha),
            format!("{:?}", want["icc_sha"].as_str().map(str::to_string)),
        );
        for (field, value) in [("xres", got.xres), ("yres", got.yres)] {
            let wanted = want[field].as_f64().unwrap();
            let close = (value - wanted).abs() <= 0.01;
            wrong(field, close.to_string(), true.to_string());
            if !close {
                wrong(&format!("{field} value"), value.to_string(), wanted.to_string());
            }
        }

        // 4. The orientation and the chosen overlay.
        wrong(
            "orientation",
            outcome.orientation.as_str().to_string(),
            expected["orientation"].as_str().unwrap().to_string(),
        );
        // By basename: the manifest recorded repo-relative paths and the outcome
        // carries absolute ones, and which of the two an overlay *is* has never
        // been the question here.
        let svg = outcome.svg.as_ref().map(|path| name_of(path));
        let wanted_svg = expected["svg"]
            .as_str()
            .map(|path| name_of(Path::new(path)));
        wrong("overlay", format!("{svg:?}"), format!("{wanted_svg:?}"));

        // 5. The warnings, by code. Wording is free to change; codes are not.
        let mut got_kinds: Vec<&str> =
            outcome.warnings.iter().map(|w| w.kind.as_str()).collect();
        got_kinds.sort_unstable();
        let mut want_kinds: Vec<&str> = expected["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["kind"].as_str().unwrap())
            .collect();
        want_kinds.sort_unstable();
        wrong("warnings", format!("{got_kinds:?}"), format!("{want_kinds:?}"));

        // 6. EXIF, field by field.
        match (&outcome.exif, expected.get("exif")) {
            (Some(got), Some(want)) => {
                let got = serde_json::to_value(got).unwrap();
                for (field, value) in want.as_object().unwrap() {
                    wrong(
                        &format!("exif.{field}"),
                        format!("{}", got.get(field).unwrap_or(&Value::Null)),
                        format!("{value}"),
                    );
                }
            }
            (None, None) => {}
            (got, want) => wrong("exif presence", got.is_some().to_string(), want.is_some().to_string()),
        }

        drop(wrong);
        failures.extend(mismatches);
        checked += 1;
    }

    assert!(checked >= 14, "only {checked} cases ran");
    assert!(failures.is_empty(), "{} difference(s):\n{}", failures.len(), failures.join("\n"));
    eprintln!("{checked} golden cases reproduced exactly");
}

/// The invariant the whole design protects, asserted the way the Ruby suite
/// phrased it: **the photo is bit-identical to the source everywhere the overlay
/// is fully transparent.**
///
/// Phrased that way on purpose. The overlay is meant to draw *over* the photo, so
/// pixels under artwork are supposed to change; an inset-border check passes only
/// for designs whose artwork stays out of the window, and the real branding draws
/// an anti-aliased edge into it.
#[test]
fn the_photo_survives_bit_for_bit_wherever_the_overlay_is_clear() {
    use rs_vips::enums::OperationRelational;
    use rs_vips::voption::{Setter, VOption};

    let Some(goldens) = goldens() else {
        eprintln!("skipped: no build/goldens");
        return;
    };
    vips::init().unwrap();

    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(goldens.join("manifest.json")).unwrap())
            .unwrap();

    let work = std::env::temp_dir().join(format!("snappr-clear-{}", std::process::id()));

    for name in [
        "landscape_8bit_fixture_overlay",
        "portrait_8bit_fixture_overlay",
        "landscape_8bit_shipped_sample",
        "landscape_16bit_fixture_overlay",
    ] {
        let case = &manifest["cases"][name];
        let source = goldens.join(case["source"].as_str().unwrap());

        let dir = work.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let staged = dir.join(source.file_name().unwrap());
        std::fs::copy(&source, &staged).unwrap();

        let config = config_for(case);
        let outcome = Overlay::new(config.clone()).process(&staged).unwrap();
        let layout = outcome.layout;
        let svg = outcome.svg.clone().expect("these cases all have an overlay");

        let output = VipsImage::new_from_file(&outcome.destination).unwrap();
        let original = VipsImage::new_from_file(&staged).unwrap().autorot().unwrap();
        let overlay = snappr_core::overlay_svg::render(
            &svg,
            layout.canvas_width,
            layout.canvas_height,
            None,
            None,
        )
        .unwrap();

        let region = |image: &VipsImage, left: i32, top: i32| {
            image
                .extract_area(left, top, layout.photo_width, layout.photo_height)
                .unwrap()
        };

        let got = region(&output, layout.photo_left, layout.photo_top);
        let want = region(&original, layout.crop_left, layout.crop_top);
        let clear = region(&overlay, layout.photo_left, layout.photo_top)
            .extract_band(3)
            .unwrap()
            // Alpha *exactly* zero. "Nearly transparent" is where the artwork
            // legitimately blends into the photo.
            .relational_const(OperationRelational::Equal, &[0.0])
            .unwrap();

        let difference = got
            .cast(rs_vips::enums::BandFormat::Float)
            .unwrap()
            .subtract(&want.cast(rs_vips::enums::BandFormat::Float).unwrap())
            .unwrap()
            .abs()
            .unwrap()
            .bandbool(rs_vips::enums::OperationBoolean::Or)
            .unwrap();

        let zero = VipsImage::black(1, 1).unwrap();
        let masked = clear.ifthenelse_with_opts(
            &difference,
            &zero,
            VOption::new().set("blend", false),
        );
        let worst = masked.unwrap().max().unwrap();

        assert_eq!(worst, 0.0, "{name}: the photo changed under a clear overlay");
    }
}
