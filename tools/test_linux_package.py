import contextlib
import hashlib
import io
import json
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import linux_package
from linux_package import (
    COMMAND, COMPONENT_NAME, GNOME_IBUS_UNIT, PROGRAM, TARGET, TARGET_DIR, check_notice_inputs,
    compiled_artifacts, control_text, copyright_text, desktop_entry, desktop_exec, dropin_text,
    install_user, uninstall_user, user_layout)

# A stand-in for the program: it prints an IBus component for the exec it is given.
FAKE_PROGRAM = '''#!{python}
import sys
if sys.argv[1:2] == ['--print-component']:
    from xml.sax.saxutils import escape
    print('<?xml version="1.0" encoding="utf-8"?><component><name>{name}</name>'
          '<exec>' + escape(sys.argv[2]) + '</exec><engines/></component>')
'''


def artifact(package, kind, name, executable=None):
    return json.dumps({'reason': 'compiler-artifact', 'package_id': package,
                       'target': {'kind': kind, 'name': name}, 'executable': executable})


class Recorder:
    def __init__(self):
        self.calls = []

    def __call__(self, command, env, timeout):
        self.calls.append((command, env))
        return subprocess.CompletedProcess(command, 0)


class CompiledArtifacts(unittest.TestCase):
    def test_every_package_and_the_one_program_are_recorded(self):
        root = Path('/work')
        program = str(root / TARGET_DIR / TARGET / 'release' / PROGRAM)
        stdout = '\n'.join([
            artifact('registry+serde', ['lib'], 'serde'),
            json.dumps({'reason': 'build-script-executed', 'package_id': 'path+app'}),
            artifact('path+app', ['lib'], 'autokeyboardlayot'),
            artifact('path+app', ['bin'], PROGRAM, program),
        ])
        packages, path = compiled_artifacts(stdout, root)
        self.assertEqual(packages, {'registry+serde', 'path+app'})
        self.assertEqual(path, Path(program))

    def test_a_program_elsewhere_twice_or_missing_is_refused(self):
        root = Path('/work')
        program = str(root / TARGET_DIR / TARGET / 'release' / PROGRAM)
        for lines in ([artifact('path+app', ['bin'], PROGRAM, '/elsewhere/' + PROGRAM)],
                      [artifact('path+app', ['bin'], PROGRAM, program)] * 2,
                      [artifact('path+app', ['lib'], 'autokeyboardlayot')]):
            with self.assertRaises(ValueError):
                compiled_artifacts('\n'.join(lines), root)


class PackageTexts(unittest.TestCase):
    def test_desktop_entry_quotes_unusual_paths(self):
        self.assertEqual(desktop_exec('/usr/bin/autokeyboardlayot'), '/usr/bin/autokeyboardlayot')
        self.assertEqual(desktop_exec('/home/a b/.local/bin/x'), '"/home/a b/.local/bin/x"')
        self.assertEqual(desktop_exec('/home/a$b/x'), '"/home/a\\\\$b/x"')
        self.assertEqual(desktop_exec('/home/100%/x'), '"/home/100%%/x"')
        entry = desktop_entry('/usr/bin/autokeyboardlayot')
        self.assertIn('Exec=/usr/bin/autokeyboardlayot --settings\n', entry)
        self.assertIn(f'Icon={COMMAND}\n', entry)

    @unittest.skipUnless(shutil.which('desktop-file-validate'), 'desktop-file-validate is not installed')
    def test_desktop_entry_passes_the_validator(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / f'{COMMAND}.desktop'
            path.write_text(desktop_entry('/home/a b/.local/bin/autokeyboardlayot'), encoding='utf-8')
            result = subprocess.run(['desktop-file-validate', str(path)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(result.stdout, '')

    def test_copyright_carries_the_license_and_marks_blank_lines(self):
        text = copyright_text('MIT License\n\nCopyright (c) 2026 AutoKeyboardLayot contributors\n')
        self.assertTrue(text.startswith('Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n'))
        self.assertIn('\nLicense: MIT\n MIT License\n .\n Copyright (c) 2026 AutoKeyboardLayot contributors\n', text)
        self.assertIn(f'/usr/share/doc/{COMMAND}/THIRD-PARTY-NOTICES.txt', text)

    def test_control_names_the_package_and_every_dependency(self):
        control = control_text('0.1.1-1', 4321, ['libc6 (>= 2.43)', 'ibus (>= 1.5)', 'curl'])
        fields = dict(line.split(': ', 1) for line in control.splitlines() if not line.startswith(' '))
        self.assertEqual(fields['Package'], COMMAND)
        self.assertEqual(fields['Version'], '0.1.1-1')
        self.assertEqual(fields['Architecture'], 'amd64')
        self.assertEqual(fields['Installed-Size'], '4321')
        self.assertEqual(fields['Depends'], 'libc6 (>= 2.43), ibus (>= 1.5), curl')
        self.assertEqual(fields['Recommends'], 'zenity')
        self.assertTrue(control.endswith('settings window.\n'))

    def test_the_package_depends_on_what_the_settings_window_loads_at_run_time(self):
        for package in ('ibus (>= 1.5)', 'curl', 'libegl1', 'libwayland-client0', 'libwayland-egl1',
                        'libxkbcommon-x11-0', 'libx11-6'):
            self.assertIn(package, linux_package.RUNTIME_DEPENDS)


class NoticeInputs(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        (self.root / 'Cargo.toml').write_text('[package]\nversion = "0.1.1"\n', encoding='utf-8')
        (self.root / 'Cargo.lock').write_text('lock', encoding='utf-8')
        self.notices = self.root / 'notices'
        self.notices.mkdir()
        (self.notices / 'THIRD-PARTY-NOTICES.txt').write_text('notices', encoding='utf-8')
        self.reviewed = hashlib.sha256(b'notices').hexdigest()
        self.receipt = {'variant': 'base', 'target': TARGET, 'build': 'package',
                        'cargo_lock_sha256': hashlib.sha256(b'lock').hexdigest(),
                        'cargo_toml_sha256': hashlib.sha256((self.root / 'Cargo.toml').read_bytes()).hexdigest(),
                        'compiled_package_ids': ['path+app']}
        self.write()

    def tearDown(self):
        self.directory.cleanup()

    def write(self, **report_changes):
        raw = json.dumps(self.receipt).encode()
        (self.root / 'receipt.json').write_bytes(raw)
        report = {'complete_text_collection': True, 'missing': [],
                  'cargo_lock_sha256': hashlib.sha256(b'lock').hexdigest(),
                  'build_receipt_sha256': hashlib.sha256(raw).hexdigest(), **report_changes}
        (self.notices / 'notices-report.json').write_text(json.dumps(report), encoding='utf-8')

    def check(self, reviewed=None):
        return check_notice_inputs(self.notices, self.root / 'receipt.json', reviewed or self.reviewed, self.root)

    def test_reviewed_complete_notices_of_this_receipt_are_accepted(self):
        notices, receipt = self.check()
        self.assertEqual(notices, self.notices / 'THIRD-PARTY-NOTICES.txt')
        self.assertEqual(receipt['target'], TARGET)

    def test_anything_incomplete_unreviewed_stale_or_foreign_is_refused(self):
        with self.assertRaises(SystemExit):
            self.check('0' * 64)
        with self.assertRaises(SystemExit):
            self.check(self.reviewed.upper())
        self.write(missing=[{'package': 'x 1.0'}])
        with self.assertRaises(SystemExit):
            self.check()
        self.write(build_receipt_sha256='0' * 64)
        with self.assertRaises(SystemExit):
            self.check()
        # A Windows receipt (no target, a test build) never serves the Linux package.
        del self.receipt['target'], self.receipt['build']
        self.write()
        with self.assertRaises(SystemExit):
            self.check()


class UserInstallation(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        self.home = self.root / 'home'
        self.home.mkdir()
        self.environment = {'HOME': str(self.home), 'PATH': '/usr/bin:/bin', 'IBUS_COMPONENT_PATH': 'stale'}
        self.units = self.root / 'units'
        self.units.mkdir()
        (self.units / GNOME_IBUS_UNIT).write_text('[Service]\n', encoding='utf-8')
        self.program = self.root / 'program'
        self.program.write_text(FAKE_PROGRAM.format(python=sys.executable, name=COMPONENT_NAME), encoding='utf-8')
        self.program.chmod(0o755)

    def tearDown(self):
        self.directory.cleanup()

    def install(self):
        recorder = Recorder()
        with contextlib.redirect_stdout(io.StringIO()):
            install_user(self.program, self.environment, recorder, [self.units], strip=False)
        return recorder

    def test_the_files_point_ibus_at_the_engine_and_ibus_is_not_restarted(self):
        recorder = self.install()
        layout = user_layout(self.environment)
        self.assertEqual(layout['binary'], self.home / '.local/bin' / COMMAND)
        self.assertEqual(stat.S_IMODE(layout['binary'].stat().st_mode), 0o755)
        component = layout['component'].read_text(encoding='utf-8')
        self.assertIn(f'<exec>{layout["binary"]} --ibus</exec>', component)
        self.assertEqual(layout['component'], self.home / '.local/share/ibus/component' / f'{COMMAND}.xml')
        self.assertIn(f'Exec={layout["binary"]} --settings\n', layout['desktop'].read_text(encoding='utf-8'))
        self.assertTrue(layout['icon'].is_file())
        self.assertEqual(layout['dropin'], self.home / '.config/systemd/user' / f'{GNOME_IBUS_UNIT}.d' / f'{COMMAND}.conf')
        dropin = layout['dropin'].read_text(encoding='utf-8')
        path = f'/usr/share/ibus/component:{self.home}/.local/share/ibus/component'
        self.assertIn(f'[Service]\nEnvironment="IBUS_COMPONENT_PATH={path}"\n', dropin)
        self.assertEqual([command for command, _ in recorder.calls],
                         [['systemctl', '--user', 'daemon-reload'], ['ibus', 'write-cache']])
        self.assertEqual(recorder.calls[1][1]['IBUS_COMPONENT_PATH'], path)
        self.assertFalse(any(path.name.startswith('.') for path in layout['binary'].parent.iterdir()))

    def test_a_reinstall_replaces_the_files(self):
        self.install()
        layout = user_layout(self.environment)
        layout['desktop'].write_text('changed', encoding='utf-8')
        self.install()
        self.assertIn('[Desktop Entry]', layout['desktop'].read_text(encoding='utf-8'))

    def test_xdg_directories_are_honoured_only_when_absolute(self):
        self.environment.update({'XDG_DATA_HOME': str(self.root / 'data'), 'XDG_CONFIG_HOME': 'relative'})
        layout = user_layout(self.environment)
        self.assertEqual(layout['component'], self.root / 'data/ibus/component' / f'{COMMAND}.xml')
        self.assertEqual(layout['dropin'].parents[3], self.home / '.config')
        self.assertIn(f'IBUS_COMPONENT_PATH=/usr/share/ibus/component:{self.root}/data/ibus/component',
                      dropin_text(layout))

    def test_percent_signs_are_escaped_in_the_unit_file(self):
        self.environment['HOME'] = '/home/100%'
        self.assertIn('/home/100%%/.local/share/ibus/component"', dropin_text(user_layout(self.environment)))

    def test_without_gnome_ibus_nothing_is_installed(self):
        (self.units / GNOME_IBUS_UNIT).unlink()
        with self.assertRaises(SystemExit):
            self.install()
        self.assertEqual(list(self.home.iterdir()), [])

    def test_uninstall_removes_what_install_wrote_and_keeps_the_profile(self):
        self.install()
        layout = user_layout(self.environment)
        for directory in layout['profile']:
            directory.mkdir(parents=True)
            (directory / 'kept').write_text('kept', encoding='utf-8')
        recorder = Recorder()
        with contextlib.redirect_stdout(io.StringIO()):
            uninstall_user(self.environment, recorder)
        for key in ('binary', 'component', 'desktop', 'icon', 'dropin'):
            self.assertFalse(layout[key].exists(), key)
        self.assertFalse(layout['dropin'].parent.exists())
        self.assertFalse((self.home / '.local/share/ibus').exists())
        for directory in layout['profile']:
            self.assertTrue((directory / 'kept').is_file())
        self.assertNotIn('IBUS_COMPONENT_PATH', recorder.calls[1][1])

    def test_uninstall_leaves_a_unit_file_it_did_not_write(self):
        layout = user_layout(self.environment)
        layout['dropin'].parent.mkdir(parents=True)
        layout['dropin'].write_text('[Service]\nEnvironment=OTHER=1\n', encoding='utf-8')
        with self.assertRaises(SystemExit):
            uninstall_user(self.environment, Recorder())
        self.assertTrue(layout['dropin'].is_file())


if __name__ == '__main__':
    unittest.main()
