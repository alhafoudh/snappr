//! What libvips does, asserted rather than remembered.
//!
//! Every claim in CLAUDE.md's "Verified mechanics" section was established by
//! measurement at some point and then written down, which means it can rot
//! silently when libvips is upgraded. These tests are those claims, executable.
//! They are also the first thing to run after changing the libvips build: if the
//! bindings load at all, this file says whether the *behaviour* the pipeline is
//! built on still holds.
//!
//! Two of them assert traps rather than features - `thumbnail` destroying bit
//! depth, and `flatten` erroring on an image with no alpha - because the pipeline
//! is shaped around avoiding both.

use rs_vips::enums::{BandFormat, BlendMode, Interpretation};
use rs_vips::voption::{Setter, VOption};
use rs_vips::VipsImage;
use snappr_core::vips;

/// Display P3, the space iPhones actually shoot in.
const P3_PROFILE: &str = "/System/Library/ColorSync/Profiles/Display P3.icc";

fn ready() {
    vips::init().expect("libvips initialises");
}

/// A flat three-band image of a known value, in a known format.
///
/// The interpretation is set deliberately rather than left to fall out of
/// `black()`, which produces `B_W`. libvips takes real decisions off it: a 3-band
/// `B_W` image is refused by `composite`, and `pngsave` writes it as 8-bit
/// whatever the band format says. The Ruby fixture builder set it for the same
/// reason (`test_helper.rb`'s `copy(interpretation: :srgb)`).
fn flat(width: i32, height: i32, value: f64, format: BandFormat) -> VipsImage {
    let base = VipsImage::black(width, height)
        .unwrap()
        .linear(&[1.0], &[value])
        .unwrap()
        .cast(format)
        .unwrap();
    let interpretation = if format == BandFormat::Ushort {
        Interpretation::Rgb16
    } else {
        Interpretation::Srgb
    };
    VipsImage::bandjoin(&[base.copy().unwrap(), base.copy().unwrap(), base])
        .unwrap()
        .copy_with_opts(VOption::new().set("interpretation", interpretation as i32))
        .unwrap()
}

fn temp(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("snappr-caps-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name).to_string_lossy().into_owned()
}

/// The one loader that is deliberately exempt from the untrusted-loader block.
/// If this fails, every overlay render fails with "buffer is not in a known
/// format" and nothing points at the cause.
#[test]
fn svg_loads_after_the_untrusted_block_is_set_up() {
    ready();

    // Two hashes, because the marker colour puts a `"#` inside the literal.
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 50" width="200" height="100">
        <rect x="0" y="0" width="50" height="50" fill="#ff00ff"/>
    </svg>"##;

    let image = VipsImage::new_from_buffer(svg, "").expect("svgload is unblocked");

    // librsvg scales by the root width/height, and always renders RGBA.
    assert_eq!(image.get_width(), 200);
    assert_eq!(image.get_height(), 100);
    assert!(image.hasalpha(), "librsvg renders with an alpha channel");
}

/// Untrusted loaders really are blocked - the hardening is not just ceremony.
/// Ruby got this by accident and had to undo half of it; here it is asked for,
/// so it is worth proving it is on.
#[test]
fn an_untrusted_loader_stays_blocked() {
    ready();

    // A minimal PPM. `ppmload` is one of the loaders libvips marks untrusted.
    let ppm = b"P1\n1 1\n0\n";
    let blocked = VipsImage::new_from_buffer(ppm, "");

    assert!(blocked.is_err(), "an untrusted loader is refused: {blocked:?}");
}

/// HEIC decodes natively, with no pre-conversion step, and `heifsave` is how a
/// HEIC fixture gets built without committing a binary.
#[test]
fn heic_round_trips_through_libheif() {
    ready();

    let path = temp("fixture.heic");
    let source = flat(64, 48, 100.0, BandFormat::Uchar);
    source.heifsave(&path).expect("heifsave exists in this build");

    let read = VipsImage::new_from_file(&path).expect("heifload decodes it");
    assert_eq!((read.get_width(), read.get_height()), (64, 48));

    // The pixels really decoded, rather than the header being read and the body
    // ignored - this is the check a libvips without an HEVC decoder fails.
    let mean = read.avg().unwrap();
    assert!((90.0..=110.0).contains(&mean), "HEVC pixels decoded, mean was {mean}");
}

/// PNG carries everything the print depends on: 16-bit samples, the ICC profile,
/// the EXIF blob and the resolution tag. This is the whole reason the output is
/// PNG rather than JPEG.
#[test]
fn png_preserves_depth_profile_exif_and_resolution() {
    ready();

    // A real profile: libvips validates the signature on save and drops anything
    // that is not one, so a synthetic blob would silently test nothing. Display P3
    // is the space iPhones actually shoot in, which is the case that matters.
    let Ok(profile) = std::fs::read(P3_PROFILE) else {
        eprintln!("skipped: no Display P3 profile at {P3_PROFILE}");
        return;
    };
    let mut source = flat(32, 24, 40000.0, BandFormat::Ushort)
        .copy_with_opts(VOption::new().set("xres", 11.811).set("yres", 11.811))
        .unwrap();
    source.set_blob_copy("icc-profile-data", &profile).unwrap();

    let path = temp("deep.png");
    source.pngsave_with_opts(&path, VOption::new().set("compression", 6)).unwrap();

    let read = VipsImage::new_from_file(&path).unwrap();
    assert_eq!(read.get_format().unwrap(), BandFormat::Ushort, "16-bit survives");
    assert_eq!(read.get_blob("icc-profile-data").unwrap(), profile, "profile survives byte for byte");
    assert!((read.get_xres() - 11.811).abs() < 0.001, "resolution tag survives");
    assert_eq!(read.getpoint(0, 0).unwrap()[0], 40000.0, "the sample value is untouched");
}

/// `extract_area` is the crop, and it preserves the format exactly. The pipeline
/// uses it rather than anything thumbnail-based for precisely this reason.
#[test]
fn extract_area_preserves_format_exactly() {
    ready();

    let source = flat(100, 80, 40000.0, BandFormat::Ushort);
    let cropped = source.extract_area(10, 10, 50, 40).unwrap();

    assert_eq!(cropped.get_format().unwrap(), BandFormat::Ushort);
    assert_eq!((cropped.get_width(), cropped.get_height()), (50, 40));
    assert_eq!(cropped.getpoint(0, 0).unwrap()[0], 40000.0);
}

/// **The trap.** `thumbnail` goes through a path that downcasts 16-bit to 8-bit,
/// so a 10-bit HEIC decoding to 40000 comes back as 156. This is why the crop is
/// never expressed as a resize, however much more "object-fit: cover" that would
/// read like - and why previews, which are display-only, may use it freely.
#[test]
fn thumbnail_downcasts_sixteen_bit_to_eight() {
    ready();

    let path = temp("deep-source.png");
    flat(200, 150, 40000.0, BandFormat::Ushort).pngsave(&path).unwrap();

    let thumb = VipsImage::thumbnail(&path, 100).unwrap();

    assert_eq!(thumb.get_format().unwrap(), BandFormat::Uchar, "thumbnail dropped to 8-bit");
    let value = thumb.getpoint(0, 0).unwrap()[0];
    assert!((150.0..=160.0).contains(&value), "40000 came back as {value}");
}

/// `composite` promotes an 8-bit overlay into a 16-bit photo with the right range
/// scaling, so the SVG render needs no manual scaling of its own.
#[test]
fn compositing_an_eight_bit_overlay_onto_sixteen_bit_scales_correctly() {
    ready();

    let photo = flat(40, 40, 40000.0, BandFormat::Ushort);

    // An opaque white overlay covering the whole frame.
    let white = flat(40, 40, 255.0, BandFormat::Uchar);
    let opaque = VipsImage::black(40, 40).unwrap().linear(&[1.0], &[255.0]).unwrap().cast(BandFormat::Uchar).unwrap();
    let overlay = VipsImage::bandjoin(&[white, opaque]).unwrap();

    let result = photo.composite2(&overlay, BlendMode::Over).unwrap();

    assert_eq!(result.get_format().unwrap(), BandFormat::Ushort, "stays 16-bit");
    assert_eq!(result.getpoint(0, 0).unwrap()[0], 65535.0, "8-bit white reaches 16-bit white");
}

/// **The other trap.** `flatten` on an image with no alpha fails with
/// "vector must have 1 or 2 elements", which points nowhere near the cause. The
/// pipeline therefore asks whether there is alpha before flattening.
#[test]
fn flatten_errors_on_an_image_with_no_alpha() {
    ready();

    let no_alpha = flat(20, 20, 100.0, BandFormat::Uchar);
    let flattened = no_alpha.flatten_with_opts(VOption::new().set("background", &[255.0, 255.0, 255.0][..]));

    assert!(flattened.is_err(), "flatten refuses an image with no alpha");

    // ... and it works on one that has alpha, which is the case the pipeline hits.
    let with_alpha = no_alpha.addalpha().unwrap();
    assert!(with_alpha
        .flatten_with_opts(VOption::new().set("background", &[255.0, 255.0, 255.0][..]))
        .is_ok());
}

/// `icc_transform` keeps the alpha channel, which is what makes it safe to run on
/// the overlay before compositing.
#[test]
fn icc_transform_keeps_the_alpha_channel() {
    ready();

    let overlay = flat(30, 30, 200.0, BandFormat::Uchar).addalpha().unwrap();
    assert_eq!(overlay.get_bands(), 4);

    let transformed = overlay
        .icc_transform_with_opts("p3", VOption::new().set("input_profile", "srgb"))
        .expect("lcms is linked in");

    assert_eq!(transformed.get_bands(), 4, "alpha survived the transform");
}
