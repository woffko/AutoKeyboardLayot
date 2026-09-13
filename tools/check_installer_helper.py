"""Compile and lint the helper plus fixture initializer; no native execution."""
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parents[1]
common = ["--locked", "--offline", "--release", "--no-default-features",
          "--features", "installer-tools", "--bin", "installer-package-helper",
          "--example", "initialize_package_store", "--target", "x86_64-pc-windows-msvc",
          "--target-dir", "target/xwin-hotkeys", "--jobs", "2"]
for stage in ("clippy", "build"):
    command = ["cargo", "xwin", stage, *common]
    if stage == "clippy":
        command += ["--", "-D", "warnings"]
    subprocess.run(command, cwd=root, timeout=1800, check=True)
print("INSTALLER_HELPER_BUILD_CHECKS_PASSED", flush=True)
