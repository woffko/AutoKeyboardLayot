#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "windows")]
mod windows_agent;

#[cfg(target_os = "windows")]
fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments
        .first()
        .is_some_and(|argument| argument == "--verify-profile")
    {
        // Read-only startup preflight: no hook, window, repair or migration.
        std::process::exit(if arguments.len() != 1 {
            10
        } else {
            windows_agent::verify_profile()
        });
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "--verify-modular-base")
    {
        std::process::exit(if arguments.len() != 1 {
            10
        } else if cfg!(feature = "legacy-bundled-input") {
            23
        } else {
            0
        });
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "--initialize-installation")
    {
        std::process::exit(if arguments.len() == 1 {
            windows_agent::initialize_installation()
        } else {
            10
        });
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "--prepare-upgrade")
    {
        let code = if arguments.len() == 2 {
            windows_agent::prepare_upgrade(std::path::Path::new(&arguments[1]))
        } else {
            10
        };
        std::process::exit(code);
    }
    if std::env::args_os().any(|argument| argument == "--settings") {
        if let Err(error) = windows_agent::run_settings() {
            windows_agent::show_fatal_error(&error);
        }
        return;
    }
    if let Err(error) = windows_agent::run() {
        windows_agent::show_fatal_error(&error.to_string());
    }
}

#[cfg(not(target_os = "windows"))]
fn main() {
    println!("AutoKeyboardLayot is a Windows user-session agent.");
}
