//! The cache key for a derived file.
//!
//! Two things used to compute this - `Preview#key_for` in Ruby with a SHA-256,
//! and `overlays.rs` in Rust with a `DefaultHasher` - agreeing on the *rule* by
//! comment and on nothing else. One function now, and both callers pass the files
//! the derived thing depends on.
//!
//! **Stable across runs**, so a restart finds the files the index points at, and
//! **keyed on mtime to the nanosecond as well as path**. Seconds are not enough:
//! a retry straight after a failed composite can land inside the same second, and
//! a key that cannot tell two composites apart is the whole thing this exists to
//! avoid.

use std::path::Path;

use sha2::{Digest, Sha256};

/// A 16-hex-character key over the paths and modification times given.
///
/// Order matters: `key_for(&[source, print])` is not `key_for(&[print, source])`.
///
/// **Pass every file the derived thing depends on.** A photo's previews are keyed
/// on the source *and* the print, and that second one is not redundant: the
/// webview serves an `asset://` URL out of its own memory cache for the life of
/// the session, so re-compositing under a key that did not change would leave the
/// operator looking at the previous branding. That bug has been fixed once
/// already.
pub fn key_for(paths: &[&Path]) -> String {
    let mut hasher = Sha256::new();
    for (index, path) in paths.iter().enumerate() {
        if index > 0 {
            hasher.update([0]);
        }
        hasher.update(stamp(path).as_bytes());
    }
    let digest = hasher.finalize();
    digest.iter().take(8).map(|byte| format!("{byte:02x}")).collect()
}

/// A path and its modification time, to the nanosecond.
///
/// Absolute but **not canonicalised**: resolving symlinks would change the key
/// for the same file reached by a different name, which is exactly the stale
/// preview the composite-in-the-key rule exists to prevent.
fn stamp(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map(|dir| dir.join(path)).unwrap_or_else(|_| path.to_path_buf())
    };

    let (secs, nanos) = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| (since.as_secs(), since.subsec_nanos()))
        // A file that is not there contributes a zero rather than failing: the
        // caller is about to write it, and a key it can compute now is worth more
        // than an error.
        .unwrap_or((0, 0));

    format!("{}\0{secs}.{nanos:09}", absolute.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("snappr-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn the_key_is_sixteen_hex_characters() {
        let path = temp("a.txt", "a");
        let key = key_for(&[&path]);

        assert_eq!(key.len(), 16, "{key}");
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()), "{key}");
    }

    #[test]
    fn the_same_file_gives_the_same_key() {
        let path = temp("stable.txt", "a");
        assert_eq!(key_for(&[&path]), key_for(&[&path]));
    }

    #[test]
    fn a_rewritten_file_gives_a_different_key() {
        let path = temp("rewritten.txt", "a");
        let before = key_for(&[&path]);

        // Same path, same length, different mtime - which is the case a key on
        // path alone cannot tell apart.
        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&path, "b").unwrap();

        assert_ne!(before, key_for(&[&path]));
    }

    #[test]
    fn every_file_in_the_key_counts() {
        let one = temp("one.txt", "1");
        let two = temp("two.txt", "2");

        assert_ne!(key_for(&[&one]), key_for(&[&one, &two]));
        assert_ne!(key_for(&[&one, &two]), key_for(&[&two, &one]));
    }

    #[test]
    fn a_missing_file_still_yields_a_key() {
        let key = key_for(&[Path::new("/nonexistent/photo.heic")]);
        assert_eq!(key.len(), 16);
    }
}
