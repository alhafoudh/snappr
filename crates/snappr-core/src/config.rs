//! What the pipeline reads, and the one place its defaults are written.
//!
//! There used to be two defaults tables - `Config::DEFAULTS` in Ruby and
//! `Settings::default` in Rust - each holding the same twelve values as literals,
//! with a handshake in the middle copying six of them across. They agreed by
//! convention. [`PipelineConfig::default`] is now the only one, and the app's
//! `settings.json` derives from it rather than restating it.
//!
//! **Two front ends, two ways in, and that asymmetry is deliberate.** The app
//! builds a `PipelineConfig` from the operator's saved settings, which the UI
//! rewrites on every toggle. The CLI reads a hand-edited file at
//! `~/.config/snappr/config.toml`. The app does not read that file and never has
//! read the CLI's config - keeping them separate is what stops a kiosk's live
//! settings being silently overridden by something a developer left in a dotfile.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::problem::Problem;

/// The two overlay slots, which are also the two crop shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
// The output directory comes from `TS_RS_EXPORT_DIR` in `.cargo/config.toml`,
// which is `src/bindings/` for every crate in the workspace.
#[ts(export)]
pub enum Orientation {
    Landscape,
    Portrait,
}

impl Orientation {
    pub const ALL: [Self; 2] = [Self::Landscape, Self::Portrait];

    /// Which way round a photo is.
    ///
    /// **A square photo is landscape.** Arbitrary, but it has to be one of them,
    /// and the print is landscape by default.
    pub fn of(width: i32, height: i32) -> Self {
        if width >= height {
            Self::Landscape
        } else {
            Self::Portrait
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Landscape => "landscape",
            Self::Portrait => "portrait",
        }
    }

    /// How the operator's messages name this slot: "the landscape overlay ...".
    pub fn slot(self) -> String {
        format!("{} overlay", self.as_str())
    }
}

/// Everything the pipeline and the watcher read. Resolved: paths are absolute and
/// overlays are a path or nothing, because deciding *which* overlay is the front
/// end's job - the app resolves the shipped sample out of its resource directory,
/// the CLI out of the repo, and neither location is knowable from here.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineConfig {
    pub watch_dir: PathBuf,
    pub extensions: Vec<String>,
    /// Dual-purpose: it names the output file *and* is the watcher's self-output
    /// guard, so the two can never disagree about what a print is called.
    pub output_suffix: String,
    pub background: String,
    pub png_compression: i32,
    pub print_short_mm: f64,
    pub print_long_mm: f64,
    pub preview_max_px: i32,
    pub preview_thumb_px: i32,
    pub preview_quality: i32,
    pub stability_checks: u32,
    pub stability_timeout_secs: u64,
    /// Whether to hand the finished print to `open(1)`. The CLI's whole output
    /// story; the app draws it in the window instead and leaves this off.
    pub open_after: bool,
    pub overlay_landscape: Option<PathBuf>,
    pub overlay_portrait: Option<PathBuf>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            watch_dir: home().join("Downloads"),
            extensions: ["heic", "heif", "jpg", "jpeg"].map(String::from).to_vec(),
            output_suffix: ".overlay.png".into(),
            background: "#ffffff".into(),
            png_compression: 6,
            print_short_mm: 100.0,
            print_long_mm: 148.0,
            preview_max_px: 2000,
            preview_thumb_px: 400,
            preview_quality: 80,
            stability_checks: 2,
            stability_timeout_secs: 30,
            open_after: true,
            // No branding by default. The overlays are an operator setting, not
            // an asset this project ships.
            overlay_landscape: None,
            overlay_portrait: None,
        }
    }
}

impl PipelineConfig {
    /// How long a transfer may go on looking unfinished before the watcher gives
    /// up on it and says so.
    pub fn stability_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.stability_timeout_secs)
    }

    /// Target width/height ratio of the print, with the long side following the
    /// photo's own orientation.
    pub fn print_ratio(&self, orientation: Orientation) -> f64 {
        match orientation {
            Orientation::Landscape => self.print_long_mm / self.print_short_mm,
            Orientation::Portrait => self.print_short_mm / self.print_long_mm,
        }
    }

    /// The overlay to composite for an orientation, or `None` for none.
    ///
    /// `None` is a legitimate configuration, not a failure: the operator can clear
    /// either slot, and a cleared slot prints the crop with no branding over it.
    pub fn overlay(&self, orientation: Orientation) -> Option<&Path> {
        match orientation {
            Orientation::Landscape => self.overlay_landscape.as_deref(),
            Orientation::Portrait => self.overlay_portrait.as_deref(),
        }
    }

    /// Fills whatever the overlay does not cover and the photo does not reach.
    pub fn background_rgb(&self) -> Result<[f64; 3], Problem> {
        let hex = self.background.trim().trim_start_matches('#');
        let valid = hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit());
        if !valid {
            return Err(Problem::BackgroundNotAColour { value: self.background.clone() });
        }
        let channel = |at: usize| {
            i64::from_str_radix(&hex[at..at + 2], 16).expect("six hex digits") as f64
        };
        Ok([channel(0), channel(2), channel(4)])
    }

    /// Forces every value that is only checked when it is used, so a bad colour
    /// or a nonsense paper size is a startup failure rather than a surprise on
    /// some later photo.
    ///
    /// **Both front ends answer to this**: the CLI when it loads its TOML, the app
    /// when the settings window saves. The app's settings were never validated at
    /// all while the only way to change them was a hand-edited JSON file; now that
    /// a form can write every one of them, this is the gate.
    pub fn validate(&self) -> Result<(), Problem> {
        self.background_rgb()?;

        let (short, long) = (self.print_short_mm, self.print_long_mm);
        if !(short.is_finite() && long.is_finite() && short > 0.0 && short <= long) {
            return Err(Problem::PaperSizeInvalid { short_mm: short, long_mm: long });
        }
        if !(0..=9).contains(&self.png_compression) {
            return Err(Problem::CompressionOutOfRange { value: self.png_compression });
        }
        if self.preview_thumb_px <= 0 || self.preview_max_px < self.preview_thumb_px {
            return Err(Problem::PreviewSizeInvalid {
                max_px: self.preview_max_px,
                thumb_px: self.preview_thumb_px,
            });
        }
        if !(1..=100).contains(&self.preview_quality) {
            return Err(Problem::PreviewQualityOutOfRange { value: self.preview_quality });
        }
        if self.stability_checks == 0 || self.stability_timeout_secs == 0 {
            return Err(Problem::StabilityInvalid {
                checks: self.stability_checks,
                timeout_secs: self.stability_timeout_secs,
            });
        }
        self.validate_watch_rules()
    }

    /// The self-output guard, enforced rather than hoped for.
    ///
    /// The print lands in the folder being watched, so the one thing that must
    /// never happen is a print that the watcher would take for a photo - that is
    /// the `IMG_1234.overlay.overlay.jpg` loop described under "The self-output
    /// guard" in CLAUDE.md. Two rules close it between them: **the suffix always
    /// ends in `.png`** (which is also the lossless invariant, since libvips picks
    /// its saver from the extension and anything else would be JPEG or worse),
    /// and **`png` is never a watched extension**. Each alone leaves a gap.
    ///
    /// The marker before `.png` is required so the guard still has something
    /// distinctive to match on: a bare `.png` suffix would name the print
    /// `IMG_1234.png` and the guard would skip every PNG in the folder.
    fn validate_watch_rules(&self) -> Result<(), Problem> {
        if self.extensions.is_empty() {
            return Err(Problem::NoExtensions);
        }
        if self.extensions.iter().any(|ext| ext.eq_ignore_ascii_case("png")) {
            return Err(Problem::PngWatched);
        }

        let suffix = &self.output_suffix;
        let invalid = |reason: &'static str| {
            Err(Problem::OutputSuffixInvalid { value: suffix.clone(), reason })
        };
        if suffix.trim() != suffix {
            return invalid("it starts or ends with a space");
        }
        if suffix.contains('/') {
            return invalid("it contains a /");
        }
        let lower = suffix.to_ascii_lowercase();
        let Some(marker) = lower.strip_suffix(".png") else {
            return invalid("it has to end in .png");
        };
        if marker.is_empty() {
            return invalid("it needs something before .png, like .overlay.png");
        }
        Ok(())
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// Expands a leading `~`, then resolves a relative path against `base`.
///
/// `base` is the config file's own directory rather than the working directory,
/// so an overlay path in a config file means the same thing whatever folder the
/// CLI is run from.
pub fn resolve(value: &str, base: &Path) -> PathBuf {
    let value = value.trim();
    let expanded = if value == "~" {
        home()
    } else if let Some(rest) = value.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(value)
    };

    if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    }
}

// --- The CLI's config file -------------------------------------------------

/// `~/.config/snappr/config.toml`, as written on disk.
///
/// Every field is optional and every absent field keeps its default, so **a
/// missing file is a normal, fully working configuration** - `snappr overlay
/// photo.heic` works on a machine that has never been configured. That is what
/// makes it safe for `load` to treat "no file" as success.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    watch_dir: Option<String>,
    extensions: Option<Vec<String>>,
    output_suffix: Option<String>,
    background: Option<String>,
    png_compression: Option<i32>,
    open_after: Option<bool>,
    print: Option<PrintTable>,
    preview: Option<PreviewTable>,
    stability: Option<StabilityTable>,
    overlay: Option<OverlayTable>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PrintTable {
    short_mm: Option<f64>,
    long_mm: Option<f64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PreviewTable {
    max_px: Option<i32>,
    thumb_px: Option<i32>,
    quality: Option<i32>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StabilityTable {
    checks: Option<u32>,
    timeout_secs: Option<u64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OverlayTable {
    landscape: Option<String>,
    portrait: Option<String>,
}

/// Where the CLI looks for its config, in order: an explicit path, `$SNAPPR_CONFIG`,
/// then the XDG config directory.
///
/// XDG rather than `~/Library/Application Support`, so the file is where someone
/// editing dotfiles expects to find it and `XDG_CONFIG_HOME` is honoured. The
/// app's own `settings.json` stays in the macOS location, because nobody
/// hand-edits that one.
pub fn config_path(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(path) = explicit {
        return path;
    }
    if let Some(from_env) = std::env::var_os("SNAPPR_CONFIG") {
        return PathBuf::from(from_env);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home().join(".config"));
    base.join("snappr/config.toml")
}

/// Reads the config at `path` over the defaults.
///
/// A file that is not there is not an error. A file that will not parse *is*:
/// the operator wrote it on purpose, so silently ignoring what they typed would
/// print photos with the wrong branding and say nothing.
pub fn load(path: &Path) -> Result<PipelineConfig, Problem> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PipelineConfig::default())
        }
        Err(error) => {
            return Err(Problem::ConfigUnreadable {
                path: path.to_path_buf(),
                detail: error.to_string(),
            })
        }
    };

    let file: ConfigFile = toml::from_str(&text).map_err(|error| Problem::ConfigUnreadable {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;

    let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut config = PipelineConfig::default();

    if let Some(dir) = file.watch_dir {
        config.watch_dir = resolve(&dir, &base);
    }
    if let Some(extensions) = file.extensions {
        config.extensions = extensions
            .iter()
            .map(|ext| ext.trim().trim_start_matches('.').to_ascii_lowercase())
            .collect();
    }
    if let Some(suffix) = file.output_suffix {
        config.output_suffix = suffix;
    }
    if let Some(background) = file.background {
        config.background = background;
    }
    if let Some(compression) = file.png_compression {
        config.png_compression = compression;
    }
    if let Some(open_after) = file.open_after {
        config.open_after = open_after;
    }
    if let Some(print) = file.print {
        config.print_short_mm = print.short_mm.unwrap_or(config.print_short_mm);
        config.print_long_mm = print.long_mm.unwrap_or(config.print_long_mm);
    }
    if let Some(preview) = file.preview {
        config.preview_max_px = preview.max_px.unwrap_or(config.preview_max_px);
        config.preview_thumb_px = preview.thumb_px.unwrap_or(config.preview_thumb_px);
        config.preview_quality = preview.quality.unwrap_or(config.preview_quality);
    }
    if let Some(stability) = file.stability {
        config.stability_checks = stability.checks.unwrap_or(config.stability_checks);
        config.stability_timeout_secs =
            stability.timeout_secs.unwrap_or(config.stability_timeout_secs);
    }
    if let Some(overlay) = file.overlay {
        config.overlay_landscape = overlay.landscape.map(|path| resolve(&path, &base));
        config.overlay_portrait = overlay.portrait.map(|path| resolve(&path, &base));
    }

    config.validate()?;
    Ok(config)
}

/// `config` written out as the CLI's config file, every key spelled out.
///
/// The app's way of handing its settings to `mise run overlay`. **One-way on
/// purpose**: the app never reads this file back, for the reason in the module
/// comment. Paths are written absolute, so the file means the same thing
/// wherever it is saved, and `load` reads back exactly `config`.
pub fn to_toml(config: &PipelineConfig) -> String {
    let path = |path: &Path| path.display().to_string();
    let file = ConfigFile {
        watch_dir: Some(path(&config.watch_dir)),
        extensions: Some(config.extensions.clone()),
        output_suffix: Some(config.output_suffix.clone()),
        background: Some(config.background.clone()),
        png_compression: Some(config.png_compression),
        open_after: Some(config.open_after),
        print: Some(PrintTable {
            short_mm: Some(config.print_short_mm),
            long_mm: Some(config.print_long_mm),
        }),
        preview: Some(PreviewTable {
            max_px: Some(config.preview_max_px),
            thumb_px: Some(config.preview_thumb_px),
            quality: Some(config.preview_quality),
        }),
        stability: Some(StabilityTable {
            checks: Some(config.stability_checks),
            timeout_secs: Some(config.stability_timeout_secs),
        }),
        overlay: Some(OverlayTable {
            landscape: config.overlay_landscape.as_deref().map(path),
            portrait: config.overlay_portrait.as_deref().map(path),
        }),
    };
    toml::to_string(&file).expect("the config file shape always serialises")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_the_defaults() {
        let config = load(Path::new("/nonexistent/snappr/config.toml")).unwrap();
        assert_eq!(config, PipelineConfig::default());
    }

    #[test]
    fn defaults_are_the_documented_ones() {
        let config = PipelineConfig::default();
        assert_eq!(config.extensions, ["heic", "heif", "jpg", "jpeg"]);
        assert_eq!(config.output_suffix, ".overlay.png");
        assert_eq!(config.background, "#ffffff");
        assert_eq!((config.print_short_mm, config.print_long_mm), (100.0, 148.0));
        assert_eq!(config.preview_thumb_px, 400);
        // No branding out of the box, in either slot.
        assert!(config.overlay_landscape.is_none() && config.overlay_portrait.is_none());
    }

    #[test]
    fn print_ratio_follows_the_orientation() {
        let config = PipelineConfig::default();
        assert!((config.print_ratio(Orientation::Landscape) - 1.48).abs() < 1e-9);
        assert!((config.print_ratio(Orientation::Portrait) - 100.0 / 148.0).abs() < 1e-9);
    }

    #[test]
    fn a_square_photo_is_landscape() {
        assert_eq!(Orientation::of(2000, 2000), Orientation::Landscape);
        assert_eq!(Orientation::of(4032, 3024), Orientation::Landscape);
        assert_eq!(Orientation::of(3024, 4032), Orientation::Portrait);
    }

    #[test]
    fn background_parses_with_or_without_the_hash() {
        let mut config = PipelineConfig::default();
        assert_eq!(config.background_rgb().unwrap(), [255.0, 255.0, 255.0]);
        config.background = "00ff00".into();
        assert_eq!(config.background_rgb().unwrap(), [0.0, 255.0, 0.0]);
    }

    #[test]
    fn a_background_that_is_not_a_colour_is_reported() {
        let config = PipelineConfig { background: "not-a-colour".into(), ..Default::default() };
        let problem = config.background_rgb().unwrap_err();
        assert_eq!(problem.code(), "background_not_a_colour");
        // The value is the fact here, so it stays in the sentence.
        assert!(problem.ui().contains("not-a-colour"));
    }

    #[test]
    fn a_file_overrides_only_what_it_names() {
        let dir = std::env::temp_dir().join(format!("snappr-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "background = \"#00ff00\"\n\
             [print]\n\
             short_mm = 105\n\
             [overlay]\n\
             landscape = \"designs/event.svg\"\n",
        )
        .unwrap();

        let config = load(&path).unwrap();
        assert_eq!(config.background, "#00ff00");
        assert_eq!(config.print_short_mm, 105.0);
        // Untouched keys keep their defaults, including the other half of a table.
        assert_eq!(config.print_long_mm, 148.0);
        assert_eq!(config.png_compression, 6);
        // A relative overlay resolves against the config file, not the cwd.
        assert_eq!(config.overlay_landscape.unwrap(), dir.join("designs/event.svg"));
        assert!(config.overlay_portrait.is_none());
    }

    #[test]
    fn the_defaults_are_valid() {
        PipelineConfig::default().validate().unwrap();
    }

    fn refused(config: PipelineConfig) -> &'static str {
        config.validate().unwrap_err().code()
    }

    #[test]
    fn every_number_out_of_range_is_refused_by_its_own_code() {
        let base = PipelineConfig::default;
        assert_eq!(refused(PipelineConfig { print_short_mm: 0.0, ..base() }), "paper_size_invalid");
        assert_eq!(
            refused(PipelineConfig { print_short_mm: 150.0, ..base() }),
            "paper_size_invalid",
            "the short side has to be the short one"
        );
        assert_eq!(refused(PipelineConfig { png_compression: 10, ..base() }), "compression_out_of_range");
        assert_eq!(refused(PipelineConfig { preview_thumb_px: 0, ..base() }), "preview_size_invalid");
        assert_eq!(refused(PipelineConfig { preview_max_px: 300, ..base() }), "preview_size_invalid");
        assert_eq!(refused(PipelineConfig { preview_quality: 0, ..base() }), "preview_quality_out_of_range");
        assert_eq!(refused(PipelineConfig { stability_checks: 0, ..base() }), "stability_invalid");
        assert_eq!(refused(PipelineConfig { stability_timeout_secs: 0, ..base() }), "stability_invalid");
    }

    #[test]
    fn the_watcher_can_never_be_handed_its_own_prints() {
        let base = PipelineConfig::default;
        assert_eq!(refused(PipelineConfig { extensions: vec![], ..base() }), "no_extensions");
        assert_eq!(
            refused(PipelineConfig { extensions: vec!["jpg".into(), "PNG".into()], ..base() }),
            "png_watched"
        );
        for suffix in [".print.jpg", ".png", "x/y.png", " .overlay.png", ".overlay.png "] {
            assert_eq!(
                refused(PipelineConfig { output_suffix: suffix.into(), ..base() }),
                "output_suffix_invalid",
                "{suffix:?} should be refused"
            );
        }
        for suffix in [".snappr.png", "-print.PNG", "_branded.png"] {
            PipelineConfig { output_suffix: suffix.into(), ..base() }
                .validate()
                .unwrap_or_else(|p| panic!("{suffix:?} should be accepted: {}", p.ui()));
        }
    }

    #[test]
    fn the_exported_file_reads_back_as_the_same_config() {
        let dir = std::env::temp_dir().join(format!("snappr-config-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let config = PipelineConfig {
            watch_dir: dir.clone(),
            extensions: vec!["heic".into(), "jpg".into()],
            output_suffix: ".snappr.png".into(),
            background: "#102030".into(),
            print_short_mm: 101.6,
            print_long_mm: 152.4,
            preview_quality: 70,
            stability_timeout_secs: 45,
            open_after: false,
            overlay_landscape: Some(dir.join("event-landscape.svg")),
            overlay_portrait: None,
            ..Default::default()
        };
        std::fs::write(&path, to_toml(&config)).unwrap();

        assert_eq!(load(&path).unwrap(), config);
    }

    #[test]
    fn a_file_that_will_not_parse_is_reported_rather_than_ignored() {
        let dir = std::env::temp_dir().join(format!("snappr-config-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "background = \n").unwrap();

        let problem = load(&path).unwrap_err();
        assert_eq!(problem.code(), "config_unreadable");
    }

    #[test]
    fn the_config_path_honours_xdg_then_falls_back() {
        // The explicit path wins over everything.
        let explicit = PathBuf::from("/tmp/somewhere/config.toml");
        assert_eq!(config_path(Some(explicit.clone())), explicit);

        // And the default sits under a `snappr` directory either way.
        let path = config_path(None);
        assert!(path.ends_with("snappr/config.toml"), "{}", path.display());
    }
}
