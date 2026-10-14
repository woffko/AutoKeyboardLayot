//! Helpers shared by `build.rs` and `tests/build_support.rs`.
//!
//! A build script cannot be unit-tested on its own, so the logic that can go wrong lives here and
//! both crates include this file with `#[path]`. Each of them uses a different subset.
#![allow(dead_code)]

use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

/// Paths, relative to the package root, whose content ends up in the executable. A change to a
/// tracked file below one of them marks the build `-dirty`, and any change re-runs the build script.
pub const BUILD_INPUTS: &[&str] = &[
    "src",
    "data",
    "ui",
    "assets",
    "Cargo.toml",
    "Cargo.lock",
    "build.rs",
    "build_support.rs",
];

/// The label for a commit: `abc1234`, or `abc1234-dirty` when the build inputs differ from it.
pub fn format_commit(short: &str, dirty: bool) -> String {
    if dirty {
        format!("{short}-dirty")
    } else {
        short.to_owned()
    }
}

/// Runs git in `directory` and returns its trimmed output, or `None` when git fails or prints
/// nothing.
pub fn git(directory: &Path, args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .current_dir(directory)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// The commit label of the checkout in `directory`, or `None` outside a git checkout. Staged,
/// unstaged and deleted tracked files below [`BUILD_INPUTS`] make it `-dirty`; untracked files and
/// edits elsewhere (documentation, tests, tools) do not change the executable and are ignored.
pub fn describe_checkout(directory: &Path) -> Option<String> {
    let short = git(directory, &["rev-parse", "--short=7", "HEAD"])?;
    let mut status = vec!["status", "--porcelain", "--untracked-files=no", "--"];
    status.extend(BUILD_INPUTS);
    let dirty = git(directory, &status).is_some();
    Some(format_commit(&short, dirty))
}

/// `make`-style freshness: `output` exists and no input is newer than it. A missing input counts
/// as stale so that the step runs and reports the problem itself.
pub fn is_up_to_date(output: &Path, inputs: &[&Path]) -> bool {
    let modified =
        |path: &Path| -> Option<SystemTime> { std::fs::metadata(path).ok()?.modified().ok() };
    let Some(built) = modified(output) else {
        return false;
    };
    inputs
        .iter()
        .all(|input| modified(input).is_some_and(|changed| changed <= built))
}
