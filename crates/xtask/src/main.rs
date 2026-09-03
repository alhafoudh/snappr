//! snappr's build tasks.
//!
//! Three stages, in this order, because each consumes the last:
//!
//! ```text
//! cargo xtask libvips    build libvips from source into build/libvips-prefix
//! cargo xtask closure    collect its dylib closure into build/vips, relocated
//! cargo xtask verify     run the built app's own CLI and prove nothing came from Homebrew
//! cargo xtask runtime    all three
//! ```
//!
//! **Why any of this exists**: Homebrew's libvips loads its format support -
//! including HEIC - from a module directory baked in at compile time, so a copied
//! Homebrew libvips silently cannot decode an iPhone photo. libvips is therefore
//! built here with `-Dmodules=disabled`, which links the loaders in.
//!
//! These were Ruby scripts. They are Rust because the project relies on Rust and
//! nothing else, and because `verify` now drives the same binary the app ships.

mod closure;
mod libvips;
mod verify;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let root = workspace_root();
    let task = std::env::args().nth(1).unwrap_or_default();
    let force = std::env::args().any(|arg| arg == "--force");

    let result = match task.as_str() {
        "libvips" => libvips::run(&root, force),
        "closure" => closure::run(&root, force),
        "verify" => verify::run(&root),
        "runtime" => libvips::run(&root, force)
            .and_then(|()| closure::run(&root, force))
            .and_then(|()| verify::run(&root)),
        other => {
            if !other.is_empty() {
                eprintln!("xtask: no task called {other:?}");
            }
            eprintln!(
                "usage: cargo xtask <libvips|closure|verify|runtime> [--force]\n\n  \
                 libvips  build libvips from source into build/libvips-prefix\n  \
                 closure  collect and relocate its dylibs into build/vips\n  \
                 verify   prove the bundle carries its own libvips\n  \
                 runtime  all three, in order"
            );
            return ExitCode::FAILURE;
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            eprintln!("xtask: {problem}");
            ExitCode::FAILURE
        }
    }
}

/// The repo root, from this crate's location rather than the working directory,
/// so a task behaves the same wherever it is invoked from.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/xtask sits two levels under the root")
        .to_path_buf()
}

/// Runs a command, failing with its name when it does.
fn sh(command: &mut Command) -> Result<(), String> {
    let shown = format!("{command:?}");
    let status = command.status().map_err(|error| format!("{shown}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("command failed ({status}): {shown}"))
    }
}

/// A command's stdout, or `None` if it failed. For the tools whose *absence* or
/// failure is information rather than an error - `otool` on a file that is not a
/// Mach-O, say.
fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string()
}

fn human(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

fn say(stage: &str, message: impl AsRef<str>) {
    println!("{stage}: {}", message.as_ref());
}
