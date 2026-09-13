//! Explicit Windows-only diagnostic for isolated VM acceptance. No typed text,
//! window titles, process paths or persisted user configuration are inspected.

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use autokeyboardlayot::windows_input_profiles::spawn_keyboard_profile_resolver;
    use std::time::Duration;
    use windows::Win32::UI::{
        Input::KeyboardAndMouse::GetKeyboardLayout,
        WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId},
    };
    // Run idle: user switching focus/layout during the check invalidates it.
    let own_before = unsafe { GetKeyboardLayout(0) };
    let foreground = unsafe { GetForegroundWindow() };
    let input_thread = unsafe { GetWindowThreadProcessId(foreground, None) };
    if foreground.0.is_null() || input_thread == 0 {
        return Err(std::io::Error::other("interactive foreground required").into());
    }
    let foreground_before = unsafe { GetKeyboardLayout(input_thread) };
    let mut resolver = spawn_keyboard_profile_resolver()?;
    let resolved = resolver
        .query_result((), Duration::from_secs(2), Duration::from_secs(3))
        .map_err(|failure| {
            std::io::Error::other(format!("profile probe transport: {failure:?}"))
        })??;
    if unsafe { GetKeyboardLayout(0) } != own_before
        || unsafe { GetForegroundWindow() } != foreground
        || unsafe { GetWindowThreadProcessId(foreground, None) } != input_thread
        || unsafe { GetKeyboardLayout(input_thread) } != foreground_before
    {
        return Err(
            std::io::Error::other("caller or foreground changed during profile probe").into(),
        );
    }
    let mut count = 0;
    for handle in resolved.loaded_layouts() {
        if let Some(profile) = resolved.profile(handle) {
            println!(
                "profile={profile} unique={}",
                resolved.unique_layout(profile).is_some()
            );
            count += 1;
        }
    }
    println!("resolved={count} caller_layout_preserved=true foreground_snapshot_preserved=true");
    // Endpoint observations do not prove absence of transient switches/events.
    // ETW/window-message and live typing checks are separate acceptance gates.
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("resolve_keyboard_profiles requires an interactive Windows test session");
    std::process::exit(2);
}
