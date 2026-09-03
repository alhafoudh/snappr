//! Collects libvips' dylib closure into `build/vips` and makes it relocatable.
//!
//! Three things here were each learned the hard way.
//!
//! **The walk must follow `@rpath` references, not only absolute ones.**
//! Homebrew's OpenEXR names libIlmThread as `@rpath/libIlmThread-3_4.33.dylib`,
//! so a walk that followed absolute paths alone silently dropped the whole
//! OpenEXR family - and the resulting libvips then failed to load with nothing
//! saying why on a machine that had Homebrew to fall back on.
//!
//! **Libraries are keyed by the basename of the install *name*, not of the real
//! file.** Homebrew's `libpng16.16.dylib` is a symlink, and dependents record the
//! symlink's name.
//!
//! **Every rewritten file has to be re-signed.** `install_name_tool` invalidates
//! the signature, and dyld refuses a present-but-wrong one - so an unsigned
//! rewrite fails at launch rather than here.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::libvips::dependencies_of;
use crate::{human, output, relative, say, sh};

/// Only these prefixes are ours to carry. `/usr/lib` and `/System` are the OS's
/// and are present on every machine.
const VENDOR_PREFIXES: [&str; 2] = ["/opt/homebrew", "/usr/local"];

/// The libraries named from *outside* `build/vips`, by a binary that links them.
///
/// snappr's executables live in `Contents/MacOS`, not in `Contents/Frameworks`,
/// so `@loader_path` - which for an executable means the executable's own
/// directory - is the wrong answer for these three. Their ids are `@rpath/...`
/// instead, resolved by the `LC_RPATH` each binary carries. Every other library
/// here is only ever loaded *by a sibling* and stays `@loader_path/...`; sibling
/// references to these three are rewritten that way too, so both routes work at
/// once.
///
/// It is three rather than one because `rs-vips` emits `-lglib-2.0` and
/// `-lgobject-2.0` alongside `-lvips`: it calls GObject directly to manage image
/// lifetimes. Miss them and a binary links *Homebrew's* glib by absolute path
/// while libvips resolves to the bundled one - two copies of glib in one process,
/// which segfaults inside `vips_init` with nothing in the message about glib.
const RPATH_NAMED: [&str; 3] =
    ["libvips.42.dylib", "libglib-2.0.0.dylib", "libgobject-2.0.0.dylib"];

pub fn run(root: &Path, force: bool) -> Result<(), String> {
    let source = root.join("build/libvips-prefix/lib/libvips.42.dylib");
    let dest = root.join("build/vips");

    if force {
        let _ = std::fs::remove_dir_all(&dest);
    } else if dylibs_in(&dest).len() > 1 {
        say("closure", "build/vips already populated, skipping to verification (--force to rebuild)");
        return verify(root, &dest);
    }

    if !source.is_file() {
        return Err(format!(
            "no libvips at {} - run `cargo xtask libvips` first",
            relative(&source, root)
        ));
    }

    let found = collect(&source);
    copy(&found, &dest, root)?;
    relocate(&dest)?;
    write_frameworks_list(root, &dest)?;
    write_link_directory(root, &dest)?;
    verify(root, &dest)
}

/// Breadth-first over `otool -L`, following only vendored paths.
///
/// Keyed by install-name basename, valued by the real file behind it.
fn collect(source: &Path) -> BTreeMap<String, PathBuf> {
    say("closure", "walking the dependency closure");

    let mut found: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: Vec<String> = vec![source.display().to_string()];

    while let Some(install_name) = queue.pop() {
        if !seen.insert(install_name.clone()) {
            continue;
        }

        let Some(real) = std::fs::canonicalize(&install_name).ok().filter(|path| path.is_file())
        else {
            say("closure", format!("  WARNING: {install_name} is referenced but missing, skipping"));
            continue;
        };

        let name = Path::new(&install_name)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| install_name.clone());

        if let Some(previous) = found.get(&name) {
            if previous != &real {
                say(
                    "closure",
                    format!(
                        "  WARNING: two different files both called {name} ({}, {})",
                        previous.display(),
                        real.display()
                    ),
                );
            }
        }
        found.insert(name, real.clone());

        let rpaths = rpaths_of(&real);
        for dep in dependencies_of(&real) {
            if let Some(resolved) = resolve_dependency(&dep, &real, &rpaths) {
                queue.push(resolved);
            }
        }
    }

    found
}

/// An absolute vendored path is taken as-is; an `@rpath/` reference is resolved
/// through the dependent's own `LC_RPATH` entries, which are themselves often
/// `@loader_path`-relative.
fn resolve_dependency(dep: &str, dependent: &Path, rpaths: &[String]) -> Option<String> {
    if is_vendored(dep) {
        return Some(dep.to_string());
    }
    let name = dep.strip_prefix("@rpath/")?;
    let here = dependent.parent()?.display().to_string();

    for rpath in rpaths {
        let base = rpath.replace("@loader_path", &here).replace("@executable_path", &here);
        let candidate = Path::new(&base).join(name);
        if candidate.is_file() {
            let real = std::fs::canonicalize(&candidate).ok()?;
            if is_vendored(&real.display().to_string()) {
                return Some(candidate.display().to_string());
            }
        }
    }
    None
}

fn is_vendored(path: &str) -> bool {
    VENDOR_PREFIXES.iter().any(|prefix| path.starts_with(prefix))
}

/// Copies each library in, flat, by its install name.
///
/// `copy`, never a hard link or a symlink: these are about to be rewritten, and
/// rewriting a link would edit Homebrew's own file.
fn copy(found: &BTreeMap<String, PathBuf>, dest: &Path, root: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    say("closure", format!("copying {} dylibs into {}", found.len(), relative(dest, root)));

    for (name, real) in found {
        let target = dest.join(name);
        std::fs::copy(real, &target).map_err(|e| format!("{}: {e}", real.display()))?;
        let mut perms = std::fs::metadata(&target).map_err(|e| e.to_string())?.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o644);
        }
        std::fs::set_permissions(&target, perms).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn relocate(dest: &Path) -> Result<(), String> {
    let files = dylibs_in(dest);
    say("closure", format!("rewriting install names for {} dylibs", files.len()));

    for file in &files {
        let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let own = if RPATH_NAMED.contains(&name.as_str()) { "@rpath" } else { "@loader_path" };
        sh(Command::new("install_name_tool").arg("-id").arg(format!("{own}/{name}")).arg(file))?;

        for dep in dependencies_of(file) {
            if !is_vendored(&dep) && !dep.starts_with("@rpath/") {
                continue;
            }
            let base = Path::new(&dep).file_name().unwrap_or_default().to_string_lossy().into_owned();
            sh(Command::new("install_name_tool")
                .arg("-change")
                .arg(&dep)
                .arg(format!("@loader_path/{base}"))
                .arg(file))?;
        }

        // Homebrew bakes absolute LC_RPATH entries into some libraries. Harmless
        // once nothing resolves through them, but they point outside the bundle,
        // so strip them rather than shipping a stale search path.
        for rpath in rpaths_of(file) {
            if is_vendored(&rpath) {
                sh(Command::new("install_name_tool").arg("-delete_rpath").arg(&rpath).arg(file))?;
            }
        }

        // install_name_tool invalidated whatever signature was there, and dyld
        // refuses a present-but-wrong one.
        sh(Command::new("codesign")
            .args(["--force", "--sign", "-", "--timestamp=none"])
            .arg(file))?;
    }

    say("closure", format!("rewrote and re-signed {} files", files.len()));
    Ok(())
}

/// Tauri's `bundle.macOS.frameworks` takes explicit paths, not a glob, and it is
/// what copies these dylibs into `Contents/Frameworks` *and codesigns them
/// there* - which is why the app cannot simply carry them as `resources`.
///
/// Forty-odd paths are not something to maintain by hand, so the task that
/// decides which dylibs exist is the task that writes the list. It goes in its own
/// config file, merged at build time with `tauri build --config`, so the generated
/// part stays out of `tauri.conf.json` where a human is editing.
fn write_frameworks_list(root: &Path, dest: &Path) -> Result<(), String> {
    let paths: Vec<String> = dylibs_in(dest)
        .iter()
        .map(|file| format!("../{}", relative(file, root)))
        .collect();

    let config = serde_json::json!({
        "bundle": { "macOS": { "frameworks": paths } }
    });
    let target = root.join("src-tauri/tauri.frameworks.json");
    let body = format!("{}\n", serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?);
    std::fs::write(&target, body).map_err(|e| format!("{}: {e}", target.display()))?;

    say("closure", format!("wrote {} with {} dylibs", relative(&target, root), dylibs_in(dest).len()));
    Ok(())
}

/// A directory of unversioned symlinks, for the *linker* only.
///
/// `rs-vips` emits `-lglib-2.0`, and a linker resolves that to
/// `libglib-2.0.dylib` - a name Homebrew has and `build/vips` does not, since the
/// closure is flat and keyed by real install names. Without this the link falls
/// through to Homebrew's copy, whose absolute install name is then baked into the
/// binary, and the bundle carries a glib it never loads while loading Homebrew's.
///
/// Symlinks live here rather than in `build/vips` so that directory stays exactly
/// what ships - `verify` asserts it holds no links at all.
fn write_link_directory(root: &Path, dest: &Path) -> Result<(), String> {
    let link_dir = root.join("build/link");
    let _ = std::fs::remove_dir_all(&link_dir);
    std::fs::create_dir_all(&link_dir).map_err(|e| e.to_string())?;

    for name in RPATH_NAMED {
        let real = dest.join(name);
        if !real.is_file() {
            return Err(format!("{name} is not in the closure - the walk missed it"));
        }
        // libvips.42.dylib -> libvips.dylib, libglib-2.0.0.dylib -> libglib-2.0.dylib
        let unversioned = match name {
            "libvips.42.dylib" => "libvips.dylib",
            "libglib-2.0.0.dylib" => "libglib-2.0.dylib",
            "libgobject-2.0.0.dylib" => "libgobject-2.0.dylib",
            other => other,
        };
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, link_dir.join(unversioned))
            .map_err(|e| format!("{unversioned}: {e}"))?;
    }

    say("closure", format!("wrote {} for the linker", relative(&link_dir, root)));
    Ok(())
}

fn verify(root: &Path, dest: &Path) -> Result<(), String> {
    println!();
    say("closure", "=== verification ===");
    let mut failures = 0;
    let mut check = |what: &str, ok: bool, detail: String| {
        if ok {
            say("closure", format!("  ok    {what}"));
        } else {
            failures += 1;
            say("closure", format!("  FAIL  {what}{}", if detail.is_empty() { String::new() } else { format!(" - {detail}") }));
        }
    };

    let top: Vec<PathBuf> = std::fs::read_dir(dest)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    let dylibs = dylibs_in(dest);

    check("libvips itself is present", dest.join("libvips.42.dylib").is_file(), String::new());

    // HEIC decode is linked into libvips rather than dlopened, so libheif must
    // show up as an ordinary dependency. If it does not, the libvips build lost
    // its heif support and most of what gets AirDropped becomes unreadable.
    let heif: Vec<String> = dylibs
        .iter()
        .filter_map(|file| file.file_name().map(|name| name.to_string_lossy().into_owned()))
        .filter(|name| name.starts_with("libheif"))
        .collect();
    check("libheif is a direct dependency", !heif.is_empty(), heif.join(", "));

    // A module directory here means libvips was built without
    // `-Dmodules=disabled`, which puts the un-relocatable module path back.
    let modules: Vec<&PathBuf> = top
        .iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("vips-modules-"))
        })
        .collect();
    check("no module directory", modules.is_empty(), format!("{modules:?}"));

    let strays: Vec<String> = top
        .iter()
        .filter(|path| path.extension().and_then(|e| e.to_str()) != Some("dylib"))
        .filter_map(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    check("nothing but dylibs at the top level", strays.is_empty(), strays.join(", "));

    let links: Vec<String> = top
        .iter()
        .filter(|path| path.symlink_metadata().map(|meta| meta.file_type().is_symlink()).unwrap_or(false))
        .filter_map(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    check("no symlinks", links.is_empty(), links.join(", "));

    let leftover: Vec<String> = dylibs
        .iter()
        .flat_map(|file| {
            dependencies_of(file)
                .into_iter()
                .filter(|dep| is_vendored(dep) || dep.starts_with("@rpath/"))
                .filter(|dep| {
                    // libvips and glib legitimately name *themselves* @rpath.
                    !RPATH_NAMED.iter().any(|named| dep.ends_with(named))
                })
                .map(move |dep| format!("{} -> {dep}", file.file_name().unwrap_or_default().to_string_lossy()))
        })
        .collect();
    check("no vendored references left", leftover.is_empty(), leftover.iter().take(5).cloned().collect::<Vec<_>>().join("; "));

    let wrong_ids: Vec<String> = dylibs
        .iter()
        .filter_map(|file| {
            let name = file.file_name()?.to_string_lossy().into_owned();
            let wanted = if RPATH_NAMED.contains(&name.as_str()) { "@rpath" } else { "@loader_path" };
            let id = install_id_of(file)?;
            (id != format!("{wanted}/{name}")).then_some(format!("{name} is {id}"))
        })
        .collect();
    check("every id is relative, and the linked ones are @rpath", wrong_ids.is_empty(), wrong_ids.join(", "));

    let missing: Vec<String> = dylibs
        .iter()
        .flat_map(|file| {
            let here = file.parent().map(Path::to_path_buf).unwrap_or_default();
            dependencies_of(file)
                .into_iter()
                .filter_map(move |dep| {
                    let name = dep.strip_prefix("@loader_path/")?;
                    (!here.join(name).is_file()).then(|| format!("{} -> {dep}", here.display()))
                })
        })
        .collect();
    check("every @loader_path reference resolves", missing.is_empty(), missing.iter().take(5).cloned().collect::<Vec<_>>().join("; "));

    let unsigned: Vec<String> = dylibs
        .iter()
        .filter(|file| {
            Command::new("codesign")
                .arg("--verify")
                .arg(file)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|status| !status.success())
                .unwrap_or(true)
        })
        .filter_map(|file| file.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    check("every file has a valid signature", unsigned.is_empty(), unsigned.join(", "));

    let total: u64 = dylibs.iter().filter_map(|f| std::fs::metadata(f).ok()).map(|m| m.len()).sum();
    println!();
    say("closure", format!("{} dylibs, {} in {}", dylibs.len(), human(total), relative(dest, root)));

    if failures == 0 {
        say("closure", "closure is relocatable. Next: cargo xtask verify");
        Ok(())
    } else {
        Err(format!("{failures} check(s) failed"))
    }
}

fn dylibs_in(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("dylib"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

fn install_id_of(path: &Path) -> Option<String> {
    let out = output("otool", &["-D", &path.display().to_string()])?;
    out.lines().nth(1).map(|line| line.trim().to_string())
}

fn rpaths_of(path: &Path) -> Vec<String> {
    let Some(out) = output("otool", &["-l", &path.display().to_string()]) else {
        return Vec::new();
    };
    let lines: Vec<&str> = out.lines().collect();
    let mut rpaths = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        if !line.contains("LC_RPATH") {
            continue;
        }
        // `path <value> (offset N)`, two lines below the command name.
        if let Some(entry) = lines.get(at + 2) {
            if let Some(rest) = entry.trim().strip_prefix("path ") {
                if let Some(value) = rest.split(" (offset").next() {
                    rpaths.push(value.to_string());
                }
            }
        }
    }
    rpaths
}
