"""Stage candidate1 and candidate3 on the VM and run the upgrade/interruption test."""
import base64, json, os, stat, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
STAGE_UNIX = '/C:/Users/w0w/AppData/Local/Temp/akl-upgrade'
STAGE = r'C:\Users\w0w\AppData\Local\Temp\akl-upgrade'

def main():
    output = Path(sys.argv[1]).resolve()
    output.mkdir()
    raw = bytearray(sys.stdin.buffer.read(4097))
    result = {'state': 'failed_before_execution', 'execution_requested': False}
    try:
        identity = raw.decode('utf-8').rstrip('\r\n')
        if not os.path.isabs(identity):
            raise ValueError('identity shape')
        fd = os.open(identity, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        try:
            options = ['-F', '/dev/null', '-o', 'BatchMode=yes', '-o', 'IdentitiesOnly=yes',
                       '-o', 'PasswordAuthentication=no', '-o', 'KbdInteractiveAuthentication=no',
                       '-o', 'StrictHostKeyChecking=yes', '-o', 'HostKeyAlias=192.168.189.138',
                       '-o', 'ConnectTimeout=5', '-o', 'ConnectionAttempts=1', '-o', 'ForwardAgent=no',
                       '-o', 'ClearAllForwardings=yes', '-o', 'ControlMaster=no',
                       '-i', f'/proc/{os.getpid()}/fd/{fd}']
            def remote(script, timeout=30):
                encoded = base64.b64encode(script.encode('utf-16-le')).decode('ascii')
                run = subprocess.run(['/usr/bin/ssh', *options, 'root@192.168.189.129',
                    'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand ' + encoded],
                    stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout)
                if run.returncode:
                    result['transport_exit_code'] = run.returncode
                    result['remote_stderr'] = run.stderr.decode('utf-8', 'replace')[-2000:]
                    raise ValueError('remote command')
                return run.stdout.decode('utf-8', 'replace')
            old_build = json.loads((ROOT / 'target/installer-vm-candidate-20260913-01/build.json').read_text())
            new_build = json.loads((ROOT / 'target/installer-vm-candidate-20260913-03/build.json').read_text())
            def art(build, suffix):
                return next(a['sha256'] for a in build['artifacts'] if a['file'].endswith(suffix))
            old_setup = ROOT / 'target/installer-vm-candidate-20260913-01/AutoKeyboardLayot-0.1.0-setup-experimental.exe'
            new_setup = ROOT / 'target/installer-vm-candidate-20260913-03/AutoKeyboardLayot-0.1.0-setup-experimental.exe'
            script = ROOT / 'tools/test-vm-upgrade.ps1'
            old_setup_sha = art(old_build, 'setup-experimental.exe')
            new_setup_sha = art(new_build, 'setup-experimental.exe')
            old_app_sha = art(old_build, 'release/AutoKeyboardLayot.exe')
            new_app_sha = art(new_build, 'release/AutoKeyboardLayot.exe')
            preflight = remote(r"""$P='SilentlyContinue'
'COMPUTER='+$env:COMPUTERNAME
'APP_PROC='+@(Get-Process -Name AutoKeyboardLayot -ErrorAction SilentlyContinue).Count
'PROFILE='+(Test-Path -LiteralPath 'C:\Users\w0w\AppData\Local\AutoKeyboardLayot')
'SESSIONS='+(((& "$env:SystemRoot\System32\query.exe" user 2>&1) | Out-String).Trim())
""")
            result['preflight'] = preflight.strip().splitlines()
            if 'COMPUTER=DESKTOP-ELS4LDK' not in preflight or 'APP_PROC=0' not in preflight or 'PROFILE=False' not in preflight or 'console' not in preflight or 'Active' not in preflight:
                raise ValueError('preflight result')
            result.update(phase='upload', old_setup_sha256=old_setup_sha, new_setup_sha256=new_setup_sha)
            remote(r"""$ErrorActionPreference='Stop'
$s='C:\Users\w0w\AppData\Local\Temp\akl-upgrade'
Remove-Item -LiteralPath $s -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $s -Force | Out-Null
'STAGE_READY'""")
            batch = (f'put "{old_setup}" "{STAGE_UNIX}/old-setup.exe"\n'
                     f'put "{new_setup}" "{STAGE_UNIX}/new-setup.exe"\n'
                     f'put "{script}" "{STAGE_UNIX}/test-vm-upgrade.ps1"\n')
            up = subprocess.run(['/usr/bin/sftp', *options, '-b', '-', 'root@192.168.189.129'],
                                input=batch.encode(), stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=180)
            if up.returncode:
                raise ValueError('upload')
            result.update(phase='execute', execution_requested=True)
            run_script = (r"$ErrorActionPreference='Stop'"
                rf";$s='{STAGE}'"
                r";$p=Join-Path $s 'test-vm-upgrade.ps1'"
                rf";& $p -Stage $s -OldSetupSha256 '{old_setup_sha}' -NewSetupSha256 '{new_setup_sha}' -OldAppSha256 '{old_app_sha}' -NewAppSha256 '{new_app_sha}' -Receipt (Join-Path $s 'result.json') | Out-Null"
                r";'VM_UPGRADE_RETURNED'")
            returned = remote(run_script, 700)
            result['command_returned'] = 'VM_UPGRADE_RETURNED' in returned
            result['phase'] = 'fetch'
            names = ['result.json', 'old-install.log', 'upgrade.log', 'interrupted.log', 'recover.log']
            batch = ''.join(f'-get "{STAGE_UNIX}/{n}" "{output / n}"\n' for n in names)
            subprocess.run(['/usr/bin/sftp', *options, '-b', '-', 'root@192.168.189.129'],
                           input=batch.encode(), stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60)
            final = output / 'result.json'
            if final.is_file():
                result['remote_result'] = json.loads(final.read_text(encoding='utf-8-sig'))
                result['state'] = result['remote_result']['state']
            else:
                result['state'] = 'unobserved_after_submission'
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
