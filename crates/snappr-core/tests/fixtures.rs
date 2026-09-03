//! Writes the fixtures the build tasks need, using the *system* libvips.
//!
//! Only one so far: the HEIC that `cargo xtask verify` decodes. It has to be
//! written by something other than the thing under test - the whole claim being
//! checked is that the bundled libvips can decode a file it did not create - and
//! it cannot be committed, because a binary in the repo is a binary nobody can
//! review.
//!
//! A test rather than a build task because writing it needs libvips bindings, and
//! this is the crate that has them.

use rs_vips::enums::{BandFormat, Interpretation};
use rs_vips::voption::{Setter, VOption};
use rs_vips::VipsImage;
use snappr_core::vips;

/// Flat mid-grey, so the mean of any patch is a known number and `verify` can
/// tell a real decode from a header read.
const GREY: f64 = 120.0;

#[test]
fn writes_the_heic_the_bundle_gate_decodes() {
    vips::init().unwrap();

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = root.join("build/cache");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("verify.heic");

    // 2400x1600 is what the gate asserts it reads back, and it is 3:2 - a real
    // camera shape rather than the print's.
    let base = VipsImage::black(2400, 1600)
        .unwrap()
        .linear(&[1.0], &[GREY])
        .unwrap()
        .cast(BandFormat::Uchar)
        .unwrap();
    let image = VipsImage::bandjoin(&[base.copy().unwrap(), base.copy().unwrap(), base])
        .unwrap()
        .copy_with_opts(VOption::new().set("interpretation", Interpretation::Srgb as i32))
        .unwrap();

    image.heifsave(path.to_str().unwrap()).expect("heifsave needs libheif");

    // Read it back the way the gate will, so a fixture that cannot be decoded
    // fails here rather than looking like a bundling problem later.
    let read = VipsImage::new_from_file(&path).unwrap();
    assert_eq!((read.get_width(), read.get_height()), (2400, 1600));
    assert_eq!(read.get_string("vips-loader").unwrap_or_default(), "heifload");

    let patch = read.extract_area(0, 0, 64, 64).unwrap();
    let mean = patch.avg().unwrap();
    assert!((mean - GREY).abs() < 8.0, "the fixture decodes to {mean}, wanted about {GREY}");
}
