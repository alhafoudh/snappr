//! The message catalogue's rules, asserted against the whole table.
//!
//! Ported from `test/problem_test.rb`, and it keeps that suite's most useful
//! property: **every assertion here is about a rule, not about wording.** Reword
//! any sentence freely; these tests only fail if a message starts leaking jargon,
//! loses a half, or shows the operator something meant for the log.

use std::path::PathBuf;

use snappr_core::problem::{Level, Problem};

/// One of every variant. The count is asserted below, so adding a variant to the
/// catalogue without adding it here fails rather than quietly going untested.
fn every_problem() -> Vec<Problem> {
    let path = PathBuf::from("/Users/op/Designs/brand.svg");
    let photo = PathBuf::from("/Users/op/Downloads/IMG_1234.heic");
    let slot = "landscape overlay".to_string();
    let file = "brand.svg".to_string();

    vec![
        Problem::OverlayWindowMissing { slot: slot.clone(), file: file.clone(), path: path.clone() },
        Problem::OverlayFileMissing { slot: slot.clone(), file: file.clone(), path: path.clone() },
        Problem::OverlayUnreadable { slot: slot.clone(), file: file.clone(), path: path.clone() },
        Problem::OverlayNoSize { slot: slot.clone(), file: file.clone(), path: path.clone() },
        Problem::OverlayNoShape { file: file.clone(), path: path.clone() },
        Problem::OverlayMissing {
            slot: slot.clone(),
            file: file.clone(),
            path: path.clone(),
            orientation: "landscape",
        },
        Problem::AspectMismatch {
            slot: slot.clone(),
            file: file.clone(),
            width: 2960,
            height: 2000,
            drawn: 0.676,
            wanted: 1.48,
            orientation: "landscape",
        },
        Problem::MarkerSurvived { slot, file: file.clone(), path: path.clone() },
        Problem::PhotoMissing { file: "IMG_1234.heic".into(), path: photo.clone() },
        Problem::TransferStalled { file: "IMG_1234.heic".into(), path: photo.clone(), secs: 30 },
        Problem::PhotoUnusable { detail: "VipsForeignLoad: unsupported".into() },
        Problem::ProfileUnusable { detail: "icc_transform failed".into() },
        Problem::OpenFailed { file: "IMG_1234.overlay.png".into(), path: photo.clone() },
        Problem::ImageToolsUnavailable { detail: "libvips.42.dylib not found".into() },
        Problem::NotAFolder { path: photo.clone() },
        Problem::CannotWatch { path: photo.clone(), detail: "EPERM".into() },
        Problem::BackgroundNotAColour { value: "not-a-colour".into() },
        Problem::PaperSizeInvalid { short_mm: 148.0, long_mm: 100.0 },
        Problem::CompressionOutOfRange { value: 12 },
        Problem::PreviewSizeInvalid { max_px: 300, thumb_px: 400 },
        Problem::PreviewQualityOutOfRange { value: 0 },
        Problem::StabilityInvalid { checks: 0, timeout_secs: 30 },
        Problem::NoExtensions,
        Problem::PngWatched,
        Problem::OutputSuffixInvalid { value: ".print.jpg".into(), reason: "it has to end in .png" },
        Problem::CannotExportConfig { path: path.clone(), detail: "EACCES".into() },
        Problem::ConfigUnreadable { path: path.clone(), detail: "expected a value".into() },
        Problem::SettingsUnreadable { path: path.clone(), detail: "expected a value".into() },
        Problem::CannotSaveSettings { detail: "ENOSPC".into() },
        Problem::CannotRememberPhotos { detail: "ENOSPC".into() },
        Problem::OverlayFileGone { file: file.clone(), path: path.clone() },
        Problem::OverlayNotADrawing { file: "brand.pdf".into(), path: path.clone() },
        Problem::OverlayAlreadyAdded { file: file.clone() },
        Problem::CannotDrawOverlays { detail: "ENOSPC".into() },
        Problem::PhotoForgotten { path: photo.clone() },
        Problem::PhotoStillWorking,
        Problem::PrintFileGone,
        Problem::PrintUnreadable { path: photo.clone() },
        Problem::PaperFallback,
        Problem::PrinterMissing { name: "Canon SELPHY CP1500".into() },
        Problem::FinderWontOpen { detail: "open -R exited 1".into() },
        Problem::WindowNotReady { what: "share sheet", detail: "no NSView".into() },
        Problem::DropAlreadyAPrint { file: "IMG_1234.overlay.png".into() },
        Problem::DropIsAFolder { file: "Event".into() },
        Problem::DropStillDownloading { file: "IMG_1234.heic.download".into() },
        Problem::DropWrongKind {
            file: "notes.txt".into(),
            kind: "a txt file".into(),
            accepted: "HEIC and JPEG".into(),
        },
        Problem::DropUnreadableName { path: photo.clone() },
        Problem::DropNotAFile { url: "https://example.com".into() },
        Problem::UnknownCommand { command: "sing".into() },
        Problem::NoPathsGiven,
    ]
}

#[test]
fn the_table_is_covered() {
    let problems = every_problem();
    let mut codes: Vec<&str> = problems.iter().map(Problem::code).collect();
    codes.sort_unstable();
    codes.dedup();

    assert_eq!(
        codes.len(),
        problems.len(),
        "two variants share a code, or one is listed twice"
    );
    assert_eq!(
        problems.len(),
        50,
        "a variant was added to the catalogue but not to every_problem()"
    );
}

/// The operator's half is what reaches the window and the terminal. It may not
/// carry anything that only means something to whoever wrote the code.
#[test]
fn no_operator_half_leaks_jargon() {
    // Every one of these has been in a user-facing string in this project at some
    // point, which is why they are the list.
    const FORBIDDEN: [&str; 12] = [
        "#ff00ff", "viewbox", "sidecar", "handshake", "ndjson", "stdin", "stdout", "icc",
        "::", "libvips", "svg>", "vips",
    ];

    for problem in every_problem() {
        let ui = problem.ui().to_lowercase();
        for needle in FORBIDDEN {
            assert!(
                !ui.contains(needle),
                "{} leaks {needle:?}: {}",
                problem.code(),
                problem.ui()
            );
        }
    }
}

#[test]
fn every_operator_half_is_a_sentence() {
    // The exemption Ruby wrote as "unless it starts with `%{`": a message may open
    // with an interpolated file name, because "notes.txt is ..." reads better than
    // "The file notes.txt is ...". These are the fixture values above, so a
    // message opening with a real interpolation is allowed and one opening with a
    // lower-case word is not.
    let interpolated = ["brand.svg", "brand.pdf", "IMG_1234", "notes.txt", "Event"];

    for problem in every_problem() {
        let ui = problem.ui();
        assert!(!ui.trim().is_empty(), "{} has no operator half", problem.code());

        if interpolated.iter().any(|value| ui.starts_with(value)) {
            continue;
        }

        let first = ui.chars().next().unwrap();
        assert!(
            first.is_uppercase(),
            "{} does not start with a capital: {ui}",
            problem.code()
        );
    }
}

/// A shell command is the fix on a developer's machine and nonsense in a shipped
/// app, so no operator half may name one. This is the rule the old
/// `mise run runtime` message broke.
#[test]
fn no_operator_half_tells_the_operator_to_run_a_command() {
    for problem in every_problem() {
        let ui = problem.ui();
        if problem.code() == "no_paths_given" {
            // The one exception, and it is not really one: this message is only
            // ever printed by the CLI, in answer to a command the operator just
            // typed wrongly. Showing the correct invocation *is* the fix.
            assert!(ui.contains("snappr overlay"));
            continue;
        }
        for needle in ["mise ", "cargo ", "npm ", "bundle exec", "$ "] {
            assert!(!ui.contains(needle), "{} names a command: {ui}", problem.code());
        }
    }
}

/// The log gets both halves, always in the same order. That is what makes a log
/// readable by whoever is holding the printer and still diagnosable later.
#[test]
fn the_log_line_carries_both_halves() {
    let problem = Problem::PhotoMissing {
        file: "IMG_1234.heic".into(),
        path: "/Users/op/Downloads/IMG_1234.heic".into(),
    };

    let ui = problem.ui();
    let log = problem.log_line();

    assert!(log.contains(&ui), "the log line contains the operator's sentence");
    assert!(log.contains("/Users/op/Downloads"), "and the path");
    // The path is the thing the operator does not need and the developer does.
    assert!(!ui.contains("/Users/op/Downloads"), "the sentence does not carry the path");
    assert!(ui.contains("IMG_1234.heic"), "but it does name the file");
}

#[test]
fn a_problem_with_no_console_half_logs_just_the_sentence() {
    let problem = Problem::PaperFallback;
    assert_eq!(problem.log_line(), problem.ui());
}

/// A libvips failure never shows its own text. It borrows the catalogue's
/// sentence and keeps the original for the log - and the mapping is by type, so
/// it cannot be defeated by a reworded libvips message.
#[test]
fn a_foreign_error_never_shows_its_own_text() {
    let problem = Problem::PhotoUnusable {
        detail: "VipsForeignLoad: buffer is not in a known format".into(),
    };

    assert!(!problem.ui().contains("VipsForeignLoad"));
    assert_eq!(problem.code(), "photo_unusable");
    assert!(problem.log_line().contains("VipsForeignLoad"));
}

/// The frontend gets the code as well as the sentence, so it can branch on the
/// kind of failure. Previously every command returned a bare string.
#[test]
fn the_payload_carries_the_code_and_the_sentence_only() {
    let problem = Problem::ProfileUnusable { detail: "icc_transform failed: bad profile".into() };
    let payload = problem.payload();

    assert_eq!(payload.code, "profile_unusable");
    assert_eq!(payload.message, problem.ui());
    assert!(!payload.message.contains("icc_transform"));
}

#[test]
fn a_warning_carries_the_code_as_its_kind() {
    let warning = Problem::MarkerSurvived {
        slot: "landscape overlay".into(),
        file: "brand.svg".into(),
        path: "/Users/op/Designs/brand.svg".into(),
    }
    .warn();

    assert_eq!(warning.kind, "marker_survived");
    assert!(!warning.message.is_empty());
}

#[test]
fn levels_serialise_the_way_the_frontend_reads_them() {
    assert_eq!(serde_json::to_string(&Level::Error).unwrap(), "\"error\"");
    assert_eq!(serde_json::to_string(&Level::Warning).unwrap(), "\"warning\"");
}
