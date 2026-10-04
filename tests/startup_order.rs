//! The Windows agent is not compiled on other systems, but its source is plain
//! text: this keeps the order of the first steps of `run()` from drifting.

const AGENT: &str = include_str!("../src/windows_agent.rs");

/// The text of `pub fn run() -> Result<()>` up to its closing brace.
fn run_function() -> &'static str {
    let start = AGENT
        .find("\npub fn run() -> Result<()> {")
        .expect("pub fn run");
    let body = &AGENT[start + 1..];
    let end = body.find("\n}\n").expect("end of run");
    &body[..end]
}

#[test]
fn a_second_instance_leaves_before_it_reads_the_configuration() {
    let run = run_function();
    let guard = run
        .find("InstanceGuard::acquire()")
        .expect("run() takes the single-instance guard");
    for later in [
        "load_runtime_configuration()",
        "ui_localization::initialize(",
    ] {
        let position = run
            .find(later)
            .unwrap_or_else(|| panic!("run() no longer calls {later}"));
        assert!(
            guard < position,
            "InstanceGuard::acquire() must come before {later}: a second instance \
             must exit without reading the configuration or the installed packages"
        );
    }
}
