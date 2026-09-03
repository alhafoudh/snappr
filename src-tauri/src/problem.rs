//! The shell's access to the message catalogue, which lives in `snappr-core`.
//!
//! **There used to be two catalogues** - one here and one in
//! `lib/snappr/problem.rb` - with six problems stated twice in slightly different
//! words, kept in step by comments pointing at each other. There is one now, in
//! `snappr_core::problem`, and this file adds only the part that cannot live
//! there: delivering a message to the window, which needs a Tauri `AppHandle`.
//!
//! The rules the catalogue is built on are unchanged and are asserted in core's
//! own tests: two halves per problem declared together, the operator's half
//! carrying no jargon and no paths unless the path is the fact, and a stable
//! `code` so tests and the frontend never depend on wording.

use tauri::{AppHandle, Emitter};

pub use snappr_core::problem::{detail, name_of, Level, Payload, Problem};

/// A transient message about something the operator just did. Carries only the
/// operator's half - the console half was already logged.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct Notice {
    pub level: Level,
    pub text: String,
}

/// Sending a problem to the window.
pub trait Notify {
    /// Logs both halves and pushes the operator's to the window as a `notice`
    /// event.
    ///
    /// For an outcome discovered *after* the command that caused it has already
    /// returned - which today means inside `mac::print`'s main-thread closure and
    /// nowhere else. Anything a command can discover before it returns belongs in
    /// its `Err`.
    fn notify(self, app: &AppHandle, level: Level);
}

impl Notify for Problem {
    fn notify(self, app: &AppHandle, level: Level) {
        match level {
            Level::Error => log::error!("{}", self.log_line()),
            Level::Warning => log::warn!("{}", self.log_line()),
        }
        let _ = app.emit("notice", Notice { level, text: self.ui() });
    }
}
