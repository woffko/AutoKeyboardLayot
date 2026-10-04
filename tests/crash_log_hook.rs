//! The installed panic hook writes a line to crash.log and never records the panic message.
//!
//! The hook is process-wide, so this binary holds exactly one test that installs it.

#[test]
fn a_panic_is_logged_with_its_location_and_without_its_message() {
    let directory = tempfile::tempdir().unwrap();
    autokeyboardlayot::crash_log::install(directory.path().to_path_buf(), "9.9.9 (test)");

    let secret = "typed-secret-word-hunter2";
    let worker = std::thread::Builder::new()
        .name("crash-test".into())
        .spawn(move || {
            let _ = std::panic::catch_unwind(|| panic!("{secret}"));
            panic!("{secret}");
        })
        .unwrap();
    assert!(worker.join().is_err());

    let log = std::fs::read_to_string(directory.path().join("crash.log")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "one line per panic: {log:?}");
    for line in lines {
        assert!(line.contains(" panic thread=crash-test at "), "{line}");
        assert!(line.contains("crash_log_hook.rs:"), "{line}");
        assert!(line.ends_with(" version=9.9.9 (test)"), "{line}");
        assert!(
            !line.contains("hunter2"),
            "the panic message leaked: {line}"
        );
    }
}
