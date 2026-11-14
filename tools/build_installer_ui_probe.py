"""Build an installation-disabled UI probe, never launch or publish it.

Usage: build_installer_ui_probe.py --output target/NEW_DIRECTORY --iscc PATH_TO_ISCC --notices NOTICES_FILE
The notices file may be an incomplete one: the probe installs nothing.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tomllib

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', required=True, help='A new child directory of target/')
parser.add_argument('--iscc', required=True, help='Path to the reviewed Inno Setup compiler')
parser.add_argument('--notices', required=True, help='Third-party notices file to bundle')
args = parser.parse_args()
compiler = Path(args.iscc)
notices = (root / args.notices).resolve()
for path, what in ((compiler, 'Inno Setup compiler'), (notices, 'notices file')):
    if not path.is_file():
        raise SystemExit(f'The {what} {path} does not exist.')
output = (root / args.output).resolve()
if output == root / 'target' or not output.is_relative_to(root / 'target'):
    raise SystemExit('Output must be a fresh child of this project target directory.')
output.mkdir()  # new receipt/artifact directory; never overwrite a prior probe
common = ["--locked", "--offline", "--release", "--no-default-features",
          "--features", "installer-tools", "--bins", "--target", "x86_64-pc-windows-msvc",
          "--target-dir", "target/xwin-hotkeys", "--jobs", "2"]
for stage in ("clippy", "build"):
    command = ["cargo", "xwin", stage, *common]
    if stage == "clippy":
        command += ["--", "-D", "warnings"]
    subprocess.run(command, cwd=root, timeout=1800, check=True)

def windows(path):
    return subprocess.check_output(["wslpath", "-w", str(path)], text=True).strip()

release = root / "target/xwin-hotkeys/x86_64-pc-windows-msvc/release"
app = release / "AutoKeyboardLayot.exe"
helper = release / "installer-package-helper.exe"
version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
command = [str(compiler), "/DInstallerUiProbe", "/DAppVersion=" + version,
           "/DAppExecutable=" + windows(app), "/DPackageHelper=" + windows(helper),
           "/DBundleNotices=" + windows(notices), "/O" + windows(output),
           windows(root / "installer/AutoKeyboardLayot.iss")]
subprocess.run(command, cwd=root, timeout=1200, check=True)
artifacts = list(output.glob("*.exe"))
if len(artifacts) != 1 or "ui-probe-NOT-FOR-INSTALLATION" not in artifacts[0].name:
    raise SystemExit("Unexpected UI probe artifact.")
receipt = {"state": "built_not_executed", "installation_disabled": True,
           "notices_complete": False, "published": False, "ui_acceptance": False,
           "artifacts": [{"file": str(path.relative_to(root)), "bytes": path.stat().st_size,
                          "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                         for path in [app, helper, artifacts[0]]]}
with (output / "build.json").open("x", encoding="utf-8") as stream:
    json.dump(receipt, stream, indent=2)
print("INSTALLATION_DISABLED_UI_PROBE_BUILT_NOT_EXECUTED", flush=True)
