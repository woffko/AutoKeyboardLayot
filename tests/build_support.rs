//! Tests for the logic `build.rs` shares through `build_support.rs`: the commit label with its
//! `-dirty` flag and the freshness check that keeps the build script cheap to re-run.

#[path = "../build_support.rs"]
mod build_support;

use std::{
    fs,
    path::Path,
    process::Command,
    time::{Duration, SystemTime},
};

use build_support::{describe_checkout, format_commit, is_up_to_date};

#[test]
fn commit_labels_mark_a_dirty_tree() {
    assert_eq!(format_commit("abc1234", false), "abc1234");
    assert_eq!(format_commit("abc1234", true), "abc1234-dirty");
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn git(directory: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(directory)
        .output()
        .expect("git can be started");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write(directory: &Path, name: &str, contents: &str) {
    let path = directory.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn checkout_label_follows_edits_of_tracked_build_inputs_only() {
    if !git_available() {
        eprintln!("git is not installed; skipping");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    git(root, &["init", "-q"]);
    write(root, "src/lib.rs", "fn main() {}\n");
    write(root, "README.md", "readme\n");
    git(root, &["add", "src/lib.rs", "README.md"]);
    git(root, &["commit", "-q", "-m", "initial"]);

    let clean = describe_checkout(root).expect("a checkout with a commit has a label");
    assert_eq!(clean.len(), 7, "{clean}");
    assert!(clean.chars().all(|c| c.is_ascii_hexdigit()), "{clean}");

    // An edit of a tracked build input marks the build, and reverting it clears the mark.
    write(root, "src/lib.rs", "fn main() { changed(); }\n");
    assert_eq!(describe_checkout(root), Some(format!("{clean}-dirty")));
    git(root, &["checkout", "--", "src/lib.rs"]);
    assert_eq!(describe_checkout(root), Some(clean.clone()));

    // Staged changes count too.
    write(root, "src/lib.rs", "fn main() { staged(); }\n");
    git(root, &["add", "src/lib.rs"]);
    assert_eq!(describe_checkout(root), Some(format!("{clean}-dirty")));
    git(root, &["reset", "-q", "--hard"]);

    // A deleted tracked input counts.
    fs::remove_file(root.join("src/lib.rs")).unwrap();
    assert_eq!(describe_checkout(root), Some(format!("{clean}-dirty")));
    git(root, &["checkout", "--", "src/lib.rs"]);

    // Untracked files and edits outside the build inputs do not change the executable.
    write(root, "src/new_file.rs", "// untracked\n");
    write(root, "README.md", "edited readme\n");
    assert_eq!(describe_checkout(root), Some(clean));
}

#[test]
fn a_directory_that_is_not_a_checkout_has_no_label() {
    if !git_available() {
        eprintln!("git is not installed; skipping");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(describe_checkout(directory.path()), None);
}

fn set_modified(path: &Path, modified: SystemTime) {
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
}

#[test]
fn freshness_needs_an_output_not_older_than_every_input() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let (output, first, second) = (
        root.join("out.fst"),
        root.join("words.txt"),
        root.join("build.rs"),
    );
    fs::write(&first, "a").unwrap();
    fs::write(&second, "b").unwrap();
    let base = SystemTime::now();
    let inputs = [first.as_path(), second.as_path()];

    assert!(
        !is_up_to_date(&output, &inputs),
        "a missing output is stale"
    );

    fs::write(&output, "fst").unwrap();
    set_modified(&first, base - Duration::from_secs(20));
    set_modified(&second, base - Duration::from_secs(10));
    set_modified(&output, base);
    assert!(is_up_to_date(&output, &inputs));

    set_modified(&second, base + Duration::from_secs(5));
    assert!(!is_up_to_date(&output, &inputs), "a newer input is stale");

    set_modified(&second, base - Duration::from_secs(10));
    fs::remove_file(&first).unwrap();
    assert!(!is_up_to_date(&output, &inputs), "a missing input is stale");
}
