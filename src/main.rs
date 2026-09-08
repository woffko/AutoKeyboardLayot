#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "windows")]
mod windows_agent;

#[cfg(target_os = "windows")]
fn main() {
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
