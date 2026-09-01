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
//! snappr.app/Contents/
//!   MacOS/ruby            the interpreter          (externalBin)
//!   Frameworks/*.dylib    libvips and its closure  (macOS.frameworks)
//!   lib/ruby/             the Ruby stdlib          (macOS.files, inert)
//!   Resources/snappr/     lib, share, config, gems (resources, inert)
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

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
            .map_err(|e| format!("cannot locate the executable: {e}"))?
            .parent()
            .and_then(Path::parent)
            .ok_or("the executable is not inside a bundle")?
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
            .ok_or("cannot find the project root")?
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
        for (label, path) in [
            ("the Ruby interpreter", &self.ruby),
            ("the libvips directory", &self.vips),
            ("the vendored gems", &self.gem_setup),
            ("bin/snappr", &self.entrypoint),
        ] {
            if !path.exists() {
                return Err(format!(
                    "{label} is missing at {}. In development, run `mise run runtime` first.",
                    path.display()
                ));
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
