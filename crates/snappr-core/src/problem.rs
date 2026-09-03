//! Every message snappr can show, in one table, each with both of its halves.
//!
//! A failure has two audiences and they want opposite things. The operator wants
//! a sentence: what happened, and what to do about it. A developer wants the
//! error class, the libvips text, the errno and the full path. Writing those at
//! the raise site means nothing guarantees both exist, and the version that gets
//! written is whichever audience the author had in mind - which is how
//! `Snappr::Problem: no #FF00FF photo window found in brand.svg` ended up in the
//! window once, and how `the Ruby interpreter is missing at /Users/.../bin/ruby.
//! In development, run mise run runtime first.` became the app's most-seen fatal
//! message.
//!
//! So they are declared together, one arm per problem in [`Problem::parts`]:
//!
//! - the **operator half** reaches the window and the terminal. No class names,
//!   no hex literals, no repo jargon, no full paths unless the path *is* the
//!   fact. One or two sentences: what happened, then what to do.
//! - the **console half** is optional and gets appended in brackets. Paths,
//!   ratios, errnos, the underlying error's own text.
//!
//! `parts` returns both from a single `match`, so the two halves of one problem
//! sit on adjacent lines and the compiler will not let a new variant be added
//! with only one of them. There is deliberately no accessor that hands a caller
//! the console half for display.
//!
//! **This table used to be two tables** - `lib/snappr/problem.rb` and
//! `src-tauri/src/problem.rs` - with six problems stated twice in slightly
//! different words. It is one now, and the rule that made the split survivable is
//! the rule that makes the merge safe: [`Problem::code`] is stable across
//! rewordings, so the tests and the frontend assert on codes and the wording is
//! free to change.

use std::fmt::Display;
use std::path::{Path, PathBuf};

/// How loud a problem is. An error means the thing the operator asked for did
/// not happen; a warning means it happened, differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Level {
    Error,
    Warning,
}

/// A problem on its way to the frontend: the operator's half plus the catalogue
/// code, so the UI can branch on the kind of failure instead of only printing it.
///
/// The console half is not here, and that is the point.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Payload {
    /// The catalogue code, stable across rewordings - so the frontend may branch
    /// on it, which it could not do when every command returned a bare string.
    pub code: String,
    pub message: String,
}

/// One degradation: something the pipeline carried on past, worth telling the
/// operator about. Travels in a result's `warnings`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Warning {
    /// The catalogue code. Named `kind` because that is what it was on the wire
    /// and what the frontend already reads.
    ///
    /// A `String` rather than the `&'static str` [`Problem::code`] returns, so a
    /// warning can be carried inside a deserialisable result - the photo index
    /// persists these across a restart.
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub enum Problem {
    // --- The overlays. Everything an operator is most likely to actually hit,
    // because these are the one part of the pipeline they author themselves.
    OverlayWindowMissing { slot: String, file: String, path: PathBuf },
    OverlayFileMissing { slot: String, file: String, path: PathBuf },
    OverlayUnreadable { slot: String, file: String, path: PathBuf },
    OverlayNoSize { slot: String, file: String, path: PathBuf },
    /// Distinct from [`Problem::OverlayNoSize`]: the file said its size and the
    /// size was unusable, so there is nothing for the operator to add - only a
    /// redraw fixes it. Only the panel's preview hits this; the pipeline never
    /// asks an overlay for its own shape, it stretches it to the paper's.
    OverlayNoShape { file: String, path: PathBuf },
    OverlayMissing { slot: String, file: String, path: PathBuf, orientation: &'static str },
    AspectMismatch {
        slot: String,
        file: String,
        width: i32,
        height: i32,
        drawn: f64,
        wanted: f64,
        orientation: &'static str,
    },
    MarkerSurvived { slot: String, file: String, path: PathBuf },

    // --- The photo itself.
    PhotoMissing { file: String, path: PathBuf },
    TransferStalled { file: String, path: PathBuf, secs: u64 },
    /// The catch-all for anything snappr did not raise itself. A libvips error
    /// never shows its own text; it borrows this sentence and keeps the original
    /// for the log.
    PhotoUnusable { detail: String },
    ProfileUnusable { detail: String },
    OpenFailed { file: String, path: PathBuf },

    // --- The image tools. One variant where there used to be seven, because
    // there is no longer a child process to be starting, unreachable, wedged or
    // timed out: either libvips is here or the install is broken.
    ImageToolsUnavailable { detail: String },

    // --- Settings, the watch folder, and the photo list.
    NotAFolder { path: PathBuf },
    CannotWatch { path: PathBuf, detail: String },
    BackgroundNotAColour { value: String },
    ConfigUnreadable { path: PathBuf, detail: String },
    SettingsUnreadable { path: PathBuf, detail: String },
    CannotSaveSettings { detail: String },
    CannotRememberPhotos { detail: String },
    OverlayFileGone { file: String, path: PathBuf },

    // --- The overlay panel. Adding, removing and drawing the cards; what each
    // overlay then *does* to a photo is the pipeline's half of the table above.
    OverlayNotADrawing { file: String, path: PathBuf },
    OverlayAlreadyAdded { file: String },
    CannotDrawOverlays { detail: String },

    // --- Acting on one photo, from the window.
    PhotoForgotten { path: PathBuf },
    PhotoStillWorking,
    PrintFileGone,
    PrintUnreadable { path: PathBuf },
    PaperFallback,
    FinderWontOpen { detail: String },
    WindowNotReady { what: &'static str, detail: String },

    // --- A file dropped on the window or the Dock icon and refused. Each of
    // these has a different fix, which is why they are not one message.
    DropAlreadyAPrint { file: String },
    DropIsAFolder { file: String },
    DropStillDownloading { file: String },
    DropWrongKind { file: String, kind: String, accepted: String },
    DropUnreadableName { path: PathBuf },
    DropNotAFile { url: String },

    // --- The command line. `snappr help` prints these, so they are the one place
    // a shell-style lower-case phrasing would be at home - and they are still
    // sentences, because the person typing the command is the operator.
    UnknownCommand { command: String },
    NoPathsGiven,
}

impl Problem {
    /// Both halves of every problem, one arm each. The single place either half
    /// is written.
    fn parts(&self) -> (String, Option<String>) {
        use Problem::*;
        match self {
            OverlayWindowMissing { slot, file, path } => (
                format!(
                    "The {slot} {file} does not show where the photo goes. Open it in your \
                     design tool and add a magenta rectangle over the area the photo should \
                     fill, then export it again over the same file."
                ),
                Some(format!("no marker pixels in {}", path.display())),
            ),
            OverlayFileMissing { slot, file, path } => (
                format!(
                    "The {slot} {file} is no longer where it was. Add it again in the \
                     overlay panel, or put the file back."
                ),
                Some(format!("{} does not exist", path.display())),
            ),
            OverlayUnreadable { slot, file, path } => (
                format!(
                    "The {slot} {file} is empty, or is not a drawing Snappr can read. \
                     Export it again from your design tool."
                ),
                Some(format!("{} parsed but has no root element", path.display())),
            ),
            OverlayNoSize { slot, file, path } => (
                format!(
                    "The {slot} {file} does not say what size it was drawn at, so Snappr \
                     cannot scale it. Export it again from your design tool - most tools \
                     write the size automatically."
                ),
                Some(format!("{} has no view box and no absolute width/height", path.display())),
            ),
            OverlayNoShape { file, path } => (
                format!(
                    "The overlay {file} does not say what shape it was drawn at, so its \
                     preview is being shown at the paper's shape. Export it again from \
                     your design tool."
                ),
                Some(format!("{} has a zero or negative height", path.display())),
            ),
            OverlayMissing { slot, file, path, orientation } => (
                format!(
                    "The {slot} {file} is missing, so {orientation} photos are printing \
                     with no branding. Add it again in the overlay panel."
                ),
                Some(format!("{} is configured but not on disk", path.display())),
            ),
            AspectMismatch { slot, file, width, height, drawn, wanted, orientation } => (
                format!(
                    "The {slot} {file} is not the shape of the paper, so the branding is \
                     being stretched. Redraw it at {width} x {height} px, or any size with \
                     the same shape."
                ),
                Some(format!("drawn at {drawn:.3}:1, {orientation} prints at {wanted:.3}:1")),
            ),
            MarkerSurvived { slot, file, path } => (
                format!(
                    "The photo area in the {slot} {file} could not be removed cleanly, so \
                     Snappr hid it instead. In your design tool, give that magenta shape a \
                     plain fill colour."
                ),
                Some(format!(
                    "marker pixels survived the element walk in {}, keyed out on alpha",
                    path.display()
                )),
            ),

            PhotoMissing { file, path } => (
                format!(
                    "The photo {file} is not there any more - it looks like it was moved or \
                     deleted."
                ),
                Some(format!("{} is not a file", path.display())),
            ),
            TransferStalled { file, path, secs } => (
                format!("{file} never finished arriving, so Snappr skipped it. AirDrop it again."),
                Some(format!("size and mtime never settled within {secs}s: {}", path.display())),
            ),
            PhotoUnusable { detail } => (
                "Snappr could not make a print from this photo. It may be damaged, or in \
                 a format Snappr cannot read."
                    .into(),
                Some(detail.clone()),
            ),
            ProfileUnusable { detail } => (
                "Snappr could not match the branding's colours to this photo, so they may \
                 print slightly off."
                    .into(),
                Some(detail.clone()),
            ),
            OpenFailed { file, path } => (
                format!("Snappr could not open {file}."),
                Some(format!("open(1) failed for {}", path.display())),
            ),

            // Never names libvips, and never names a shell command: the operator
            // half has to read the same in a shipped app as on a developer's
            // machine, and "reinstall" is the only advice that is true in both.
            ImageToolsUnavailable { detail } => (
                "Snappr's image tools are not working. Reinstall Snappr.".into(),
                Some(detail.clone()),
            ),

            // The path *is* the fact in both of these: the operator picked this
            // folder and has to go and look at it. That is the rule - a path
            // appears when the operator has to find it, not as decoration on a
            // sentence that already reads without it.
            NotAFolder { path } => (
                format!("{} is not a folder, so Snappr cannot watch it.", name_of(path)),
                Some(path.display().to_string()),
            ),
            CannotWatch { path, detail } => (
                format!(
                    "Snappr cannot watch {}. Check the folder still exists and that Snappr \
                     is allowed to read it.",
                    name_of(path)
                ),
                Some(format!("{}: {detail}", path.display())),
            ),
            BackgroundNotAColour { value } => (
                format!(
                    "Snappr's background setting is not a colour: {value}. It should look \
                     like #ffffff."
                ),
                None,
            ),
            ConfigUnreadable { path, detail } => (
                format!(
                    "Snappr could not read its settings file, so it is using the defaults. \
                     Check {}.",
                    path.display()
                ),
                Some(detail.clone()),
            ),
            SettingsUnreadable { path, detail } => (
                "Snappr could not read your saved settings and has started with the \
                 defaults. Pick your watched folder and your overlays again."
                    .into(),
                Some(format!("{}: {detail}", path.display())),
            ),
            CannotSaveSettings { detail } => {
                ("Snappr could not save your settings.".into(), Some(detail.clone()))
            }
            CannotRememberPhotos { detail } => (
                "Snappr could not remember which photos it has seen.".into(),
                Some(detail.clone()),
            ),
            OverlayFileGone { file, path } => (
                format!("The overlay {file} is not there any more. Add it again."),
                Some(path.display().to_string()),
            ),

            // "drawing" rather than "SVG": the operator picked a file in Finder and
            // the thing that is wrong with it is that it is not artwork. The
            // extension belongs in the console half, where it is diagnostic.
            OverlayNotADrawing { file, path } => (
                format!(
                    "{file} is not a drawing Snappr can use as an overlay. Export the \
                     design from your tool as an SVG and add that instead."
                ),
                Some(path.display().to_string()),
            ),
            OverlayAlreadyAdded { file } => {
                (format!("{file} is already in the overlay panel."), None)
            }
            CannotDrawOverlays { detail } => (
                "Snappr could not draw the overlay pictures. The overlays themselves \
                 still work."
                    .into(),
                Some(detail.clone()),
            ),

            PhotoForgotten { path } => (
                "Snappr has lost track of that photo. Drop it on the window again.".into(),
                Some(path.display().to_string()),
            ),
            PhotoStillWorking => {
                ("That photo is still being worked on. Try again in a moment.".into(), None)
            }
            PrintFileGone => (
                "The file for printing is gone. Press Try again to make it once more.".into(),
                None,
            ),
            PrintUnreadable { path } => (
                "Snappr could not open the file for printing. Press Try again to make it \
                 once more."
                    .into(),
                Some(format!("the print could not be read from {}", path.display())),
            ),
            // Deliberately does not name `print_paper_name`: it is a PPD keyword
            // and pure jargon. The advice is actionable right now, because the
            // print panel is already showing Paper Size.
            PaperFallback => (
                "This printer does not have the photo paper Snappr expects, so the print \
                 may come out with a white border. Check Paper Size in the print dialog."
                    .into(),
                None,
            ),
            FinderWontOpen { detail } => {
                ("Snappr could not open Finder.".into(), Some(detail.clone()))
            }
            WindowNotReady { what, detail } => {
                (format!("Snappr could not open the {what}. Try again."), Some(detail.clone()))
            }

            DropAlreadyAPrint { file } => (
                format!("{file} is a print Snappr already made. Drop the original photo instead."),
                None,
            ),
            DropIsAFolder { file } => (
                format!(
                    "{file} is a folder. Drop the photos inside it, or point the watched \
                     folder at it."
                ),
                None,
            ),
            DropStillDownloading { file } => (
                format!("{file} is still downloading. Wait for it to finish, then drop it again."),
                None,
            ),
            DropWrongKind { file, kind, accepted } => {
                (format!("{file} is {kind}. Snappr prints {accepted} photos."), None)
            }
            DropUnreadableName { path } => (
                "Snappr could not read that file's name.".into(),
                Some(path.display().to_string()),
            ),
            DropNotAFile { url } => {
                ("Snappr can only open files, not links.".into(), Some(url.clone()))
            }

            UnknownCommand { command } => {
                (format!("Snappr has no command called \"{command}\"."), None)
            }
            NoPathsGiven => (
                "Tell Snappr which photos to overlay, for example:\n  \
                 snappr overlay ~/Downloads/IMG_1234.heic"
                    .into(),
                None,
            ),
        }
    }

    /// The catalogue key. Stable across rewordings, which is what makes it the
    /// thing to assert on, the thing to log, and the thing the frontend may
    /// branch on.
    ///
    /// These spellings are the ones the Ruby pipeline put on the wire as a
    /// warning's `kind`, so anything already reading them keeps working.
    pub fn code(&self) -> &'static str {
        use Problem::*;
        match self {
            OverlayWindowMissing { .. } => "overlay_window_missing",
            OverlayFileMissing { .. } => "overlay_file_missing",
            OverlayUnreadable { .. } => "overlay_unreadable",
            OverlayNoSize { .. } => "overlay_no_size",
            OverlayNoShape { .. } => "overlay_no_shape",
            OverlayMissing { .. } => "overlay_missing",
            AspectMismatch { .. } => "aspect_mismatch",
            MarkerSurvived { .. } => "marker_survived",
            PhotoMissing { .. } => "photo_missing",
            TransferStalled { .. } => "transfer_stalled",
            PhotoUnusable { .. } => "photo_unusable",
            ProfileUnusable { .. } => "profile_unusable",
            OpenFailed { .. } => "open_failed",
            ImageToolsUnavailable { .. } => "image_tools_unavailable",
            NotAFolder { .. } => "not_a_folder",
            CannotWatch { .. } => "cannot_watch",
            BackgroundNotAColour { .. } => "background_not_a_colour",
            ConfigUnreadable { .. } => "config_unreadable",
            SettingsUnreadable { .. } => "settings_unreadable",
            CannotSaveSettings { .. } => "cannot_save_settings",
            CannotRememberPhotos { .. } => "cannot_remember_photos",
            OverlayFileGone { .. } => "overlay_file_gone",
            OverlayNotADrawing { .. } => "overlay_not_a_drawing",
            OverlayAlreadyAdded { .. } => "overlay_already_added",
            CannotDrawOverlays { .. } => "cannot_draw_overlays",
            PhotoForgotten { .. } => "photo_forgotten",
            PhotoStillWorking => "photo_still_working",
            PrintFileGone => "print_file_gone",
            PrintUnreadable { .. } => "print_unreadable",
            PaperFallback => "paper_fallback",
            FinderWontOpen { .. } => "finder_wont_open",
            WindowNotReady { .. } => "window_not_ready",
            DropAlreadyAPrint { .. } => "drop_already_a_print",
            DropIsAFolder { .. } => "drop_is_a_folder",
            DropStillDownloading { .. } => "drop_still_downloading",
            DropWrongKind { .. } => "drop_wrong_kind",
            DropUnreadableName { .. } => "drop_unreadable_name",
            DropNotAFile { .. } => "drop_not_a_file",
            UnknownCommand { .. } => "unknown_command",
            NoPathsGiven => "no_paths_given",
        }
    }

    /// What the operator reads.
    pub fn ui(&self) -> String {
        self.parts().0
    }

    /// What goes to the log: the operator's sentence, then the console half in
    /// brackets. Both halves, always in the same order, so a log is readable by
    /// whoever is holding the printer and still diagnosable later.
    pub fn log_line(&self) -> String {
        let (ui, detail) = self.parts();
        match detail {
            Some(detail) => format!("{ui} [{detail}]"),
            None => ui,
        }
    }

    /// The frontend's view: the operator's half plus the code.
    pub fn payload(&self) -> Payload {
        Payload { code: self.code().to_string(), message: self.ui() }
    }

    /// Logs both halves and hands back the operator's, for returning in an `Err`.
    /// The console half cannot escape any other way.
    pub fn say(self) -> Payload {
        log::error!("{}", self.log_line());
        self.payload()
    }

    /// Same, for a problem worth saying out loud that did not stop anything.
    pub fn warn(self) -> Warning {
        log::warn!("{}", self.log_line());
        Warning { kind: self.code().to_string(), message: self.ui() }
    }
}

impl Display for Problem {
    /// The operator's half, so `{}` on a Problem can never leak the console half.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.ui())
    }
}

impl Display for Payload {
    /// Same rule: the sentence, never the code and never the console half.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Problem {}

/// Reports a degradation into a collector, in the shape the pipeline puts on the
/// wire. This is the whole of "a warning" - the pipeline carries on.
///
/// Takes an `Option` because plenty of callers have nowhere to put one and should
/// not have to invent a collector to report it; the log still gets both halves.
pub fn warn_into(warnings: Option<&mut Vec<Warning>>, problem: Problem) {
    let warning = problem.warn();
    if let Some(warnings) = warnings {
        warnings.push(warning);
    }
}

/// The last path segment, which is what an operator recognises. Falls back to the
/// whole path when there is no segment to take.
pub fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Turns any error into the console half of a problem, for the `detail` fields
/// above. Exists so call sites read `detail(e)` rather than `e.to_string()`.
pub fn detail(error: impl Display) -> String {
    error.to_string()
}
