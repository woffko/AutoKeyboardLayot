"""Optional Authenticode signing for the installer build. Off unless it is fully configured.

tools/build_experimental_installer.py signs the application and the package helper before the
installer is compiled, and the installer after it, when these environment variables are set:

  AKL_SIGNTOOL            path to signtool.exe (this alone does not switch signing on)
  AKL_SIGN_THUMBPRINT     SHA-1 thumbprint (40 hex digits) of the code-signing certificate in a
                          Windows certificate store; the private key may be on a hardware token
  AKL_SIGN_TIMESTAMP_URL  RFC 3161 timestamp server (http:// or https://)

With none of them set nothing happens and the build is exactly what it was. With some but not all
set the build stops: a half-configured signing run must not quietly produce unsigned files.

No password, key or certificate file is read, stored or passed on a command line. The certificate is
selected by its thumbprint and the signing tool asks for any PIN itself. Every file is verified
with signtool after it is signed. The owner chooses and buys the certificate; nothing here does.
"""
import re
import subprocess
from pathlib import Path

TOOL = 'AKL_SIGNTOOL'
THUMBPRINT = 'AKL_SIGN_THUMBPRINT'
TIMESTAMP_URL = 'AKL_SIGN_TIMESTAMP_URL'

_THUMBPRINT = re.compile(r'^[0-9A-Fa-f]{40}$')
_URL = re.compile(r'^https?://[A-Za-z0-9.-]+(:[0-9]{1,5})?(/[A-Za-z0-9._~:/?#%=&+-]*)?$')


class SigningConfigurationError(ValueError):
    """The Authenticode settings are incomplete or unusable."""


class Config:
    def __init__(self, tool, thumbprint, timestamp_url):
        self.tool = tool
        self.thumbprint = thumbprint
        self.timestamp_url = timestamp_url

    def describe(self):
        """What the build receipt records. Public values only."""
        return {'signed': True, 'thumbprint': self.thumbprint, 'timestamp_url': self.timestamp_url}


def configuration(env):
    """None when signing is off, a Config when it is fully configured; anything else is an error."""
    tool = env.get(TOOL, '').strip()
    thumbprint = env.get(THUMBPRINT, '').strip()
    timestamp_url = env.get(TIMESTAMP_URL, '').strip()
    if not (tool or thumbprint or timestamp_url):
        return None
    missing = [name for name, value in ((TOOL, tool), (THUMBPRINT, thumbprint), (TIMESTAMP_URL, timestamp_url))
               if not value]
    if missing:
        raise SigningConfigurationError(
            f'Authenticode signing is half configured, {", ".join(missing)} missing: '
            f'set {TOOL}, {THUMBPRINT} and {TIMESTAMP_URL}, or none of them.')
    if not _THUMBPRINT.fullmatch(thumbprint):
        raise SigningConfigurationError(f'{THUMBPRINT} must be 40 hexadecimal digits.')
    if not _URL.fullmatch(timestamp_url):
        raise SigningConfigurationError(f'{TIMESTAMP_URL} must be an http:// or https:// address.')
    if Path(tool).name.lower() != 'signtool.exe' or not Path(tool).is_file():
        raise SigningConfigurationError(f'{TOOL} must be the path of an existing signtool.exe.')
    return Config(tool, thumbprint.upper(), timestamp_url)


def sign_command(config, file):
    """Sign with the SHA-256 file digest and an RFC 3161 SHA-256 timestamp."""
    return [config.tool, 'sign', '/fd', 'SHA256', '/sha1', config.thumbprint,
            '/tr', config.timestamp_url, '/td', 'SHA256', str(file)]


def verify_command(config, file):
    """Check the signature with the default Authenticode policy; an untrusted chain fails."""
    return [config.tool, 'verify', '/pa', '/v', str(file)]


def sign_and_verify(config, files, run=subprocess.run, to_windows=str):
    """Signs each file and verifies it before the next one. Any failure stops the build."""
    for file in files:
        path = to_windows(file)
        run(sign_command(config, path), check=True, timeout=300, stdin=subprocess.DEVNULL)
        run(verify_command(config, path), check=True, timeout=120, stdin=subprocess.DEVNULL)
