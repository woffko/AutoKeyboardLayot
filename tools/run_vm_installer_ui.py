"""Stage an installer build on the VM and run installer acceptance scenarios in the interactive session.

Usage: run_vm_installer_ui.py OUTPUT_DIRECTORY BUILD_DIRECTORY SCENARIO... < identity-path

BUILD_DIRECTORY is an output of tools/build_experimental_installer.py, relative to the repository
(its build.json names the setup and the program). Each SCENARIO is one of tools/test-installer-ui.ps1
except probe, which needs a /DInstallerUiProbe build and runs directly in the session.
Needs AKL_VM_HOST, AKL_VM_USER and AKL_VM_COMPUTER_NAME in the environment; AKL_VM_TEST_USER names
the interactive user (see vm_config.py). The ssh identity path comes on standard input.
"""
import base64, json, os, re, subprocess, sys
from pathlib import Path, PurePosixPath

from vm_config import VmSettings, VmSettingsError, sftp_path

ROOT = Path(__file__).resolve().parents[1]
STAGE_NAME = 'akl-installer-ui'
TASK = 'AklInstallerAcceptance'
SCENARIOS = ('retry', 'helper-failure', 'timer-exception', 'no-store', 'uninstall-confirmation', 'missing-exe',
             'startup-removal', 'fence-invalid', 'settings-open')


def artifacts(build_directory):
    """(setup path, setup SHA-256, program SHA-256) from the build receipt."""
    build = json.loads((build_directory / 'build.json').read_text())
    setup = next(a for a in build['artifacts'] if a['file'].endswith('setup-experimental.exe'))
    app = next(a for a in build['artifacts'] if PurePosixPath(a['file']).name == 'AutoKeyboardLayot.exe')
    for digest in (setup['sha256'], app['sha256']):
        if not re.fullmatch('[0-9a-f]{64}', digest):
            raise ValueError('receipt hash')
    return build_directory / PurePosixPath(setup['file']).name, setup['sha256'], app['sha256']


def main():
    try:
        vm = VmSettings.from_environment()
    except VmSettingsError as error:
        print(error, file=sys.stderr)
        return 2
    if len(sys.argv) < 4 or any(name not in SCENARIOS for name in sys.argv[3:]):
        print(__doc__, file=sys.stderr)
        return 2
    output = Path(sys.argv[1]).resolve()
    output.mkdir()
    build_directory = (ROOT / sys.argv[2]).resolve()
    scenarios = sys.argv[3:]
    raw = bytearray(sys.stdin.buffer.read(4097))
    result = {'state': 'failed_before_execution', 'execution_requested': False, 'scenarios': {}}
    try:
        identity = raw.decode('utf-8').rstrip('\r\n')
        if not os.path.isabs(identity):
            raise ValueError('identity shape')
        fd = os.open(identity, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        try:
            options = ['-F', '/dev/null', '-o', 'BatchMode=yes', '-o', 'IdentitiesOnly=yes',
                       '-o', 'PasswordAuthentication=no', '-o', 'KbdInteractiveAuthentication=no',
                       '-o', 'StrictHostKeyChecking=yes', *vm.host_key_option(),
                       '-o', 'ConnectTimeout=5', '-o', 'ConnectionAttempts=1', '-o', 'ForwardAgent=no',
                       '-o', 'ClearAllForwardings=yes', '-o', 'ControlMaster=no',
                       '-i', f'/proc/{os.getpid()}/fd/{fd}']
            def remote(script, timeout=30):
                encoded = base64.b64encode(script.encode('utf-16-le')).decode('ascii')
                run = subprocess.run(['/usr/bin/ssh', *options, vm.login,
                    'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand ' + encoded],
                    stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout)
                if run.returncode:
                    result['transport_exit_code'] = run.returncode
                    result['remote_stderr'] = run.stderr.decode('utf-8', 'replace')[-2000:]
                    raise ValueError('remote command')
                return run.stdout.decode('utf-8', 'replace')
            def sftp(batch, timeout=120):
                return subprocess.run(['/usr/bin/sftp', *options, '-b', '-', vm.login], input=batch.encode(),
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout)
            result['phase'] = 'preflight'
            preflight = remote(vm.preflight_script(), 30)
            result['preflight'] = preflight.strip().splitlines()
            if not vm.preflight_ok(preflight):
                raise ValueError('preflight result')
            stage = vm.stage_directory(STAGE_NAME, preflight)
            stage_unix = sftp_path(stage)
            setup, setup_sha, app_sha = artifacts(build_directory)
            result.update(phase='upload', setup_sha256=setup_sha, app_sha256=app_sha)
            remote(f"""$ErrorActionPreference='Stop'
$s='{stage}'
Remove-Item -LiteralPath $s -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $s -Force | Out-Null
'STAGE_READY'""", 30)
            if sftp(f'put "{setup}" "{stage_unix}/setup.exe"\n'
                    f'put "{ROOT / "tools/test-installer-ui.ps1"}" "{stage_unix}/test-installer-ui.ps1"\n').returncode:
                raise ValueError('upload')
            result.update(phase='execute', execution_requested=True)
            for scenario in scenarios:
                out = f'out-{scenario}'
                cmd = (f'@echo off\r\n'
                       f'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0test-installer-ui.ps1" '
                       f'-Executable "%~dp0setup.exe" -ExpectedSha256 {setup_sha} -InstalledAppHash {app_sha} '
                       f'-Scenario {scenario} -OutputDirectory "%~dp0{out}" > "%~dp0{out}.log" 2>&1\r\n')
                (output / f'run-{scenario}.cmd').write_bytes(cmd.encode('ascii'))
                sftp(f'put "{output / f"run-{scenario}.cmd"}" "{stage_unix}/run-{scenario}.cmd"\n', 60)
                run_script = (r"$ErrorActionPreference='Continue'"
                    rf";$s='{stage}'"
                    rf";$task='{TASK}'"
                    r";Unregister-ScheduledTask -TaskName $task -Confirm:$false -ErrorAction SilentlyContinue"
                    rf";$act=New-ScheduledTaskAction -Execute (Join-Path $s 'run-{scenario}.cmd')"
                    rf";$pr=New-ScheduledTaskPrincipal -UserId '{vm.scheduled_task_user}' -LogonType Interactive"
                    r";$set=New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 15)"
                    r";Register-ScheduledTask -TaskName $task -Action $act -Principal $pr -Settings $set -Force -ErrorAction Stop | Out-Null"
                    r";Start-ScheduledTask -TaskName $task -ErrorAction Stop"
                    r";$deadline=(Get-Date).AddSeconds(900)"
                    rf";while((Get-Date) -lt $deadline -and -not (Test-Path -LiteralPath (Join-Path $s '{out}\result.json'))){{Start-Sleep -Seconds 3}}"
                    r";Start-Sleep -Seconds 2"
                    r";Unregister-ScheduledTask -TaskName $task -Confirm:$false -ErrorAction SilentlyContinue"
                    r";'VM_INSTALLER_UI_RETURNED'")
                returned = remote(run_script, 960)
                folder = output / out
                folder.mkdir()
                sftp(f'-get "{stage_unix}/{out}/result.json" "{folder / "result.json"}"\n'
                     f'-get "{stage_unix}/{out}/setup.log" "{folder / "setup.log"}"\n'
                     f'-get "{stage_unix}/{out}.log" "{folder / "driver.log"}"\n', 60)
                final = folder / 'result.json'
                state = json.loads(final.read_text(encoding='utf-8-sig'))['state'] if final.is_file() else 'unobserved'
                result['scenarios'][scenario] = {'state': state, 'returned': 'VM_INSTALLER_UI_RETURNED' in returned}
            result['phase'] = 'complete'
            states = {entry['state'] for entry in result['scenarios'].values()}
            result['state'] = 'passed' if states == {'passed'} else 'failed'
        finally:
            os.close(fd)
    except Exception as error:
        result['state'] = 'unobserved_after_submission' if result['execution_requested'] else 'failed_before_execution'
        result['failure_kind'] = type(error).__name__
    finally:
        raw[:] = b'\0' * len(raw)
        (output / 'controller-result.json').write_text(json.dumps(result, indent=2))
    return 0 if result['state'] == 'passed' else 1


if __name__ == '__main__':
    raise SystemExit(main())
