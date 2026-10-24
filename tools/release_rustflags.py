"""Rust flags for the Windows release executables: the configured flags plus path remapping.

Without remapping, an executable records absolute build-machine paths (the account name, the
checkout, Cargo's registry cache and the Rust toolchain) in its panic locations. The flags
configured for the target in .cargo/config.toml (the static C runtime) are repeated, because
CARGO_ENCODED_RUSTFLAGS replaces them instead of adding to them.

Print the value for a release build:

  CARGO_ENCODED_RUSTFLAGS="$(python3 tools/release_rustflags.py)" cargo xwin build --release ...
"""
import os
from pathlib import Path
import subprocess
import sys
import tomllib

TARGET = 'x86_64-pc-windows-msvc'


def release_rustflags(root, cargo_home, sysroot):
    """The configured target flags, then one remapping per machine path."""
    config = tomllib.loads((root / '.cargo/config.toml').read_text(encoding='utf-8'))
    configured = config['target'][TARGET]['rustflags']
    return [*configured,
            f'--remap-path-prefix={cargo_home}=/cargo',
            f'--remap-path-prefix={sysroot}=/rust',
            f'--remap-path-prefix={root}=.']


def encoded(flags):
    """CARGO_ENCODED_RUSTFLAGS form, which keeps paths with spaces intact."""
    if any('\x1f' in flag for flag in flags):
        raise ValueError('a flag contains the separator')
    return '\x1f'.join(flags)


def machine_paths(root):
    """Cargo's home and the active toolchain's sysroot on this machine."""
    cargo_home = Path(os.environ.get('CARGO_HOME') or Path.home() / '.cargo').resolve()
    sysroot = subprocess.check_output(['rustc', '--print', 'sysroot'], cwd=root, text=True, timeout=60)
    # Resolved like the others: the compiler records the paths it opens, past any symlink.
    return cargo_home, Path(sysroot.strip()).resolve()


def cargo_env(root, environ=None):
    """The environment for a release cargo run of the checkout at `root`: CARGO_ENCODED_RUSTFLAGS set."""
    flags = release_rustflags(root, *machine_paths(root))
    return dict(os.environ if environ is None else environ, CARGO_ENCODED_RUSTFLAGS=encoded(flags))


def leaked_paths(data, paths):
    """The machine paths whose text still occurs in an executable's bytes, as UTF-8 or as the
    UTF-16LE of Windows resources and wide strings."""
    return [path for path in paths
            if str(path).encode('utf-8') in data or str(path).encode('utf-16le') in data]


if __name__ == '__main__':
    repository = Path(__file__).resolve().parents[1]
    sys.stdout.write(encoded(release_rustflags(repository, *machine_paths(repository))))
