"""Settings of the test VM for the VM acceptance scripts, read from the environment.

Nothing about a machine is committed. The address, the accounts, the expected computer
name and the profile paths on the VM come from these variables, and a script that is
missing one stops before it touches the VM:

  AKL_VM_HOST            address or name of the VM (required)
  AKL_VM_USER            account for ssh and sftp (required)
  AKL_VM_COMPUTER_NAME   computer name of the VM; the scripts refuse any other machine (required)
  AKL_VM_TEST_USER       interactive user whose session and profile are tested, for scripts that
                         start a task in that session (default: AKL_VM_USER)
  AKL_VM_HOST_KEY_ALIAS  name under which the host key is pinned in known_hosts (default: the host)
  AKL_VM_LOCALAPPDATA    LOCALAPPDATA of the tested user (default: asked from the VM)
  AKL_VM_STAGE           staging directory on the VM (default: Temp under that LOCALAPPDATA)

What the VM reports is the ssh account's LOCALAPPDATA, so when AKL_VM_TEST_USER names another
user, AKL_VM_LOCALAPPDATA and AKL_VM_STAGE are required: a directory both users can use.
"""
import json
import os
import re
from pathlib import PurePosixPath

# A name or address that ssh cannot mistake for an option, and that cannot carry a command.
_HOST = re.compile(r'^[A-Za-z0-9][A-Za-z0-9._:-]{0,252}$')
_ACCOUNT = re.compile(r'^[A-Za-z0-9_][A-Za-z0-9._-]{0,63}$')
_COMPUTER = re.compile(r'^[A-Za-z0-9][A-Za-z0-9-]{0,62}$')
# A drive path that is safe inside a single-quoted PowerShell string and a double-quoted sftp argument.
_WINDOWS_DIRECTORY = re.compile(r"^[A-Za-z]:\\[^\"'`$;|&<>(){}\r\n\0]+$")


class VmSettingsError(ValueError):
    """A required setting is missing or has an unusable value; the message names the variable."""


def _read(env, name, pattern, hint, required=True):
    value = env.get(name, '').strip()
    if not value:
        if required:
            raise VmSettingsError(f'Set {name} ({hint}).')
        return None
    if not pattern.fullmatch(value):
        raise VmSettingsError(f'{name} has an unusable value ({hint}).')
    return value


class VmSettings:
    def __init__(self, host, user, computer, test_user, host_key_alias, local_app_data, stage):
        self.host = host
        self.user = user
        self.computer = computer
        self.test_user = test_user
        self.host_key_alias = host_key_alias
        self.local_app_data = local_app_data
        self.stage = stage

    @classmethod
    def from_environment(cls, env=None):
        env = os.environ if env is None else env
        host = _read(env, 'AKL_VM_HOST', _HOST, 'address or name of the VM')
        user = _read(env, 'AKL_VM_USER', _ACCOUNT, 'account for ssh and sftp')
        computer = _read(env, 'AKL_VM_COMPUTER_NAME', _COMPUTER, "the VM's computer name")
        test_user = _read(env, 'AKL_VM_TEST_USER', _ACCOUNT, 'interactive user to test', required=False) or user
        alias = _read(env, 'AKL_VM_HOST_KEY_ALIAS', _HOST, 'host key name', required=False) or host
        local = _read(env, 'AKL_VM_LOCALAPPDATA', _WINDOWS_DIRECTORY, 'a Windows path such as D:\\Users\\name\\AppData\\Local',
                      required=False)
        stage = _read(env, 'AKL_VM_STAGE', _WINDOWS_DIRECTORY, 'a Windows directory path', required=False)
        if test_user.lower() != user.lower():
            for name, value in (('AKL_VM_LOCALAPPDATA', local), ('AKL_VM_STAGE', stage)):
                if value is None:
                    raise VmSettingsError(
                        f'Set {name}: AKL_VM_TEST_USER {test_user} is not the ssh account {user}, whose profile '
                        'the VM would report.')
        return cls(host, user, computer, test_user, alias, local, stage)

    @property
    def login(self):
        return f'{self.user}@{self.host}'

    @property
    def scheduled_task_user(self):
        return f'{self.computer}\\{self.test_user}'

    def host_key_option(self):
        return ['-o', f'HostKeyAlias={self.host_key_alias}']

    def preflight_script(self):
        """PowerShell that reports what the scripts check before they change anything."""
        profile = "(Join-Path $env:LOCALAPPDATA 'AutoKeyboardLayot')"
        if self.local_app_data:
            profile = f"(Join-Path '{self.local_app_data}' 'AutoKeyboardLayot')"
        return ("$ProgressPreference='SilentlyContinue'\n"
                "'COMPUTER='+$env:COMPUTERNAME\n"
                "'LOCALAPPDATA='+$env:LOCALAPPDATA\n"
                "'APP_PROC='+@(Get-Process -Name AutoKeyboardLayot -ErrorAction SilentlyContinue).Count\n"
                f"'PROFILE='+(Test-Path -LiteralPath {profile})\n"
                "'SESSIONS='+(((& \"$env:SystemRoot\\System32\\query.exe\" user 2>&1) | Out-String).Trim())\n")

    def preflight_ok(self, text):
        """True for a clean VM: the expected computer, no running app, no profile, an active console."""
        lines = text.splitlines()
        return (f'COMPUTER={self.computer}' in lines and 'APP_PROC=0' in lines and 'PROFILE=False' in lines
                and 'console' in text and 'Active' in text)

    def effective_local_app_data(self, preflight_text):
        """The tested user's LOCALAPPDATA: the setting, or what the VM reported."""
        if self.local_app_data:
            return self.local_app_data
        for line in preflight_text.splitlines():
            if line.startswith('LOCALAPPDATA='):
                reported = line[len('LOCALAPPDATA='):].strip()
                if _WINDOWS_DIRECTORY.fullmatch(reported):
                    return reported
        raise VmSettingsError('The VM did not report a usable LOCALAPPDATA; set AKL_VM_LOCALAPPDATA.')

    def stage_directory(self, name, preflight_text):
        """Windows path of the staging directory for one script."""
        if self.stage:
            return self.stage.rstrip('\\')
        return f'{self.effective_local_app_data(preflight_text).rstrip(chr(92))}\\Temp\\{name}'


def build_artifacts(build_directory):
    """(setup path, setup SHA-256, program SHA-256) from the receipt of build_experimental_installer.py.

    The program is found by its name wherever the build put it: below release/ for an unsigned build,
    in signed-inputs/ for a signed one. The setup is the one -setup-experimental.exe of the build.
    """
    build = json.loads((build_directory / 'build.json').read_text())
    setups = [entry for entry in build['artifacts'] if entry['file'].endswith('-setup-experimental.exe')]
    programs = [entry for entry in build['artifacts'] if PurePosixPath(entry['file']).name == 'AutoKeyboardLayot.exe']
    if len(setups) != 1 or len(programs) != 1:
        raise ValueError('the receipt names no single setup and program')
    for digest in (setups[0]['sha256'], programs[0]['sha256']):
        if not re.fullmatch('[0-9a-f]{64}', digest):
            raise ValueError('receipt hash')
    return build_directory / PurePosixPath(setups[0]['file']).name, setups[0]['sha256'], programs[0]['sha256']


def sftp_path(windows_path):
    """The form in which the Windows OpenSSH sftp server addresses a drive path."""
    return '/' + windows_path.replace('\\', '/')
