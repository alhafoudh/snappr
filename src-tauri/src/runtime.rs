//! Where the bundled Ruby, libvips and gem tree live, and the environment the
//! sidecar needs to find them.
//!
//! Two layouts, because the app runs from two very different places.
//!
//! In development there is no `.app` at all: `bin/build-runtime`, `bin/build-vips`
//! and `bin/build-gems` leave everything under `build/` in the project, and the
//! Ruby sources are read straight from the working tree so editing `lib/snappr/*`
//! and restarting is enough.
//!
//! In the bundle the pieces are split by what macOS will codesign. Tauri signs
//! nested code only in `MacOS`, `Frameworks`, `Plugins`, `Helpers`, `XPCServices`
//! and `Libraries` - notably **not** `Resources` - so every Mach-O file has to land
//! in one of those, and only inert data may go in `Resources`:
//!
//! ```text
//! Snappr.app/Contents/
//!   MacOS/ruby            the interpreter          (externalBin)      PLANNED
//!   Frameworks/*.dylib    libvips and its closure  (macOS.frameworks) PLANNED
//!   lib/ruby/             the Ruby stdlib          (macOS.files)      PLANNED
//!   Resources/snappr/     lib, config, gems        (resources)        PLANNED
//!   Resources/sample/     the sample overlay       (resources)        WIRED
//! ```
//!
//! **Only the last line is real.** `bundle.resources` names `sample/` and nothing
//! else, so `Resources/snappr` never exists, `detect` always takes the development
//! branch, and `in_bundle` below has never run. Do not plan against it as if it
//! had. Note `share/` is deliberately absent from that list even as intent: it is
//! test material and must never ship.
//!
//! The sample is looked up separately, through `BaseDirectory::Resource` in
//! `settings.rs`, precisely so it does not depend on any of the PLANNED lines - an
//! app whose Ruby runtime is not built can still reveal an SVG.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

use crate::problem::{detail, Problem};

#[derive(Debug, Clone)]
pub struct Runtime {
    pub ruby: PathBuf,
    /// The directory holding libvips and everything it links.
    pub vips: PathBuf,
    /// `bundler/setup.rb` from `bundle install --standalone`, which puts the
    /// vendored gems on `$LOAD_PATH` without needing bundler or rubygems.
    pub gem_setup: PathBuf,
    /// What `Snappr.root` resolves against: the directory holding `lib/`, `share/`
    /// and `config/`.
    pub snappr_root: PathBuf,
    pub entrypoint: PathBuf,
}

impl Runtime {
    pub fn detect(app: &AppHandle) -> Result<Self, String> {
        // `is_dir`, never `exists`. Under `tauri dev` the resource directory is
        // `src-tauri/target/debug`, which already contains a *file* called
        // `Snappr` - the binary itself, named by the `[[bin]]` target in
        // `Cargo.toml` - and the disk is case-insensitive, so the probe below
        // matches it whatever the case.
        // `exists` would therefore be true in development, sending every dev run
        // down the bundle branch to look for `Contents/MacOS/ruby`, and the
        // resulting failure reads as a broken Ruby runtime rather than as a path
        // bug. This one word is the whole guard.
        let bundled = app
            .path()
            .resource_dir()
            .ok()
            .map(|dir| dir.join("snappr"))
            .filter(|dir| dir.is_dir());

        let runtime = match bundled {
            Some(root) => Self::in_bundle(app, root)?,
            None => Self::in_development()?,
        };
        runtime.check()?;
        Ok(runtime)
    }

    fn in_bundle(app: &AppHandle, snappr_root: PathBuf) -> Result<Self, String> {
        // Contents/MacOS/<binary> -> Contents
        let contents = std::env::current_exe()
            .map_err(|e| Problem::PipelineFilesLost { detail: detail(e) }.say())?
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| {
                Problem::PipelineFilesLost {
                    detail: "the executable is not inside a bundle".into(),
                }
                .say()
            })?
            .to_path_buf();

        let _ = app;
        Ok(Self {
            ruby: contents.join("MacOS/ruby"),
            vips: contents.join("Frameworks"),
            gem_setup: snappr_root.join("gems/bundler/setup.rb"),
            entrypoint: snappr_root.join("bin/snappr"),
            snappr_root,
        })
    }

    fn in_development() -> Result<Self, String> {
        // src-tauri/ -> the project root. CARGO_MANIFEST_DIR is baked in at compile
        // time, which is exactly right here: in development the binary is only ever
        // run from the tree it was built in.
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or_else(|| {
                Problem::PipelineFilesLost {
                    detail: format!("no parent for CARGO_MANIFEST_DIR {}", env!("CARGO_MANIFEST_DIR")),
                }
                .say()
            })?
            .to_path_buf();

        Ok(Self {
            ruby: root.join("build/ruby-runtime/bin/ruby"),
            vips: root.join("build/vips"),
            gem_setup: root.join("build/gems/bundler/setup.rb"),
            entrypoint: root.join("bin/snappr"),
            snappr_root: root,
        })
    }

    fn check(&self) -> Result<(), String> {
        // The labels are the console's half. The operator gets one sentence
        // whichever piece is missing, because "the vendored gems" is not a
        // distinction they can act on. The label and the path are how a
        // developer tells which stage of `mise run runtime` has not been run -
        // that is the whole of it now, since the window no longer says so.
        for (what, path) in [
            ("the Ruby interpreter", &self.ruby),
            ("the libvips directory", &self.vips),
            ("the vendored gems", &self.gem_setup),
            ("bin/snappr", &self.entrypoint),
        ] {
            if !path.exists() {
                return Err(Problem::PipelineNotInstalled { what, path }.say());
            }
        }
        Ok(())
    }

    /// The complete environment for the sidecar. Everything here is load-bearing;
    /// see the comments on each entry.
    pub fn env(&self) -> HashMap<String, String> {
        HashMap::from([
            // FFI reads this from Ruby's ENV and joins the paths itself, rather than
            // relying on dyld - so it works despite meaning nothing to dyld on macOS,
            // and unlike DYLD_* it survives hardened runtime. Without it FFI falls
            // through to its hardcoded /opt/homebrew/lib, which "works" on a
            // developer machine and fails everywhere else.
            ("LD_LIBRARY_PATH".into(), self.vips.display().to_string()),
            // libheif bakes in its own plugin directory the same way libvips used to
            // bake in its module directory, and opportunistically dlopens everything
            // it finds there - dragging in a second libheif plus ffmpeg, opus, vpx
            // and x264. libde265 is linked in, so decoding needs none of it. Point
            // this somewhere empty and the scan finds nothing.
            (
                "LIBHEIF_PLUGIN_PATH".into(),
                self.vips.join("no-heif-plugins").display().to_string(),
            ),
            ("SNAPPR_ROOT".into(), self.snappr_root.display().to_string()),
            ("SNAPPR_LOG_LEVEL".into(), "info".into()),
        ])
    }

    /// `--disable-gems` is correct and faster: the standalone bundle puts the gems
    /// on `$LOAD_PATH` by itself, so rubygems is never needed.
    pub fn args(&self) -> Vec<String> {
        vec![
            "--disable-gems".into(),
            "-r".into(),
            self.gem_setup.display().to_string(),
            self.entrypoint.display().to_string(),
            "serve".into(),
        ]
    }
}
