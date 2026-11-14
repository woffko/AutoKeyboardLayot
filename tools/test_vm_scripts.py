"""The VM acceptance scripts take everything about the machine from the environment.

The settings layer is tested directly. The three drivers run against a fake ssh and sftp
that record every call, so the tests can see which login, host key name, staging
directory and expected computer reach the VM. Nothing here contacts a machine.
"""
import base64
import contextlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import types
import unittest
from pathlib import Path
from unittest import mock

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import vm_config  # noqa: E402
from vm_config import VmSettings, VmSettingsError, sftp_path  # noqa: E402

DRIVERS = ['run_vm_cross_session.py', 'run_vm_installer_ui.py', 'run_vm_upgrade.py']
SCRIPTS = DRIVERS + ['test-vm-cross-session.ps1', 'test-vm-upgrade.ps1', 'test-installer-ui.ps1']
# The build directory (and scenarios) the drivers that take them get in these tests.
BUILD = 'target/installer-build-a'
# A build whose program was signed: the receipt names it below signed-inputs/.
SIGNED_BUILD = 'target/installer-build-b'
DRIVER_ARGUMENTS = {'run_vm_cross_session.py': (SIGNED_BUILD,), 'run_vm_installer_ui.py': (BUILD, 'retry', 'missing-exe'),
                    'run_vm_upgrade.py': (BUILD, SIGNED_BUILD)}
# The receipts' digests: setup and program of BUILD, then of SIGNED_BUILD.
DIGESTS = {'a-setup': 'a' * 64, 'a-program': 'b' * 64, 'b-setup': 'c' * 64, 'b-program': 'd' * 64}
# Values that tie a script to one machine or one person. Written in pieces so that a scan for
# these values does not flag this file.
MACHINE_LOCAL = ['192' + '.168.', 'C:\\Users\\', '/ho' + 'me/', 'root' + '@', 'DESK' + 'TOP-', 'w0' + 'w']
# Tracked tools that may hold one of them, with the reason. Every other tracked file below tools/
# must hold none.
MACHINE_LOCAL_EXCEPTIONS = {
    ('test_deploy_host_script.py', '/ho' + 'me/'): "the deploy script's own hygiene pattern",
}
ROOT = TOOLS.parent

ENVIRONMENT = {
    'AKL_VM_HOST': 'vm.test.invalid',
    'AKL_VM_USER': 'tester',
    'AKL_VM_COMPUTER_NAME': 'TESTGUEST01',
}


class SettingsTests(unittest.TestCase):
    def test_required_variables_are_named_when_missing(self):
        for name in ENVIRONMENT:
            env = {key: value for key, value in ENVIRONMENT.items() if key != name}
            with self.assertRaises(VmSettingsError) as caught:
                VmSettings.from_environment(env)
            self.assertIn(name, str(caught.exception))

    def test_defaults_and_overrides(self):
        settings = VmSettings.from_environment(ENVIRONMENT)
        self.assertEqual(settings.login, 'tester@vm.test.invalid')
        self.assertEqual(settings.host_key_alias, 'vm.test.invalid')
        self.assertEqual(settings.test_user, 'tester')
        self.assertEqual(settings.scheduled_task_user, 'TESTGUEST01\\tester')
        env = dict(ENVIRONMENT, AKL_VM_HOST_KEY_ALIAS='pinned-name', AKL_VM_TEST_USER='console',
                   AKL_VM_LOCALAPPDATA='D:\\Profiles\\console\\AppData\\Local', AKL_VM_STAGE='D:\\Stage\\akl')
        settings = VmSettings.from_environment(env)
        self.assertEqual(settings.host_key_option(), ['-o', 'HostKeyAlias=pinned-name'])
        self.assertEqual(settings.scheduled_task_user, 'TESTGUEST01\\console')
        self.assertEqual(settings.stage_directory('akl-upgrade', ''), 'D:\\Stage\\akl')

    def test_another_tested_user_needs_its_profile_and_a_stage_from_the_settings(self):
        other = dict(ENVIRONMENT, AKL_VM_TEST_USER='console')
        complete = dict(other, AKL_VM_LOCALAPPDATA='D:\\Profiles\\console\\AppData\\Local',
                        AKL_VM_STAGE='D:\\Stage\\akl')
        for missing in ['AKL_VM_LOCALAPPDATA', 'AKL_VM_STAGE']:
            env = {key: value for key, value in complete.items() if key != missing}
            with self.assertRaises(VmSettingsError) as caught:
                VmSettings.from_environment(env)
            self.assertIn(missing, str(caught.exception))
        self.assertEqual(VmSettings.from_environment(complete).test_user, 'console')
        # The same account in another case is the same Windows user.
        self.assertEqual(VmSettings.from_environment(dict(ENVIRONMENT, AKL_VM_TEST_USER='TESTER')).test_user, 'TESTER')

    def test_unusable_values_are_refused_before_they_can_reach_a_command_line(self):
        bad = {
            'AKL_VM_HOST': ['-oProxyCommand=x', 'host name', 'host;reboot', '$(id)', 'a' * 300, '-host'],
            'AKL_VM_USER': ['-x', 'a b', 'a@b', 'root;x', ''],
            'AKL_VM_COMPUTER_NAME': ['a b', 'x;y', "x'y", '-x'],
            'AKL_VM_LOCALAPPDATA': ['relative\\path', "C:\\a'b", 'C:\\a"b', 'C:\\a;b', 'C:\\a`b', 'C:\\a$b', 'C:\\a\nb'],
            'AKL_VM_STAGE': ['stage', 'C:\\x" -o y', "C:\\x'; calc; '"],
        }
        for name, values in bad.items():
            for value in values:
                if name in ENVIRONMENT and not value:
                    continue  # an empty required value is reported as missing, which is also an error
                with self.assertRaises(VmSettingsError, msg=f'{name}={value!r}'):
                    VmSettings.from_environment(dict(ENVIRONMENT, **{name: value}))

    def test_stage_and_profile_come_from_the_setting_or_from_what_the_vm_reports(self):
        reported = 'COMPUTER=TESTGUEST01\nLOCALAPPDATA=E:\\Users\\console\\AppData\\Local\nAPP_PROC=0\n'
        settings = VmSettings.from_environment(ENVIRONMENT)
        self.assertEqual(settings.stage_directory('akl-upgrade', reported),
                         'E:\\Users\\console\\AppData\\Local\\Temp\\akl-upgrade')
        with self.assertRaises(VmSettingsError):
            settings.stage_directory('akl-upgrade', 'COMPUTER=TESTGUEST01\n')
        with self.assertRaises(VmSettingsError):
            settings.stage_directory('akl-upgrade', 'LOCALAPPDATA=relative\n')
        pinned = VmSettings.from_environment(dict(ENVIRONMENT, AKL_VM_LOCALAPPDATA='F:\\L'))
        self.assertEqual(pinned.stage_directory('akl-ui-offline', ''), 'F:\\L\\Temp\\akl-ui-offline')
        self.assertEqual(sftp_path('F:\\L\\Temp\\x'), '/F:/L/Temp/x')

    def test_preflight_accepts_only_a_clean_expected_machine(self):
        settings = VmSettings.from_environment(ENVIRONMENT)
        clean = 'COMPUTER=TESTGUEST01\nAPP_PROC=0\nPROFILE=False\nSESSIONS=console 1 Active\n'
        self.assertTrue(settings.preflight_ok(clean))
        for broken in [clean.replace('TESTGUEST01', 'OTHERBOX'), clean.replace('APP_PROC=0', 'APP_PROC=1'),
                       clean.replace('PROFILE=False', 'PROFILE=True'), clean.replace('Active', 'Disconnected'),
                       clean.replace('console', 'rdp-tcp'), clean.replace('COMPUTER=TESTGUEST01', 'COMPUTER=TESTGUEST011')]:
            self.assertFalse(settings.preflight_ok(broken), broken)

    def test_the_preflight_script_looks_in_the_tested_users_profile(self):
        self.assertIn("Join-Path $env:LOCALAPPDATA 'AutoKeyboardLayot'",
                      VmSettings.from_environment(ENVIRONMENT).preflight_script())
        pinned = VmSettings.from_environment(dict(ENVIRONMENT, AKL_VM_LOCALAPPDATA='F:\\L'))
        self.assertIn("Join-Path 'F:\\L' 'AutoKeyboardLayot'", pinned.preflight_script())


class HygieneTests(unittest.TestCase):
    def test_no_tracked_tool_names_a_machine_or_a_person(self):
        found = set()
        for name in tracked_tools():
            text = (TOOLS / name).read_bytes().decode('utf-8', 'replace')
            for marker in MACHINE_LOCAL:
                if marker in text:
                    found.add((name, marker))
        unexpected = sorted(found - MACHINE_LOCAL_EXCEPTIONS.keys())
        self.assertEqual(unexpected, [], 'tracked tools name a machine or a person')
        self.assertEqual(sorted(MACHINE_LOCAL_EXCEPTIONS.keys() - found), [], 'stale exceptions')

    def test_the_ui_probe_builder_checks_its_compiler_and_notices_before_building(self):
        with tempfile.TemporaryDirectory() as scratch:
            # A cargo that records being started: nothing may be built with a wrong argument.
            stub = Path(scratch) / 'bin'
            stub.mkdir()
            started = Path(scratch) / 'cargo-started'
            (stub / 'cargo').write_text(f'#!/bin/sh\ntouch {started}\nexit 1\n')
            (stub / 'cargo').chmod(0o755)
            env = dict(os.environ, PATH=f'{stub}{os.pathsep}{os.environ.get("PATH", "")}')
            output = f'target/ui-probe-argument-test-{os.getpid()}'
            self.addCleanup(shutil.rmtree, ROOT / output, ignore_errors=True)
            missing = str(Path(scratch) / 'missing' / 'ISCC.exe')
            notices = Path(scratch) / 'NOTICES.txt'
            notices.write_text('notices')
            runs = {
                'no compiler': ['--output', output, '--notices', str(notices)],
                'missing compiler': ['--output', output, '--iscc', missing, '--notices', str(notices)],
                'missing notices': ['--output', output, '--iscc', str(notices),
                                    '--notices', str(Path(scratch) / 'missing.txt')],
            }
            for case, arguments in runs.items():
                run = subprocess.run([sys.executable, str(TOOLS / 'build_installer_ui_probe.py'), *arguments],
                                     cwd=ROOT, env=env, capture_output=True, text=True, timeout=60)
                self.assertNotEqual(run.returncode, 0, case)
                self.assertFalse(started.exists(), f'{case}: cargo was started')
                self.assertFalse((ROOT / output).exists(), f'{case}: the output was created')

    def test_every_script_still_compiles(self):
        for name in DRIVERS + ['vm_config.py']:
            compile((TOOLS / name).read_text(encoding='utf-8'), name, 'exec')

    def test_the_guest_scripts_demand_the_machine_and_the_profile(self):
        for name in ['test-vm-cross-session.ps1', 'test-vm-upgrade.ps1']:
            text = (TOOLS / name).read_text(encoding='utf-8')
            self.assertIn('[Parameter(Mandatory=$true)][string]$ExpectedComputer', text, name)
            self.assertIn('[Parameter(Mandatory=$true)][string]$LocalAppData', text, name)
            self.assertIn('$env:COMPUTERNAME -ne $ExpectedComputer', text, name)


def tracked_tools():
    """The names below tools/ that git tracks (every file there outside a checkout)."""
    try:
        listed = subprocess.run(['git', '-C', str(ROOT), 'ls-files', '-z', '--', 'tools'],
                                capture_output=True, check=True).stdout.decode('utf-8')
        names = [path[len('tools/'):] for path in listed.split('\0') if path]
    except (OSError, subprocess.CalledProcessError):
        names = [str(path.relative_to(TOOLS)) for path in TOOLS.rglob('*')
                 if path.is_file() and '__pycache__' not in path.parts]
    if not names:
        raise AssertionError('no tracked tools found')
    return names


class FakeVm:
    """Stands in for /usr/bin/ssh and /usr/bin/sftp and answers like a clean VM."""

    def __init__(self, computer='TESTGUEST01', local_app_data='E:\\Users\\console\\AppData\\Local'):
        self.computer = computer
        self.local_app_data = local_app_data
        self.ssh = []      # (argv, decoded PowerShell)
        self.sftp = []     # (argv, batch text)

    def run(self, argv, **kwargs):
        stdout = b''
        if argv[0].endswith('ssh'):
            encoded = argv[-1].split('-EncodedCommand ')[1]
            script = base64.b64decode(encoded).decode('utf-16-le')
            self.ssh.append((list(argv), script))
            stdout = self.answer(script).encode()
        elif argv[0].endswith('sftp'):
            self.sftp.append((list(argv), kwargs['input'].decode()))
        else:
            raise AssertionError(f'unexpected command {argv}')
        return subprocess.CompletedProcess(argv, 0, stdout, b'')

    def answer(self, script):
        if "'COMPUTER='" in script:
            return (f'COMPUTER={self.computer}\nLOCALAPPDATA={self.local_app_data}\nAPP_PROC=0\nPROFILE=False\n'
                    'SESSIONS=console 1 Active\n')
        for marker in ('STAGE_READY', 'VM_UPGRADE_RETURNED', 'VM_CROSS_SESSION_RETURNED', 'VM_INSTALLER_UI_RETURNED'):
            if marker in script:
                return marker + '\n'
        return ''


def receipt(setup, setup_digest, program, program_digest):
    return json.dumps({'artifacts': [{'file': program, 'sha256': program_digest},
                                     {'file': 'target/release/installer-package-helper.exe', 'sha256': 'e' * 64},
                                     {'file': setup, 'sha256': setup_digest}]})


def artifacts(root):
    builds = {
        BUILD: receipt(f'{BUILD}/AutoKeyboardLayot-0.1.1-setup-experimental.exe', DIGESTS['a-setup'],
                       'target/xwin-hotkeys/x86_64-pc-windows-msvc/release/AutoKeyboardLayot.exe', DIGESTS['a-program']),
        SIGNED_BUILD: receipt(f'{SIGNED_BUILD}/AutoKeyboardLayot-0.1.2-setup-experimental.exe', DIGESTS['b-setup'],
                              f'{SIGNED_BUILD}/signed-inputs/AutoKeyboardLayot.exe', DIGESTS['b-program']),
    }
    for folder, text in builds.items():
        (root / folder).mkdir(parents=True)
        (root / folder / 'build.json').write_text(text)


def run_driver(name, environment, fake, arguments=None):
    """Runs one driver's main() against the fake VM and returns (exit code, output directory)."""
    if arguments is None:
        arguments = DRIVER_ARGUMENTS.get(name, ())
    module = types.ModuleType(name)
    module.__file__ = str(TOOLS / name)
    source = (TOOLS / name).read_text(encoding='utf-8')
    with tempfile.TemporaryDirectory() as scratch:
        scratch = Path(scratch)
        artifacts(scratch)
        identity = scratch / 'identity'
        identity.write_bytes(b'test identity file, never a key')
        output = scratch / 'out'
        stdin = types.SimpleNamespace(buffer=io.BytesIO(str(identity).encode() + b'\n'))
        exec(compile(source, str(TOOLS / name), 'exec'), module.__dict__)
        module.ROOT = scratch
        with mock.patch.dict(os.environ, environment, clear=True), \
                mock.patch.object(sys, 'argv', [name, str(output), *arguments]), \
                mock.patch.object(sys, 'stdin', stdin), \
                mock.patch.object(module.subprocess, 'run', fake.run):
            code = module.main()
        controller = (output / 'controller-result.json').read_text() if (output / 'controller-result.json').exists() else ''
        return code, controller


@unittest.skipUnless(hasattr(os, 'O_NOFOLLOW'), 'the VM drivers run from Linux or WSL (/proc, O_NOFOLLOW)')
class DriverTests(unittest.TestCase):
    def every_remote_text(self, fake):
        return [script for _, script in fake.ssh] + [batch for _, batch in fake.sftp]

    def test_missing_settings_stop_every_driver_before_it_does_anything(self):
        for name in DRIVERS:
            fake = FakeVm()
            message = io.StringIO()
            with contextlib.redirect_stderr(message):
                code, _ = run_driver(name, {}, fake)
            self.assertEqual(code, 2, name)
            self.assertIn('AKL_VM_HOST', message.getvalue(), name)
            self.assertEqual(fake.ssh + fake.sftp, [], name)

    def test_a_wrong_computer_stops_before_anything_is_staged(self):
        for name in DRIVERS:
            fake = FakeVm(computer='SOMEOTHERPC')
            code, controller = run_driver(name, ENVIRONMENT, fake)
            self.assertEqual(code, 1, name)
            self.assertEqual(len(fake.ssh), 1, f'{name}: only the preflight may run')
            self.assertEqual(fake.sftp, [], name)
            self.assertIn('failed_before_execution', controller)

    def test_drivers_use_the_login_host_key_name_and_staging_from_the_environment(self):
        environment = dict(ENVIRONMENT, AKL_VM_HOST_KEY_ALIAS='pinned-name')
        stages = {'run_vm_cross_session.py': 'akl-cross-session', 'run_vm_installer_ui.py': 'akl-installer-ui',
                  'run_vm_upgrade.py': 'akl-upgrade'}
        for name in DRIVERS:
            fake = FakeVm()
            run_driver(name, environment, fake)
            calls = fake.ssh + fake.sftp
            self.assertGreaterEqual(len(fake.ssh), 3, name)
            self.assertGreaterEqual(len(fake.sftp), 2, name)
            for argv, _ in calls:
                self.assertIn('tester@vm.test.invalid', argv, name)
                self.assertIn('HostKeyAlias=pinned-name', argv, name)
                self.assertEqual(sum(part == 'tester@vm.test.invalid' for part in argv), 1, name)
            stage = f'E:\\Users\\console\\AppData\\Local\\Temp\\{stages[name]}'
            texts = self.every_remote_text(fake)
            self.assertTrue(any(stage in text for text in texts), f'{name}: staging directory {stage}')
            self.assertTrue(any(sftp_path(stage) in batch for _, batch in fake.sftp), name)
            for text in texts:
                for marker in MACHINE_LOCAL:
                    self.assertNotIn(marker, text, f'{name} sent {marker!r}')

    def test_a_stage_from_the_environment_replaces_the_derived_one(self):
        environment = dict(ENVIRONMENT, AKL_VM_STAGE='G:\\Scratch\\akl')
        for name in DRIVERS:
            fake = FakeVm()
            run_driver(name, environment, fake)
            texts = self.every_remote_text(fake)
            self.assertTrue(any('G:\\Scratch\\akl' in text for text in texts), name)
            self.assertTrue(any('/G:/Scratch/akl' in batch for _, batch in fake.sftp), name)
            self.assertFalse(any('\\Temp\\akl-' in text for text in texts), name)

    def test_the_guest_scripts_receive_the_expected_computer_and_profile_root(self):
        for name in ['run_vm_cross_session.py', 'run_vm_upgrade.py']:
            fake = FakeVm()
            run_driver(name, ENVIRONMENT, fake)
            invocation = next(script for _, script in fake.ssh if '-Receipt' in script)
            self.assertIn("-ExpectedComputer 'TESTGUEST01'", invocation, name)
            self.assertIn("-LocalAppData 'E:\\Users\\console\\AppData\\Local'", invocation, name)

    def test_the_interactive_task_runs_as_the_configured_user(self):
        environment = dict(ENVIRONMENT, AKL_VM_TEST_USER='console',
                           AKL_VM_LOCALAPPDATA='D:\\Profiles\\console\\AppData\\Local',
                           AKL_VM_STAGE='D:\\Stage\\akl')
        fake = FakeVm()
        run_driver('run_vm_installer_ui.py', environment, fake)
        tasks = [script for _, script in fake.ssh if 'Register-ScheduledTask' in script]
        self.assertEqual(len(tasks), 2)
        for task in tasks:
            self.assertIn("-UserId 'TESTGUEST01\\console'", task)

    def test_the_installer_ui_driver_runs_each_named_scenario_of_the_build(self):
        fake = FakeVm()
        _, controller = run_driver('run_vm_installer_ui.py', ENVIRONMENT, fake)
        uploads = ''.join(batch for _, batch in fake.sftp)
        self.assertIn('installer-build-a/AutoKeyboardLayot-0.1.1-setup-experimental.exe', uploads)
        self.assertIn('run-retry.cmd', uploads)
        self.assertIn('run-missing-exe.cmd', uploads)
        self.assertEqual(set(json.loads(controller)['scenarios']), {'retry', 'missing-exe'})

    def test_the_upgrade_driver_installs_the_named_old_build_then_the_new_one(self):
        fake = FakeVm()
        _, controller = run_driver('run_vm_upgrade.py', ENVIRONMENT, fake)
        uploads = ''.join(batch for _, batch in fake.sftp)
        self.assertIn('installer-build-a/AutoKeyboardLayot-0.1.1-setup-experimental.exe" "', uploads)
        self.assertIn('/old-setup.exe', uploads)
        self.assertIn('installer-build-b/AutoKeyboardLayot-0.1.2-setup-experimental.exe" "', uploads)
        self.assertIn('/new-setup.exe', uploads)
        invocation = next(script for _, script in fake.ssh if '-OldAppSha256' in script)
        for parameter, digest in (('-OldSetupSha256', 'a-setup'), ('-OldAppSha256', 'a-program'),
                                  ('-NewSetupSha256', 'b-setup'), ('-NewAppSha256', 'b-program')):
            self.assertIn(f"{parameter} '{DIGESTS[digest]}'", invocation)

    def test_the_upgrade_driver_needs_both_builds(self):
        fake = FakeVm()
        with contextlib.redirect_stderr(io.StringIO()):
            code, _ = run_driver('run_vm_upgrade.py', ENVIRONMENT, fake, (BUILD,))
        self.assertEqual(code, 2)
        self.assertEqual(fake.ssh + fake.sftp, [])

    def test_a_receipt_without_one_setup_and_one_program_is_refused(self):
        with tempfile.TemporaryDirectory() as scratch:
            folder = Path(scratch)
            program = 'target/release/AutoKeyboardLayot.exe'
            setup = 'AutoKeyboardLayot-0.1.1-setup-experimental.exe'
            for text in (json.dumps({'artifacts': [{'file': program, 'sha256': 'b' * 64}]}),
                         json.dumps({'artifacts': [{'file': setup, 'sha256': 'a' * 64}]}),
                         receipt(setup, 'a' * 64, program, 'not a digest')):
                (folder / 'build.json').write_text(text)
                with self.assertRaises(ValueError, msg=text):
                    vm_config.build_artifacts(folder)
            (folder / 'build.json').write_text(receipt(setup, 'a' * 64, 'out/signed-inputs/AutoKeyboardLayot.exe', 'b' * 64))
            self.assertEqual(vm_config.build_artifacts(folder), (folder / setup, 'a' * 64, 'b' * 64))

    def test_an_unknown_scenario_or_a_missing_build_stops_before_anything_runs(self):
        for arguments in [(BUILD,), (BUILD, 'probe'), (BUILD, 'retry', 'unknown')]:
            fake = FakeVm()
            message = io.StringIO()
            with contextlib.redirect_stderr(message):
                code, _ = run_driver('run_vm_installer_ui.py', ENVIRONMENT, fake, arguments)
            self.assertEqual(code, 2, arguments)
            self.assertEqual(fake.ssh + fake.sftp, [], arguments)


if __name__ == '__main__':
    unittest.main()
