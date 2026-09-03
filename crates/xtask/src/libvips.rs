//! Builds libvips from source into `build/libvips-prefix`.
//!
//! **`-Dmodules=disabled` is the whole point.** Homebrew's libvips dlopens its
//! format support from a `VIPS_LIBDIR` baked in at compile time, so a copied
//! Homebrew libvips has no HEIC support wherever you put it - and most of what
//! gets AirDropped to a kiosk is HEIC. Built this way the loaders are *in* the
//! library, which is what makes the closure in `build/vips` relocatable at all.
//!
//! sharp's prebuilt libvips was the obvious shortcut and does not work either: it
//! cannot decode HEVC, and iPhone HEICs are HEVC.

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::{human, output, relative, say, sh};

const VERSION: &str = "8.18.4";

/// From the GitHub release asset digest for v8.18.4.
const TARBALL_SHA256: &str = "2677bad6c422617fd1172d359c16af34e736965d042c214203a87187d26ff037";

/// `enabled`, not `auto`: a missing dependency must fail the build loudly here
/// rather than silently produce a libvips that cannot read what gets AirDropped.
const REQUIRED: [&str; 8] = ["heif", "rsvg", "jpeg", "png", "lcms", "exif", "zlib", "tiff"];

/// meson distinguishes "feature" options (enabled/disabled/auto) from plain
/// booleans (true/false), and libvips uses both. Passing `disabled` to a boolean
/// fails setup with `Option "nsgif" value disabled is not boolean`, so the two
/// lists have to stay separate. Types are from libvips' own `meson_options.txt`.
const DISABLED_FEATURES: [&str; 30] = [
    "modules",
    "heif-module",
    "jpeg-xl-module",
    "magick-module",
    "openslide-module",
    "poppler-module",
    "magick",
    "poppler",
    "openslide",
    "jpeg-xl",
    "openexr",
    "openjpeg",
    "fftw",
    "cfitsio",
    "matio",
    "nifti",
    "pdfium",
    "quantizr",
    "imagequant",
    "webp",
    "raw",
    "cgif",
    "uhdr",
    "archive",
    "orc",
    "pangocairo",
    "fontconfig",
    "introspection",
    "highway",
    "spng",
];

const DISABLED_BOOLEANS: [&str; 7] =
    ["nsgif", "cplusplus", "cpp-docs", "docs", "examples", "vapi", "deprecated"];

pub fn run(root: &Path, force: bool) -> Result<(), String> {
    let paths = Paths::new(root);

    if force {
        let _ = std::fs::remove_dir_all(&paths.prefix);
        let _ = std::fs::remove_dir_all(&paths.work);
    } else if paths.library().is_file() {
        say("libvips", format!("already built at {} (--force to rebuild)", paths.rel(paths.library())));
        return report(&paths);
    }

    fetch(&paths)?;
    extract(&paths)?;
    setup(&paths)?;
    compile(&paths)?;
    report(&paths)
}

struct Paths {
    root: PathBuf,
    cache: PathBuf,
    src: PathBuf,
    work: PathBuf,
    prefix: PathBuf,
}

impl Paths {
    fn new(root: &Path) -> Self {
        let src = root.join(format!("build/src/vips-{VERSION}"));
        Self {
            root: root.to_path_buf(),
            cache: root.join("build/cache"),
            work: src.join("_build"),
            src,
            prefix: root.join("build/libvips-prefix"),
        }
    }

    fn library(&self) -> PathBuf {
        self.prefix.join("lib/libvips.42.dylib")
    }

    fn tarball(&self) -> PathBuf {
        self.cache.join(format!("vips-{VERSION}.tar.xz"))
    }

    fn rel(&self, path: impl AsRef<Path>) -> String {
        relative(path.as_ref(), &self.root)
    }
}

fn fetch(paths: &Paths) -> Result<(), String> {
    let tarball = paths.tarball();
    if tarball.is_file() && digest(&tarball)? == TARBALL_SHA256 {
        say("libvips", "tarball already fetched and verified");
        return Ok(());
    }

    std::fs::create_dir_all(&paths.cache).map_err(|e| e.to_string())?;
    let url = format!(
        "https://github.com/libvips/libvips/releases/download/v{VERSION}/vips-{VERSION}.tar.xz"
    );
    say("libvips", format!("fetching {url}"));
    sh(Command::new("curl")
        .args(["-fL", "--retry", "3", "-o"])
        .arg(&tarball)
        .arg(&url))?;

    let actual = digest(&tarball)?;
    if actual != TARBALL_SHA256 {
        let _ = std::fs::remove_file(&tarball);
        return Err(format!(
            "checksum mismatch\n  expected {TARBALL_SHA256}\n  got      {actual}"
        ));
    }
    say("libvips", "checksum verified");
    Ok(())
}

fn digest(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let digest = Sha256::digest(&bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn extract(paths: &Paths) -> Result<(), String> {
    if paths.src.join("meson.build").is_file() {
        say("libvips", "source already extracted");
        return Ok(());
    }

    let into = paths.src.parent().ok_or("no parent for the source directory")?;
    std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
    say("libvips", format!("extracting into {}", paths.rel(&paths.src)));
    sh(Command::new("tar").arg("xJf").arg(paths.tarball()).arg("-C").arg(into))
}

fn setup(paths: &Paths) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(&paths.work);

    let mut options: Vec<String> = REQUIRED.iter().map(|name| format!("-D{name}=enabled")).collect();
    options.extend(DISABLED_FEATURES.iter().map(|name| format!("-D{name}=disabled")));
    options.extend(DISABLED_BOOLEANS.iter().map(|name| format!("-D{name}=false")));

    say("libvips", format!("configuring with {} options", options.len()));
    sh(Command::new("meson")
        .arg("setup")
        .arg(&paths.work)
        .arg(format!("--prefix={}", paths.prefix.display()))
        .arg("--buildtype=release")
        .arg("--default-library=shared")
        // No install rpath: `closure` rewrites the whole thing anyway, and the
        // install *name* is set by `name_for_rpath` below.
        .arg("-Dcpp_std=c++17")
        .args(&options)
        .current_dir(&paths.src))
}

fn compile(paths: &Paths) -> Result<(), String> {
    say("libvips", "building with ninja");
    sh(Command::new("ninja").arg("-C").arg(&paths.work))?;
    say("libvips", format!("installing into {}", paths.rel(&paths.prefix)));
    sh(Command::new("ninja").arg("-C").arg(&paths.work).arg("install"))
}

// The install name is deliberately left as meson wrote it: the absolute path to
// this prefix. A *debug* build links this copy and then needs no `LC_RPATH` at all,
// which is what keeps `cargo test` working - see `crates/snappr-core/build.rs`.
// The `@rpath` name belongs to the relocated copy in `build/vips`, which is what a
// release build links through `build/link`.

/// The point of the exercise: prove the loaders are IN the library rather than in
/// a module directory beside it.
fn report(paths: &Paths) -> Result<(), String> {
    let library = paths.library();
    if !library.is_file() {
        return Err(format!("no library at {}", paths.rel(&library)));
    }

    println!();
    say("libvips", format!("=== {} ===", paths.rel(&library)));
    let size = std::fs::metadata(&library).map(|meta| meta.len()).unwrap_or(0);
    say("libvips", format!("size: {}", human(size)));

    let modules: Vec<PathBuf> = std::fs::read_dir(paths.prefix.join("lib"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("vips-modules-"))
                })
                .collect()
        })
        .unwrap_or_default();

    if modules.is_empty() {
        say("libvips", "no module directory - format support is linked in, as intended");
    } else {
        say(
            "libvips",
            format!(
                "WARNING: a module directory exists at {} - modules were meant to be disabled",
                modules.iter().map(|path| paths.rel(path)).collect::<Vec<_>>().join(", ")
            ),
        );
    }

    let vendored = dependencies_of(&library)
        .into_iter()
        .filter(|dep| dep.starts_with("/opt/homebrew"))
        .count();
    say("libvips", format!("links {vendored} Homebrew dylibs (closure relocates these)"));

    println!();
    say("libvips", "next: cargo xtask closure");
    Ok(())
}

pub fn dependencies_of(path: &Path) -> Vec<String> {
    let Some(out) = output("otool", &["-L", &path.display().to_string()]) else {
        return Vec::new();
    };
    out.lines()
        .skip(1)
        .filter_map(|line| line.trim().split(" (compatibility").next())
        .map(str::to_string)
        .collect()
}
