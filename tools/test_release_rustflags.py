import os
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest import mock

import release_rustflags as module
from release_rustflags import encoded, leaked_paths, release_rustflags

ROOT = Path(__file__).resolve().parents[1]


class ReleaseRustflagsTests(unittest.TestCase):
    def test_the_configured_flags_come_first_and_every_machine_path_is_remapped(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / 'checkout with space'
            (root / '.cargo').mkdir(parents=True)
            (root / '.cargo/config.toml').write_text(
                '[target.x86_64-pc-windows-msvc]\nrustflags = ["-C", "target-feature=+crt-static"]\n',
                encoding='utf-8')
            flags = release_rustflags(root, Path('/srv/builder/.cargo'), Path('/opt/toolchain'))
        self.assertEqual(flags, [
            '-C', 'target-feature=+crt-static',
            '--remap-path-prefix=/srv/builder/.cargo=/cargo',
            '--remap-path-prefix=/opt/toolchain=/rust',
            f'--remap-path-prefix={root}=.',
        ])
        self.assertEqual(encoded(flags).split('\x1f'), flags)
        with self.assertRaises(ValueError):
            encoded(['-C', 'bad\x1fflag'])

    def test_the_release_flags_keep_the_static_c_runtime_of_the_repository(self):
        configured = tomllib.loads((ROOT / '.cargo/config.toml').read_text(encoding='utf-8'))
        flags = release_rustflags(ROOT, Path('/c'), Path('/s'))
        self.assertIn('target-feature=+crt-static', flags)
        self.assertEqual(flags[:-3], configured['target']['x86_64-pc-windows-msvc']['rustflags'])

    def test_a_machine_path_left_in_an_executable_is_reported(self):
        paths = [Path('/srv/builder/.cargo'), Path('/srv/builder/checkout')]
        clean = b'\x00/cargo/registry/src/fst-0.4.7/src/raw.rs\x00./src/main.rs\x00'
        self.assertEqual(leaked_paths(clean, paths), [])
        self.assertEqual(leaked_paths(clean + b'/srv/builder/checkout/src/lib.rs', paths), [paths[1]])
        # Windows resources and wide strings hold text as UTF-16LE.
        wide = '/srv/builder/.cargo/registry'.encode('utf-16le')
        self.assertEqual(leaked_paths(clean + wide, paths), [paths[0]])

    def test_the_machine_paths_are_the_resolved_ones_the_compiler_sees(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory).resolve()
            (base / 'toolchain').mkdir()
            (base / 'cargo').mkdir()
            (base / 'via-link').symlink_to(base / 'toolchain')
            (base / 'cargo-link').symlink_to(base / 'cargo')
            bin_directory = base / 'bin'
            bin_directory.mkdir()
            rustc = bin_directory / 'rustc'
            rustc.write_text(f'#!/bin/sh\necho {base / "via-link"}\n', encoding='utf-8')
            rustc.chmod(0o755)
            environment = {'PATH': f'{bin_directory}{os.pathsep}{os.environ["PATH"]}',
                           'CARGO_HOME': str(base / 'cargo-link')}
            with mock.patch.dict(os.environ, environment):
                cargo_home, sysroot = module.machine_paths(base)
            self.assertEqual(sysroot, base / 'toolchain')
            self.assertEqual(cargo_home, base / 'cargo')
            (base / '.cargo').mkdir()
            (base / '.cargo/config.toml').write_text(
                '[target.x86_64-pc-windows-msvc]\nrustflags = ["-C", "target-feature=+crt-static"]\n', encoding='utf-8')
            with mock.patch.dict(os.environ, environment):
                built = module.cargo_env(base)
            self.assertEqual(built['CARGO_ENCODED_RUSTFLAGS'].split('\x1f'),
                             release_rustflags(base, base / 'cargo', base / 'toolchain'))
            self.assertEqual(built['PATH'], environment['PATH'])


class InstallerBuildWiringTests(unittest.TestCase):
    """The installer build cannot run here; its text is checked like in test_authenticode_hook."""

    def setUp(self):
        self.text = (ROOT / 'tools/build_experimental_installer.py').read_text(encoding='utf-8')

    def position(self, fragment):
        self.assertEqual(self.text.count(fragment), 1, fragment)
        return self.text.index(fragment)

    def test_both_cargo_runs_remap_paths_and_the_outputs_are_checked_before_use(self):
        environment = self.position('cargo_env = dict(os.environ, CARGO_ENCODED_RUSTFLAGS=')
        clippy = self.position("subprocess.run([*cargo, 'clippy'")
        build = self.position("build = subprocess.run([*cargo, 'build'")
        self.assertEqual(self.text.count('env=cargo_env)'), 2)
        leak = self.position('release_rustflags.leaked_paths(path.read_bytes(), [cargo_home, sysroot, root])')
        self.assertLess(environment, clippy)
        self.assertLess(clippy, build)
        self.assertLess(build, leak)
        self.assertLess(leak, self.position('if signing:\n    # Sign copies'))


class TestArtifactBuildWiringTests(unittest.TestCase):
    def test_the_test_executables_are_built_with_the_release_flags(self):
        text = (ROOT / 'tools/export_windows_test_artifacts.py').read_text(encoding='utf-8')
        self.assertIn('import release_rustflags', text)
        self.assertEqual(text.count('env=release_rustflags.cargo_env(root)'), 1)
        self.assertIn('result = subprocess.run(command, cwd=root, stdout=subprocess.PIPE, timeout=1200, check=True,\n'
                      '                        env=release_rustflags.cargo_env(root))', text)


if __name__ == '__main__':
    unittest.main()
