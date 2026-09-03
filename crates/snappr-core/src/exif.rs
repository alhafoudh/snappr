//! The camera, lens and exposure off the source header, for the info line.
//!
//! Read from the image rather than the path, so it shares the header read the
//! pipeline has already paid for. libvips is lazy: none of this touches a pixel.
//!
//! **Read it from the post-`autorot` image.** `autorot` strips the orientation
//! tag once it has been applied, and everything else measured before it is
//! transposed on every portrait phone photo - so the same rule that decides the
//! crop decides where these fields are read.

use std::sync::OnceLock;

use regex::Regex;
use rs_vips::VipsImage;

/// What a photo says about itself. Every field is independently optional, and an
/// empty struct is a normal answer: plenty of photos carry no camera tags at all.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
// `optional_fields`, matching `skip_serializing_if` below: an absent tag is an
// absent *key*, so the TypeScript is `camera?: string` and not
// `camera: string | null`. The two are not the same claim, and the frontend
// already tests `exif?.camera`.
#[ts(export, optional_fields)]
pub struct Exif {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lens: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focal_length: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focal_length_35mm: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aperture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shutter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iso: Option<u32>,
    /// ISO 8601, and naive when the camera recorded no zone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub taken_at: Option<String>,
}

impl Exif {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// **EXIF tags come out of libvips as one annotated string per tag**, shaped
/// `<raw> (<human>, <Type>, <n> components, <n> bytes)` - FNumber, for instance,
/// is `"1244236/699009 (f/1.8, Rational, 1 components, 8 bytes)"`. The human half
/// is the one worth showing.
///
/// This anchors on the **tail**, not on "strip the last bracket": ApertureValue's
/// human half is itself parenthesised (`"1.66 EV (f/1.8)"`), so a bracket-counting
/// parse gets it wrong. Non-greedy `raw` stops at the first `" ("`; greedy `human`
/// then swallows everything up to the tail.
///
/// Known limit: a lens model containing `(` splits in the wrong place and comes
/// out slightly mangled. Rare enough not to guard, and it degrades to an ugly
/// string rather than a wrong one.
fn annotated() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        // (?s) is Ruby's /m: the value may contain newlines.
        Regex::new(r"(?s)^(?<raw>.*?) \((?<human>.*), \w+, \d+ components?, \d+ bytes\)$")
            .expect("a literal pattern")
    })
}

/// EXIF writes its timestamps with colons in the date, which no date parser
/// takes: `"2026:03:21 12:40:52"`. The zone arrives separately, if at all.
fn timestamp() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"^(\d{4}):(\d{2}):(\d{2})[ T](\d{2}:\d{2}:\d{2})").expect("a literal pattern")
    })
}

/// Everything this photo actually carries.
pub fn read(image: &VipsImage) -> Exif {
    let camera = camera_of(image);
    let lens = lens_of(image, camera.as_deref());

    Exif {
        camera,
        lens,
        // libvips spells it "6.8 mm"; a camera spells it "6.8mm".
        focal_length: human(image, "exif-ifd2-FocalLength")
            .map(|text| text.replace(' ', "")),
        focal_length_35mm: integer(image, "exif-ifd2-FocalLengthIn35mmFilm"),
        aperture: human(image, "exif-ifd2-FNumber"),
        // "1/120 sec." is how libvips spells it and "1/120s" is how a camera
        // does. Also covers the long end, where the tag reads "2.0 sec.".
        shutter: human(image, "exif-ifd2-ExposureTime").map(|text| {
            let trimmed = text.trim_end();
            match trimmed.strip_suffix("sec.") {
                Some(head) => format!("{}s", head.trim_end()),
                None => text,
            }
        }),
        iso: integer(image, "exif-ifd2-ISOSpeedRatings"),
        taken_at: taken_at_of(image),
    }
}

/// The model alone, not "make model".
///
/// Nikon, Canon and Fujifilm all repeat the make inside the model - "NIKON Z 6",
/// "Canon EOS R5" - so prepending it stutters, and Apple's make is the useless
/// "Apple". The model is what a person calls the camera. Make is the fallback only
/// for the odd file that carries one and no model.
fn camera_of(image: &VipsImage) -> Option<String> {
    human(image, "exif-ifd0-Model").or_else(|| human(image, "exif-ifd0-Make"))
}

/// The lens, unless it is the camera saying its own name again.
///
/// Apple writes LensModel as "<camera> back camera 6.765mm f/1.78": the camera
/// name, then the focal length and aperture the line already prints. One prefix
/// test drops every phone lens and keeps every interchangeable one, which is
/// exactly where the lens is the interesting fact.
fn lens_of(image: &VipsImage, camera: Option<&str>) -> Option<String> {
    let lens = human(image, "exif-ifd2-LensModel")?;
    if let Some(camera) = camera {
        // Full Unicode lowercasing, not ASCII-only: a non-Latin model name must
        // compare the same way.
        if lens.to_lowercase().starts_with(&camera.to_lowercase()) {
            return None;
        }
    }
    Some(lens)
}

/// ISO 8601, so the frontend can hand it straight to `Date`. The zone is its own
/// tag and is often missing, in which case the timestamp stays naive - better
/// than inventing an offset the camera never recorded.
fn taken_at_of(image: &VipsImage) -> Option<String> {
    let stamp = human(image, "exif-ifd2-DateTimeOriginal")
        .or_else(|| human(image, "exif-ifd0-DateTime"))?;
    let captures = timestamp().captures(&stamp)?;

    let offset = human(image, "exif-ifd2-OffsetTimeOriginal")
        .or_else(|| human(image, "exif-ifd2-OffsetTime"))
        .unwrap_or_default();

    Some(format!(
        "{}-{}-{}T{}{offset}",
        &captures[1], &captures[2], &captures[3], &captures[4]
    ))
}

/// The human half of one tag, or `None` if the photo does not carry it.
pub fn human(image: &VipsImage, name: &str) -> Option<String> {
    clean(&value(image, name)?)
}

/// The readable half of one annotated value, trimmed.
///
/// Split out from [`human`] because it is the whole of the parsing and none of
/// the I/O: libvips' annotation format is undocumented prose, and being able to
/// assert against it without writing a file is worth more than the indirection
/// costs.
pub fn clean(raw: &str) -> Option<String> {
    let text = match annotated().captures(raw) {
        Some(captures) => captures["human"].to_string(),
        None => raw.to_string(),
    };
    // NULs as well as whitespace: EXIF ASCII fields are routinely NUL-padded to
    // their declared length, and Rust's `trim` alone would leave those in, where
    // they reach the info line as a run of invisible characters.
    let text = text.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn integer(image: &VipsImage, name: &str) -> Option<u32> {
    let text = human(image, name)?;
    // Strictly digits: `parse()` alone would accept "+4" and "-4".
    if text.is_empty() || !text.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// One raw tag, as libvips holds it, or `None` when the photo does not carry it.
///
/// A missing tag is the normal case rather than an error - cameras vary wildly in
/// what they write - so this asks before reading. `get_typeof` answers that; Ruby
/// had to use `get_fields.include?` because its `get` raised.
///
/// **One difference from the Ruby, and it is a real if narrow loss.** Ruby read
/// these as raw bytes and scrubbed them, so a tag whose bytes are not valid UTF-8
/// arrived mangled but present (`"Leica \u{FFFD}M11"`). rs-vips' `get_string`
/// yields an *empty string* for such a value, and the raw image pointer is
/// `pub(crate)`, so there is no way from here to read the bytes instead. Such a
/// tag is therefore reported as absent.
///
/// What that costs, precisely: a camera that writes a tag in Latin-1 loses that
/// one field from the info line, where before it showed with a replacement
/// character. Accented names that are valid UTF-8 - which is most of them - are
/// unaffected, and the reason Ruby *needed* the scrub is gone anyway: it was
/// protecting `JSON.generate` from bytes that could not be encoded, and a Rust
/// `String` cannot hold those in the first place. If a real camera turns up in
/// this state, the fix is to read `exif-data` and parse the block directly.
pub fn value(image: &VipsImage, name: &str) -> Option<String> {
    if image.get_typeof(name).ok()? == 0 {
        return None;
    }

    match image.get_string(name) {
        Ok(text) if !text.is_empty() => Some(text),
        Ok(_) => None,
        Err(_) => {
            // A field of some other type - an int, a blob. Not an error worth
            // reporting: the photo simply does not carry this tag as text.
            crate::vips::clear_errors();
            None
        }
    }
}
