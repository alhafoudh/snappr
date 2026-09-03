//! Ported from `test/exif_test.rb`.
//!
//! Every assertion goes through a real libvips write and read-back, so the
//! annotated strings under test are the ones libvips actually produces rather
//! than a mock of them. That is what makes these tests worth anything: the format
//! being parsed is libvips' own prose, and it is not documented anywhere.

use std::path::PathBuf;

use rs_vips::enums::{BandFormat, Interpretation};
use rs_vips::voption::{Setter, VOption};
use rs_vips::VipsImage;
use snappr_core::exif::{self, Exif};
use snappr_core::vips;

/// A stand-in for what an iPhone writes, in the raw spellings EXIF uses -
/// rationals as fractions, the date with colons.
const APPLE: [(&str, &str); 10] = [
    ("exif-ifd0-Make", "Apple"),
    ("exif-ifd0-Model", "iPhone 17 Pro"),
    ("exif-ifd2-LensModel", "iPhone 17 Pro back camera 6.765mm f/1.78"),
    ("exif-ifd2-FNumber", "89/50"),
    ("exif-ifd2-ExposureTime", "1/120"),
    ("exif-ifd2-ISOSpeedRatings", "400"),
    ("exif-ifd2-FocalLength", "677/100"),
    ("exif-ifd2-FocalLengthIn35mmFilm", "24"),
    ("exif-ifd2-DateTimeOriginal", "2026:03:21 12:40:52"),
    ("exif-ifd2-OffsetTimeOriginal", "+01:00"),
];

fn dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snappr-exif-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes a JPEG carrying `tags`, then reads it back the way the pipeline does.
///
/// Round-tripping through a real file matters: libvips only renders a tag in its
/// annotated form (`"89/50 (f/1.8, Rational, 1 components, 8 bytes)"`) once it has
/// been through an actual EXIF block.
fn photo_with(name: &str, tags: &[(&str, &str)]) -> Exif {
    vips::init().unwrap();

    let base = VipsImage::black(64, 48)
        .unwrap()
        .linear(&[1.0], &[120.0])
        .unwrap()
        .cast(BandFormat::Uchar)
        .unwrap();
    let mut image = VipsImage::bandjoin(&[base.copy().unwrap(), base.copy().unwrap(), base])
        .unwrap()
        .copy_with_opts(VOption::new().set("interpretation", Interpretation::Srgb as i32))
        .unwrap();

    for (tag, value) in tags {
        image.set_string(*tag, value).unwrap();
    }

    let path = dir().join(name);
    image.jpegsave(path.to_str().unwrap()).unwrap();

    let read = VipsImage::new_from_file(&path).unwrap();
    exif::read(&read)
}

#[test]
fn reads_the_whole_apple_set() {
    let exif = photo_with("apple.jpg", &APPLE);

    assert_eq!(exif.camera.as_deref(), Some("iPhone 17 Pro"));
    assert_eq!(exif.focal_length.as_deref(), Some("6.8mm"));
    assert_eq!(exif.focal_length_35mm, Some(24));
    assert_eq!(exif.aperture.as_deref(), Some("f/1.8"));
    assert_eq!(exif.shutter.as_deref(), Some("1/120s"));
    assert_eq!(exif.iso, Some(400));
    assert_eq!(exif.taken_at.as_deref(), Some("2026-03-21T12:40:52+01:00"));
}

#[test]
fn a_photo_with_no_camera_tags_reads_as_empty() {
    let exif = photo_with("bare.jpg", &[]);
    assert!(exif.is_empty(), "{exif:?}");
}

/// Every field is independently optional, so dropping any one tag must not empty
/// the rest. Cameras vary wildly in what they write.
#[test]
fn dropping_any_single_tag_never_empties_the_rest() {
    for skipped in 0..APPLE.len() {
        let tags: Vec<(&str, &str)> = APPLE
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != skipped)
            .map(|(_, pair)| *pair)
            .collect();

        let exif = photo_with(&format!("partial-{skipped}.jpg"), &tags);
        assert!(!exif.is_empty(), "dropping {} emptied it", APPLE[skipped].0);
    }
}

/// The human half wins: the raw half of FNumber is `1244236/699009`.
#[test]
fn the_human_half_of_a_rational_is_what_is_shown() {
    let exif = photo_with("aperture.jpg", &[("exif-ifd2-FNumber", "89/50")]);
    assert_eq!(exif.aperture.as_deref(), Some("f/1.8"));
}

/// The parse anchors on the annotation's tail, not on the last bracket, because
/// this tag's human half is itself parenthesised.
#[test]
fn an_annotation_whose_human_half_is_parenthesised_still_parses() {
    vips::init().unwrap();

    let base = VipsImage::black(8, 8).unwrap().cast(BandFormat::Uchar).unwrap();
    let mut image = VipsImage::bandjoin(&[base.copy().unwrap(), base.copy().unwrap(), base])
        .unwrap()
        .copy_with_opts(VOption::new().set("interpretation", Interpretation::Srgb as i32))
        .unwrap();
    image.set_string("exif-ifd2-ApertureValue", "107/64").unwrap();

    let path = dir().join("aperture-value.jpg");
    image.jpegsave(path.to_str().unwrap()).unwrap();
    let read = VipsImage::new_from_file(&path).unwrap();

    let text = exif::human(&read, "exif-ifd2-ApertureValue").unwrap();
    // libvips renders this as "1.67 EV (f/1.8, Rational, 1 components, 8 bytes)".
    assert!(text.contains("EV"), "{text}");
    assert!(text.contains("f/"), "the inner bracket survived the parse: {text}");
    assert!(!text.contains("components"), "the annotation tail was removed: {text}");
}

/// **Each of these tests builds its image at a size no other test uses**, and
/// that is not fussiness. libvips has an operation cache: `black(4, 4)` called
/// twice with identical arguments hands back *the same image*, so a tag set on
/// one turns up on the other, and this test failed with a camera name another
/// test had written.
#[test]
fn asking_for_an_absent_tag_is_none() {
    vips::init().unwrap();
    let image = VipsImage::black(3, 3).unwrap();
    assert_eq!(exif::value(&image, "exif-ifd0-Model"), None);
    assert_eq!(exif::human(&image, "exif-ifd0-Model"), None);
}

/// Apple writes LensModel as the camera name plus what the line already says, so
/// one prefix test drops every phone lens.
#[test]
fn a_lens_that_restates_the_camera_is_dropped() {
    let exif = photo_with("phone-lens.jpg", &APPLE);
    assert_eq!(exif.lens, None, "{exif:?}");
}

/// ... and keeps every interchangeable one, which is where the lens is the
/// interesting fact.
#[test]
fn a_real_lens_is_kept() {
    let exif = photo_with(
        "nikon.jpg",
        &[
            ("exif-ifd0-Make", "NIKON CORPORATION"),
            ("exif-ifd0-Model", "NIKON Z 6"),
            ("exif-ifd2-LensModel", "NIKKOR Z 24-70mm f/2.8 S"),
        ],
    );

    assert_eq!(exif.camera.as_deref(), Some("NIKON Z 6"));
    assert_eq!(exif.lens.as_deref(), Some("NIKKOR Z 24-70mm f/2.8 S"));
}

/// The model alone, not "make model" - and Make is the fallback only.
#[test]
fn the_camera_is_the_model_with_make_as_a_fallback() {
    let both = photo_with(
        "both.jpg",
        &[("exif-ifd0-Make", "Apple"), ("exif-ifd0-Model", "iPhone 17 Pro")],
    );
    assert_eq!(both.camera.as_deref(), Some("iPhone 17 Pro"));

    let make_only = photo_with("make-only.jpg", &[("exif-ifd0-Make", "Apple")]);
    assert_eq!(make_only.camera.as_deref(), Some("Apple"));
}

/// No offset tag means a naive timestamp, not an invented zone.
#[test]
fn a_timestamp_with_no_offset_stays_naive() {
    let exif = photo_with(
        "naive.jpg",
        &[("exif-ifd2-DateTimeOriginal", "2026:03:21 12:40:52")],
    );
    assert_eq!(exif.taken_at.as_deref(), Some("2026-03-21T12:40:52"));
}

#[test]
fn the_timestamp_falls_back_to_the_ifd0_tag() {
    let exif = photo_with("ifd0-date.jpg", &[("exif-ifd0-DateTime", "2026:03:21 12:40:52")]);
    assert_eq!(exif.taken_at.as_deref(), Some("2026-03-21T12:40:52"));
}

/// The long end of the exposure range, where the tag reads "2.0 sec.".
#[test]
fn a_long_exposure_reads_in_seconds() {
    let exif = photo_with("long.jpg", &[("exif-ifd2-ExposureTime", "2/1")]);
    assert_eq!(exif.shutter.as_deref(), Some("2s"));
}

/// An accented name that is valid UTF-8 survives intact. libvips writes EXIF
/// ASCII fields to the 7-bit spec, transliterating accents away, so this one is
/// set on an in-memory image rather than round-tripped through a file - which is
/// exactly what the Ruby test had to do too.
#[test]
fn an_accented_name_survives() {
    vips::init().unwrap();

    // Size unique to this test - see the note on the operation cache above.
    let mut image = VipsImage::black(5, 5).unwrap();
    image.set_string("exif-ifd0-Model", "Hasselblad 907X édition").unwrap();

    let text = exif::human(&image, "exif-ifd0-Model").unwrap();
    assert_eq!(text, "Hasselblad 907X édition");
    // And it round-trips through the wire format the frontend receives.
    let json = serde_json::to_string(&text).unwrap();
    assert_eq!(serde_json::from_str::<String>(&json).unwrap(), text);
}

/// EXIF ASCII fields are routinely NUL-padded to their declared length. Left in,
/// the padding reaches the info line as a run of invisible characters.
///
/// Asserted against the parse rather than through a file, because libvips cannot
/// be *given* a string with an interior NUL - `set_string` refuses it - while a
/// real camera file carries them freely.
#[test]
fn nul_padding_is_trimmed() {
    assert_eq!(exif::clean("Canon EOS R5\0\0\0").as_deref(), Some("Canon EOS R5"));
    assert_eq!(exif::clean("  spaced  ").as_deref(), Some("spaced"));
    assert_eq!(exif::clean("\0\0"), None, "a value that is only padding is absent");
    assert_eq!(exif::clean(""), None);
}

/// The annotation format, asserted directly. This is libvips' own prose and it
/// is documented nowhere, so the shapes that have actually been seen are pinned.
#[test]
fn the_annotation_tail_is_what_the_parse_anchors_on() {
    assert_eq!(
        exif::clean("1244236/699009 (f/1.8, Rational, 1 components, 8 bytes)").as_deref(),
        Some("f/1.8")
    );
    // The human half is itself parenthesised - a bracket-counting parse gets
    // this one wrong.
    assert_eq!(
        exif::clean("107/64 (1.66 EV (f/1.8), Rational, 1 components, 8 bytes)").as_deref(),
        Some("1.66 EV (f/1.8)")
    );
    // Singular "component" happens too.
    assert_eq!(
        exif::clean("400 (400, Short, 1 component, 2 bytes)").as_deref(),
        Some("400")
    );
    // An unannotated value is used verbatim, which is the path a tag set on an
    // in-memory image takes.
    assert_eq!(exif::clean("iPhone 17 Pro").as_deref(), Some("iPhone 17 Pro"));
}

#[test]
fn an_integer_field_refuses_anything_that_is_not_digits() {
    vips::init().unwrap();

    let mut image = VipsImage::black(6, 6).unwrap();
    image.set_string("exif-ifd2-ISOSpeedRatings", "400").unwrap();
    image.set_string("exif-ifd2-FocalLengthIn35mmFilm", "+24").unwrap();

    let exif = exif::read(&image);
    assert_eq!(exif.iso, Some(400));
    assert_eq!(exif.focal_length_35mm, None, "a leading sign is not a number here");
}

/// Absent fields are absent keys on the wire, not nulls - the frontend tests
/// `photo.exif?.camera` and a null would read as "the camera wrote nothing" just
/// the same, but the payload stays small and the shape matches what Ruby sent.
#[test]
fn absent_fields_are_absent_from_the_json() {
    let exif = Exif { camera: Some("iPhone 17 Pro".into()), ..Default::default() };
    let json = serde_json::to_string(&exif).unwrap();

    assert_eq!(json, r#"{"camera":"iPhone 17 Pro"}"#);
}
