//! Tests for the logic `build.rs` shares through `build_support.rs`: the commit label with its
//! `-dirty` flag and the freshness check that keeps the build script cheap to re-run.

#[path = "../build_support.rs"]
mod build_support;

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime},
};

use build_support::{
    GIT_LOCATION_VARIABLES, describe_checkout_with, format_commit, git_command, is_up_to_date,
    watched_git_paths,
};

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

/// Git that gives the same answer on every machine: no global or system configuration, no hooks,
/// and no checkout above the test's own directory.
struct Isolation {
    home: tempfile::TempDir,
    ceiling: PathBuf,
}

impl Isolation {
    fn around(root: &Path) -> Self {
        let home = tempfile::tempdir().unwrap();
        fs::write(home.path().join("gitconfig"), "").unwrap();
        Self {
            home,
            ceiling: root.parent().unwrap().to_path_buf(),
        }
    }

    fn apply(&self, command: &mut Command) {
        command
            .env("GIT_CONFIG_GLOBAL", self.home.path().join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CEILING_DIRECTORIES", &self.ceiling);
    }
}

fn git(isolation: &Isolation, directory: &Path, args: &[&str]) {
    let mut command = git_command(directory);
    isolation.apply(&mut command);
    let hooks = format!(
        "core.hooksPath={}",
        isolation.home.path().join("no-hooks").display()
    );
    let output = command
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            &hooks,
        ])
        .args(args)
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
    let isolation = Isolation::around(root);
    let describe_checkout =
        |root: &Path| describe_checkout_with(root, &|command| isolation.apply(command));
    let git = |root: &Path, args: &[&str]| git(&isolation, root, args);
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
    let isolation = Isolation::around(directory.path());
    assert_eq!(
        describe_checkout_with(directory.path(), &|command| isolation.apply(command)),
        None
    );
}

#[test]
fn git_drops_inherited_variables_that_point_at_another_repository() {
    let command = git_command(Path::new("."));
    let removed: Vec<String> = command
        .get_envs()
        .filter(|(_, value)| value.is_none())
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect();
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
    ] {
        assert!(GIT_LOCATION_VARIABLES.contains(&variable), "{variable}");
        assert!(
            removed.iter().any(|name| name == variable),
            "{variable} is kept"
        );
    }
}

#[test]
fn the_build_script_watches_only_git_paths_that_exist() {
    if !git_available() {
        eprintln!("git is not installed; skipping");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let isolation = Isolation::around(root);
    let watched = || watched_git_paths(root, &|command| isolation.apply(command));
    git(&isolation, root, &["init", "-q", "-b", "trunk"]);
    write(root, "src/lib.rs", "fn main() {}\n");
    git(&isolation, root, &["add", "src/lib.rs"]);
    git(&isolation, root, &["commit", "-q", "-m", "initial"]);
    let git_dir = root.join(".git");

    // A fresh checkout has its branch as a loose ref and no packed-refs yet: the refs/heads
    // directory stands in, where packing the branch will show.
    let loose = watched();
    assert!(loose.iter().all(|path| path.exists()), "{loose:?}");
    let canonical = |paths: &[PathBuf]| -> Vec<PathBuf> {
        paths
            .iter()
            .map(|path| fs::canonicalize(path).unwrap())
            .collect()
    };
    let expected_loose = canonical(&[
        git_dir.join("HEAD"),
        git_dir.join("refs/heads/trunk"),
        git_dir.join("refs/heads"),
    ]);
    assert_eq!(canonical(&loose), expected_loose);

    // Packed, the branch lives in packed-refs.
    git(&isolation, root, &["pack-refs", "--all"]);
    let packed = watched();
    assert!(packed.iter().all(|path| path.exists()), "{packed:?}");
    assert_eq!(
        canonical(&packed),
        canonical(&[git_dir.join("HEAD"), git_dir.join("packed-refs")])
    );
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
