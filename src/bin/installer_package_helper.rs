#[cfg(windows)]
#[path = "../installer_helper_windows.rs"]
mod native;
fn main() {
    #[cfg(windows)]
    if native::run().is_err() {
        eprintln!("installer helper failed; inspect session status, do not replay approvals");
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    {
        eprintln!("installer-package-helper requires Windows");
        std::process::exit(1);
    }
}
