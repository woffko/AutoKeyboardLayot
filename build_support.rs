//! Helpers shared by `build.rs` and `tests/build_support.rs`.
//!
//! A build script cannot be unit-tested on its own, so the logic that can go wrong lives here and
//! both crates include this file with `#[path]`. Each of them uses a different subset.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// Variables that point git at another repository, work tree, index or object store. A build
/// started from a git hook inherits them; the label must still describe the checkout it builds.
pub const GIT_LOCATION_VARIABLES: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
];

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

/// git in `directory`, without the inherited [`GIT_LOCATION_VARIABLES`].
pub fn git_command(directory: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(directory);
    for variable in GIT_LOCATION_VARIABLES {
        command.env_remove(variable);
    }
    command
}

/// Runs git in `directory` and returns its trimmed output, or `None` when git fails or prints
/// nothing.
pub fn git(directory: &Path, args: &[&str]) -> Option<String> {
    git_with(directory, args, &|_| {})
}

/// [`git`] with `setup` applied to the command first; tests isolate git from the user's
/// configuration this way.
pub fn git_with(directory: &Path, args: &[&str], setup: &dyn Fn(&mut Command)) -> Option<String> {
    let mut command = git_command(directory);
    setup(&mut command);
    command
        .args(args)
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
    describe_checkout_with(directory, &|_| {})
}

/// [`describe_checkout`] with `setup` applied to every git command.
pub fn describe_checkout_with(directory: &Path, setup: &dyn Fn(&mut Command)) -> Option<String> {
    let short = git_with(directory, &["rev-parse", "--short=7", "HEAD"], setup)?;
    let mut status = vec!["status", "--porcelain", "--untracked-files=no", "--"];
    status.extend(BUILD_INPUTS);
    let dirty = git_with(directory, &status, setup).is_some();
    Some(format_commit(&short, dirty))
}

/// The files whose change moves `HEAD` of the checkout in `directory`, for
/// `cargo:rerun-if-changed`: `HEAD`, the current branch's loose ref and `packed-refs`, each only
/// when it exists, since Cargo re-runs a build script on every build for a path that does not.
/// When the branch has no loose ref (packed, or not born yet) or nothing is packed yet, the
/// `refs/heads` directory stands in: the next commit or pack writes the loose ref there.
pub fn watched_git_paths(directory: &Path, setup: &dyn Fn(&mut Command)) -> Vec<PathBuf> {
    let path = |name: &str| {
        git_with(directory, &["rev-parse", "--git-path", name], setup)
            .map(|path| directory.join(path))
    };
    let mut names = vec!["HEAD".to_owned()];
    names.extend(git_with(directory, &["symbolic-ref", "-q", "HEAD"], setup));
    names.push("packed-refs".to_owned());
    let mut watched = Vec::new();
    let mut watch_heads = false;
    for name in names {
        let Some(found) = path(&name) else {
            continue;
        };
        if found.exists() {
            watched.push(found);
        } else if name != "HEAD" {
            // The branch not loose (packed, or not born yet), or nothing packed yet: the next
            // commit or pack writes its loose ref under refs/heads, touching no path above.
            watch_heads = true;
        }
    }
    if watch_heads
        && let Some(heads) = path("refs/heads")
        && heads.is_dir()
    {
        watched.push(heads);
    }
    watched
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
