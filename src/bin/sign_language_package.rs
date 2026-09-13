//! Offline developer utility; never distributed as part of the installed agent.
#[cfg(windows)]
#[path = "../package_signing_windows.rs"]
mod native;

fn main() {
    #[cfg(windows)]
    if let Err(stage) = native::run() {
        // No input contents, decrypted bytes, or debug representations of keys.
        eprintln!("signing failed: {stage}");
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    {
        eprintln!("sign-language-package requires native Windows and the key owner's profile");
        std::process::exit(1);
    }
}
