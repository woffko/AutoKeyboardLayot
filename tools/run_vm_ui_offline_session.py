"""Stage candidate3 on the VM and run the real-setup offline UI acceptance in the interactive session."""
import base64, json, os, stat, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
STAGE = r'C:\Users\w0w\AppData\Local\Temp\akl-ui-offline'
STAGE_UNIX = '/C:/Users/w0w/AppData/Local/Temp/akl-ui-offline'
TASK = 'AklUiOfflineAcceptance'

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
                    result['remote_stdout'] = run.stdout.decode('utf-8', 'replace')[-2000:]
                    raise ValueError('remote command')
                return run.stdout.decode('utf-8', 'replace')
            result['phase'] = 'preflight'
            preflight = remote(r"""$ProgressPreference='SilentlyContinue'
'COMPUTER='+$env:COMPUTERNAME
'APP_PROC='+@(Get-Process -Name AutoKeyboardLayot -ErrorAction SilentlyContinue).Count
'PROFILE='+(Test-Path -LiteralPath 'C:\Users\w0w\AppData\Local\AutoKeyboardLayot')
'SESSIONS='+(((& "$env:SystemRoot\System32\query.exe" user 2>&1) | Out-String).Trim())
""", 30)
            result['preflight'] = preflight.strip().splitlines()
            text = preflight
            if 'COMPUTER=DESKTOP-ELS4LDK' not in text or 'APP_PROC=0' not in text or 'PROFILE=False' not in text or 'console' not in text or 'Active' not in text:
                raise ValueError('preflight result')
            setup = ROOT / 'target/installer-vm-candidate-20260913-03/AutoKeyboardLayot-0.1.0-setup-experimental.exe'
            catalog = ROOT / 'target/ui-package-candidates-20260912-01/catalog.aklc'
            ru = ROOT / 'target/ui-package-candidates-20260912-01/ru-RU-r1.aklp'
            driver = ROOT / 'tools/test-installer-ui-offline.ps1'
            build = json.loads((ROOT / 'target/installer-vm-candidate-20260913-03/build.json').read_text())
            setup_sha = next(a['sha256'] for a in build['artifacts'] if a['file'].endswith('setup-experimental.exe'))
            app_sha = next(a['sha256'] for a in build['artifacts'] if a['file'].endswith('release/AutoKeyboardLayot.exe'))
            result.update(phase='upload', setup_sha256=setup_sha, app_sha256=app_sha)
            remote(r"""$ErrorActionPreference='Stop'
$s='C:\Users\w0w\AppData\Local\Temp\akl-ui-offline'
Remove-Item -LiteralPath $s -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path (Join-Path $s 'catalog') -Force | Out-Null
'STAGE_READY'""", 30)
            batch = (f'put "{setup}" "{STAGE_UNIX}/setup.exe"\n'
                     f'put "{catalog}" "{STAGE_UNIX}/catalog/catalog.aklc"\n'
                     f'put "{ru}" "{STAGE_UNIX}/catalog/ru-RU-r1.aklp"\n'
                     f'put "{driver}" "{STAGE_UNIX}/test-installer-ui-offline.ps1"\n')
            up = subprocess.run(['/usr/bin/sftp', *options, '-b', '-', 'root@192.168.189.129'],
                                input=batch.encode(), stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60)
            if up.returncode:
                raise ValueError('upload')
            result.update(phase='execute', execution_requested=True)
            cmd = (f'@echo off\r\n'
                   f'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0test-installer-ui-offline.ps1" '
                   f'-Executable "%~dp0setup.exe" -ExpectedSha256 {setup_sha} '
                   f'-Catalog "%~dp0catalog\\catalog.aklc" -InstalledAppHash {app_sha} '
                   f'-OutputDirectory "%~dp0out" > "%~dp0driver.log" 2>&1\r\n'
                   f'echo DRIVER_EXIT=%ERRORLEVEL% >> "%~dp0driver.log"\r\n')
            (output / 'run_ui.cmd').write_bytes(cmd.encode('ascii'))
            batch = f'put "{output / "run_ui.cmd"}" "{STAGE_UNIX}/run_ui.cmd"\n'
            subprocess.run(['/usr/bin/sftp', *options, '-b', '-', 'root@192.168.189.129'],
                           input=batch.encode(), stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60)
            run_script = (r"$ErrorActionPreference='Continue'"
                r";$s='C:\Users\w0w\AppData\Local\Temp\akl-ui-offline'"
                r";$task='AklUiOfflineAcceptance'"
                r";Unregister-ScheduledTask -TaskName $task -Confirm:$false -ErrorAction SilentlyContinue"
                r";$act=New-ScheduledTaskAction -Execute (Join-Path $s 'run_ui.cmd')"
                r";$pr=New-ScheduledTaskPrincipal -UserId 'DESKTOP-ELS4LDK\w0w' -LogonType Interactive"
                r";$set=New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 15)"
                r";Register-ScheduledTask -TaskName $task -Action $act -Principal $pr -Settings $set -Force -ErrorAction Stop | Out-Null"
                r";Start-ScheduledTask -TaskName $task -ErrorAction Stop"
                r";$deadline=(Get-Date).AddSeconds(420)"
                r";while((Get-Date) -lt $deadline -and -not (Test-Path -LiteralPath (Join-Path $s 'out\result.json'))){Start-Sleep -Seconds 3}"
                r";$info=Get-ScheduledTaskInfo -TaskName $task"
                r";'LAST_RESULT='+$info.LastTaskResult"
                r";'TASK_STATE='+((Get-ScheduledTask -TaskName $task).State)"
                r";'RESULT_PRESENT='+(Test-Path -LiteralPath (Join-Path $s 'out\result.json'))"
                r";Unregister-ScheduledTask -TaskName $task -Confirm:$false -ErrorAction SilentlyContinue"
                r";'VM_UI_OFFLINE_RETURNED'")
            returned = remote(run_script, 480)
            result['task_query'] = returned.strip()
            result['command_returned'] = 'VM_UI_OFFLINE_RETURNED' in returned
            result['phase'] = 'fetch'
            names = ['result.json', 'driver.log', 'package-page.png', 'catalog-selection.png', 'download-complete.png', 'review-page.png', 'installed.png', 'failure-window.png', 'inno.log']
            batch = ''.join(f'-get "{STAGE_UNIX}/out/{n}" "{output / n}"\n' for n in names)
            batch += f'-get "{STAGE_UNIX}/driver.log" "{output / "driver.log"}"\n'
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
