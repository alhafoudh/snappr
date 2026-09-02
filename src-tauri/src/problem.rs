//! Every message the shell can show, in one table, each with both of its halves.
//!
//! The Rust twin of `lib/snappr/problem.rb`, and it exists for the same reason.
//! A failure has two audiences that want opposite things: the operator wants a
//! sentence saying what happened and what to do, while a developer wants the
//! errno, the path and the class. Written at the throw site, only one of the two
//! ever gets written - which is how `the Ruby interpreter is missing at
//! /Users/.../build/ruby-runtime/bin/ruby. In development, run `mise run
//! runtime` first.` ended up being the app's most-seen fatal message.
//!
//! So they are declared together, one arm per problem, in [`Problem::parts`]:
//!
//! - the **operator half** is what reaches the window. No paths unless the path
//!   is the fact, no errnos, no "sidecar", no backticked shell commands.
//! - the **console half** is optional and gets appended in brackets. Paths,
//!   errnos, the underlying error's own text.
//!
//! `parts` is a single match returning both, so the two halves of one problem sit
//! on adjacent lines and the compiler will not let a new variant be added with
//! only one of them.
//!
//! The only ways out are [`Problem::say`] and [`Problem::notify`], both of which
//! log the console half on the way. There is deliberately no accessor that hands
//! a caller the console half for display.

use std::fmt::Display;
use std::path::Path;

use tauri::{AppHandle, Emitter};

/// How loud a problem is. An error means the thing the operator asked for did
/// not happen; a warning means it happened, differently.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warning,
}

/// A transient message about something the operator just did. Carries only the
/// operator's half - the console half was already logged.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Notice {
    pub level: Level,
    pub text: String,
}

pub enum Problem<'a> {
    // --- Startup. All of these end up in the red Stage callout, which already
    // carries the `mise run runtime` hint as its own second line - so none of
    // them repeats it.
    PipelineNotInstalled { what: &'a str, path: &'a Path },
    PipelineFilesLost { detail: String },
    PipelineWontStart { detail: String },
    PipelineUnreachable { what: &'a str },
    PipelineNotResponding { detail: String },
    PipelineTimedOut { secs: u64 },
    PipelineStarting,

    // --- Watching the drop folder.
    NotAFolder { path: &'a Path },
    CannotWatch { path: &'a Path, detail: String },
    TransferStalled { file: String, path: &'a Path, secs: u64 },

    // --- Settings and the photo list, both of which live in snappr's own
    // support folder rather than anywhere the operator put them.
    SettingsUnreadable { path: &'a Path, detail: String },
    CannotSaveSettings { detail: String },
    CannotRememberPhotos { detail: String },
    OverlayFileGone { file: String, path: &'a Path },

    // --- The overlay panel. Adding, removing and drawing the cards; what each
    // overlay then *does* to a photo is the pipeline's half of the catalogue.
    OverlayNotADrawing { file: String, path: &'a Path },
    OverlayAlreadyAdded { file: String },
    CannotDrawOverlays { detail: String },

    // --- Acting on one photo.
    PhotoForgotten { path: &'a Path },
    PhotoStillWorking,
    PrintFileGone,
    PrintUnreadable { path: &'a Path },
    PaperFallback,
    FinderWontOpen { detail: String },
    WindowNotReady { what: &'a str, detail: String },

    // --- A file dropped on the window or the Dock icon and refused. Each of
    // these has a different fix, which is why they are not one message.
    DropAlreadyAPrint { file: String },
    DropIsAFolder { file: String },
    DropStillDownloading { file: String },
    DropWrongKind { file: String, kind: String, accepted: String },
    DropUnreadableName { path: &'a Path },
    DropNotAFile { url: String },
}

impl Problem<'_> {
    /// Both halves of every problem, one arm each. The single place either half
    /// is written.
    fn parts(&self) -> (String, Option<String>) {
        use Problem::*;
        match self {
            PipelineNotInstalled { what, path } => (
                "Snappr's image tools are not installed.".into(),
                Some(format!("{what} is missing at {}", path.display())),
            ),
            PipelineFilesLost { detail } => (
                "Snappr cannot find its own program files.".into(),
                Some(detail.clone()),
            ),
            PipelineWontStart { detail } => (
                "Snappr's image tools did not start.".into(),
                Some(detail.clone()),
            ),
            PipelineUnreachable { what } => (
                "Snappr cannot talk to its image tools.".into(),
                Some(format!("the sidecar has no {what}")),
            ),
            PipelineNotResponding { detail } => (
                "Snappr's image tools have stopped responding. Restart snappr.".into(),
                Some(detail.clone()),
            ),
            PipelineTimedOut { secs } => (
                "Snappr's image tools took too long on this photo and gave up.".into(),
                Some(format!("no response within {secs}s")),
            ),
            PipelineStarting => (
                "Snappr is still starting up. Try this photo again in a moment.".into(),
                Some("no sidecar installed yet".into()),
            ),

            // The path is the fact in both of these: the operator picked this
            // folder and has to go and look at it.
            NotAFolder { path } => (
                format!("{} is not a folder, so snappr cannot watch it.", name_of(path)),
                Some(path.display().to_string()),
            ),
            CannotWatch { path, detail } => (
                format!(
                    "Snappr cannot watch {}. Check the folder still exists and that snappr \
                     is allowed to read it.",
                    name_of(path)
                ),
                Some(format!("{}: {detail}", path.display())),
            ),
            TransferStalled { file, path, secs } => (
                format!("{file} never finished arriving, so snappr skipped it. AirDrop it again."),
                Some(format!("size and mtime never settled within {secs}s: {}", path.display())),
            ),

            SettingsUnreadable { path, detail } => (
                "Snappr could not read your saved settings and has started with the \
                 defaults. Pick your watched folder and your overlays again."
                    .into(),
                Some(format!("{}: {detail}", path.display())),
            ),
            CannotSaveSettings { detail } => (
                "Snappr could not save your settings.".into(),
                Some(detail.clone()),
            ),
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
                    "{file} is not a drawing snappr can use as an overlay. Export the \
                     design from your tool as an SVG and add that instead."
                ),
                Some(path.display().to_string()),
            ),
            OverlayAlreadyAdded { file } => (
                format!("{file} is already in the overlay panel."),
                None,
            ),
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
            PhotoStillWorking => (
                "That photo is still being worked on. Try again in a moment.".into(),
                None,
            ),
            PrintFileGone => (
                "The file for printing is gone. Press Try again to make it once more.".into(),
                None,
            ),
            PrintUnreadable { path } => (
                "Snappr could not open the file for printing. Press Try again to make it \
                 once more."
                    .into(),
                Some(format!("NSImage could not read {}", path.display())),
            ),
            // Deliberately does not name `print_paper_name`: it is a PPD keyword
            // and pure jargon. The advice is actionable right now, because the
            // print panel is already showing Paper Size.
            PaperFallback => (
                "This printer does not have the photo paper snappr expects, so the print \
                 may come out with a white border. Check Paper Size in the print dialog."
                    .into(),
                None,
            ),
            FinderWontOpen { detail } => (
                "Snappr could not open Finder.".into(),
                Some(detail.clone()),
            ),
            WindowNotReady { what, detail } => (
                format!("Snappr could not open the {what}. Try again."),
                Some(detail.clone()),
            ),

            DropAlreadyAPrint { file } => (
                format!("{file} is a print snappr already made. Drop the original photo instead."),
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
            DropWrongKind { file, kind, accepted } => (
                format!("{file} is {kind}. Snappr prints {accepted} photos."),
                None,
            ),
            DropUnreadableName { path } => (
                "Snappr could not read that file's name.".into(),
                Some(path.display().to_string()),
            ),
            DropNotAFile { url } => (
                "Snappr can only open files, not links.".into(),
                Some(url.clone()),
            ),
        }
    }

    /// What the operator reads.
    pub fn ui(&self) -> String {
        self.parts().0
    }

    /// What goes to the log: the operator's sentence, then the detail in
    /// brackets. Both halves, always in the same order.
    pub fn log_line(&self) -> String {
        let (ui, detail) = self.parts();
        match detail {
            Some(detail) => format!("{ui} [{detail}]"),
            None => ui,
        }
    }

    /// Logs both halves and hands back the operator's, for returning in an `Err`
    /// that the frontend will display. The detail cannot escape any other way.
    pub fn say(self) -> String {
        log::error!("{}", self.log_line());
        self.ui()
    }

    /// Same, for a problem that is worth saying out loud but did not stop
    /// anything.
    pub fn warn(self) -> String {
        log::warn!("{}", self.log_line());
        self.ui()
    }

    /// Logs both halves and pushes the operator's to the window as a `notice`
    /// event. For an outcome discovered *after* the command that caused it has
    /// already returned - which today means inside `mac::print`'s main-thread
    /// closure and nowhere else. Anything a command can discover before it
    /// returns belongs in its `Err`.
    pub fn notify(self, app: &AppHandle, level: Level) {
        match level {
            Level::Error => log::error!("{}", self.log_line()),
            Level::Warning => log::warn!("{}", self.log_line()),
        }
        let _ = app.emit("notice", Notice { level, text: self.ui() });
    }
}

/// The last path segment, which is what an operator recognises. Falls back to
/// the whole path when there is no segment to take.
pub fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Turns any error into the console half of a problem, for the `detail` fields
/// above. Exists so call sites read `.detail(e)` rather than `e.to_string()`.
pub fn detail(error: impl Display) -> String {
    error.to_string()
}
