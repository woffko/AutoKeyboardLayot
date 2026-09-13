"""One-shot creation; never explicitly write private material to files or logs."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import resource
import stat
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
OPENSSL = '/usr/bin/openssl'
POWERSHELL = '/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe'
SPKI_PREFIX = bytes.fromhex('302a300506032b6570032100')


def run(argv, secret=None, timeout=20, env=None):
    result = subprocess.run(argv, input=secret, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            timeout=timeout, env=env, cwd=ROOT)
    if result.returncode:
        raise RuntimeError('subprocess failed; output intentionally suppressed')
    return result.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--operation', required=True)
    parser.add_argument('--receipt-directory', required=True, type=Path)
    parser.add_argument('--probe-only', action='store_true')
    args = parser.parse_args()
    if not re.fullmatch('[a-z0-9-]{1,32}', args.operation):
        raise SystemExit('Invalid operation/signer ID.')
    info = os.stat(OPENSSL)
    if info.st_uid != 0 or info.st_mode & (stat.S_IWGRP | stat.S_IWOTH):
        raise SystemExit('OpenSSL executable ownership is not trusted.')
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    script = run(['wslpath', '-w', str(ROOT / 'tools/protect-package-key.ps1')]).decode().strip()
    helper = [POWERSHELL, '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
              '-File', script, '-Operation', args.operation]
    ready = run(helper + ['-Mode', 'probe'], timeout=30).decode('utf-8-sig').splitlines()
    if len(ready) != 2 or ready[0] != 'READY':
        raise SystemExit('Preflight did not return the expected public receipt.')
    if args.probe_only:
        print(json.dumps({'ready': True, 'key_directory': ready[1], 'key_created': False}))
        return
    args.receipt_directory.mkdir(mode=0o700, parents=False, exist_ok=False)
    receipt = {'operation': args.operation, 'state': 'started', 'key_directory': ready[1],
               'repository': 'woffko/AutoKeyboardLayot', 'private_key_may_exist': False}
    receipt_path = args.receipt_directory / 'creation.json'
    def save_receipt():
        temporary = args.receipt_directory / 'creation.next.json'
        with temporary.open('x', encoding='utf-8') as stream:
            json.dump(receipt, stream, indent=2)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, receipt_path)
        directory_fd = os.open(args.receipt_directory, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    save_receipt()
    private = None
    stage = 'generate'
    try:
        crypto_env = {'PATH': '/usr/bin:/bin', 'LC_ALL': 'C', 'OPENSSL_CONF': '/dev/null'}
        private = bytearray(run([OPENSSL, 'genpkey', '-algorithm', 'ED25519', '-outform', 'DER'], env=crypto_env))
        if len(private) != 48:
            raise RuntimeError('unexpected private encoding')
        public_der = run([OPENSSL, 'pkey', '-inform', 'DER', '-pubout', '-outform', 'DER'], private, env=crypto_env)
        if len(public_der) != 44 or not public_der.startswith(SPKI_PREFIX):
            raise RuntimeError('unexpected public encoding')
        public = public_der[len(SPKI_PREFIX):]
        fingerprint = hashlib.sha256(public).hexdigest()
        stage = 'signature-test'
        with tempfile.TemporaryDirectory(prefix='autokey-public-key-test-') as directory:
            temp = Path(directory)
            (temp / 'challenge').write_bytes(b'AutoKeyboardLayot package signing key self-test v1\n')
            (temp / 'public.der').write_bytes(public_der)
            signature = run([OPENSSL, 'pkeyutl', '-sign', '-rawin', '-inkey', '/dev/stdin', '-keyform', 'DER',
                             '-in', str(temp / 'challenge')], private, env=crypto_env)
            (temp / 'signature').write_bytes(signature)
            run([OPENSSL, 'pkeyutl', '-verify', '-rawin', '-pubin', '-inkey', str(temp / 'public.der'),
                 '-keyform', 'DER', '-in', str(temp / 'challenge'), '-sigfile', str(temp / 'signature')], env=crypto_env)
        receipt.update(public_key_hex=public.hex(), fingerprint_sha256=fingerprint,
                       private_key_may_exist=True, state='protecting')
        save_receipt()
        public_args = ['-PublicKey', public.hex(), '-Fingerprint', fingerprint]
        stage = 'protect'
        created = run(helper + ['-Mode', 'create', *public_args], private, timeout=45).decode('utf-8-sig').splitlines()
        if created != ['CREATED', ready[1]]:
            raise RuntimeError('unexpected creation receipt')
        stage = 'verify-new-process'
        verified = run(helper + ['-Mode', 'verify', *public_args], hashlib.sha256(private).digest(), timeout=45).decode('utf-8-sig').splitlines()
        if verified != ['VERIFIED', ready[1]]:
            raise RuntimeError('stored key verification failed')
        metadata = {'format': 1, 'algorithm': 'Ed25519', 'signer': args.operation,
                    'public_key_hex': public.hex(), 'fingerprint_sha256': fingerprint,
                    'repository': receipt['repository']}
        with (args.receipt_directory / 'public-key.json').open('x', encoding='utf-8') as stream:
            json.dump(metadata, stream, indent=2)
            stream.flush()
            os.fsync(stream.fileno())
        receipt.update(state='verified', signature_self_test=True, dpapi_cross_process_test=True)
        save_receipt()
        print(json.dumps(receipt))
    except Exception:
        receipt.update(state='needs-inspection', failed_stage=stage)
        save_receipt()
        print('Key operation needs inspection; do not repeat generation. See the nonsecret receipt.')
        raise SystemExit(1)
    finally:
        if private is not None:
            private[:] = b'\0' * len(private)


if __name__ == '__main__':
    main()
