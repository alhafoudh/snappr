//! Operator settings, and the slice of them the Ruby side is told about.
//!
//! Plain serde into `app_config_dir()` rather than tauri-plugin-store: the Rust
//! shell owns settings, the fs plugin's Rust side offers only scope methods anyway,
//! and this way the file is human-readable JSON with no IPC round-trip and no
//! capability to grant.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

fn default_watch_dir() -> PathBuf {
    dirs_home().join("Downloads")
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
}

fn default_extensions() -> Vec<String> {
    ["heic", "heif", "jpg", "jpeg"].map(String::from).to_vec()
}

fn default_output_suffix() -> String {
    ".overlay.png".into()
}

fn default_background() -> String {
    "#ffffff".into()
}

fn default_png_compression() -> u8 {
    6
}

fn default_short_mm() -> f64 {
    100.0
}

fn default_long_mm() -> f64 {
    148.0
}

fn default_preview_max_px() -> u32 {
    2000
}

fn default_preview_thumb_px() -> u32 {
    400
}

fn default_preview_quality() -> u8 {
    80
}

fn default_stability_checks() -> u32 {
    2
}

fn default_stability_timeout_secs() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub watch_dir: PathBuf,
    pub extensions: Vec<String>,
    /// Dual-purpose: it names the output file *and* is the watcher's self-output
    /// guard, so it has to match what the Ruby side appends.
    pub output_suffix: String,
    pub background: String,
    pub png_compression: u8,
    pub print_short_mm: f64,
    pub print_long_mm: f64,
    pub preview_max_px: u32,
    pub preview_thumb_px: u32,
    pub preview_quality: u8,
    pub stability_checks: u32,
    pub stability_timeout_secs: u64,
    /// Empty means the system default printer.
    pub printer: String,
    /// The driver's PPD keyword for the paper to print on - NOT the label the
    /// dialog shows. `Postcard.Fullbleed` is what the Canon SELPHY calls the paper
    /// macOS displays as "Postcard Borderless"; plain `Postcard` has the same
    /// dimensions but ~3 mm margins. Empty means let the driver pick, which on the
    /// SELPHY is the bordered one.
    pub print_paper_name: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            watch_dir: default_watch_dir(),
            extensions: default_extensions(),
            output_suffix: default_output_suffix(),
            background: default_background(),
            png_compression: default_png_compression(),
            print_short_mm: default_short_mm(),
            print_long_mm: default_long_mm(),
            preview_max_px: default_preview_max_px(),
            preview_thumb_px: default_preview_thumb_px(),
            preview_quality: default_preview_quality(),
            stability_checks: default_stability_checks(),
            stability_timeout_secs: default_stability_timeout_secs(),
            printer: String::new(),
            print_paper_name: "Postcard.Fullbleed".into(),
        }
    }
}

impl Settings {
    pub fn stability_timeout(&self) -> Duration {
        Duration::from_secs(self.stability_timeout_secs)
    }

    pub fn path(app: &AppHandle) -> Result<PathBuf, String> {
        let dir = app
            .path()
            .app_config_dir()
            .map_err(|e| format!("cannot resolve the config directory: {e}"))?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        Ok(dir.join("settings.json"))
    }

    /// A missing or unreadable file is not an error: it just means defaults. A
    /// corrupt one is worth saying out loud, but still not worth refusing to start
    /// over - a kiosk that will not open because of a stray character is worse.
    pub fn load(app: &AppHandle) -> Self {
        let Ok(path) = Self::path(app) else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
                log::warn!("{} is not readable ({e}), falling back to defaults", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, app: &AppHandle) -> Result<(), String> {
        let path = Self::path(app)?;
        let raw = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, raw).map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    /// The seven keys the pipeline actually reads. Deliberately not the whole
    /// struct: the watch settings belong to this side now, and sending them would
    /// only create a second place for them to drift.
    pub fn pipeline_config(&self) -> Value {
        json!({
            "print": { "short_mm": self.print_short_mm, "long_mm": self.print_long_mm },
            "output_suffix": self.output_suffix,
            "background": self.background,
            "png_compression": self.png_compression,
            "preview": {
                "max_px": self.preview_max_px,
                "thumb_px": self.preview_thumb_px,
                "quality": self.preview_quality,
            },
        })
    }
}
