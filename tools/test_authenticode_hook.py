import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import authenticode_hook as hook  # noqa: E402

THUMBPRINT = 'a1b2c3d4e5f60718293a4b5c6d7e8f9012345678'
STAMP = 'https://timestamp.example.test/rfc3161'


class ConfigurationTests(unittest.TestCase):
    def setUp(self):
        self._directory = tempfile.TemporaryDirectory()
        self.addCleanup(self._directory.cleanup)
        self.tool = Path(self._directory.name) / 'SignTool.exe'
        self.tool.write_bytes(b'not a real tool')

    def env(self, **changes):
        values = {hook.TOOL: str(self.tool), hook.THUMBPRINT: THUMBPRINT, hook.TIMESTAMP_URL: STAMP}
        values.update(changes)
        return {key: value for key, value in values.items() if value is not None}

    def test_signing_is_off_unless_something_is_set(self):
        self.assertIsNone(hook.configuration({}))
        self.assertIsNone(hook.configuration({'PATH': '/usr/bin', 'HOME': '/root'}))
        self.assertIsNone(hook.configuration({hook.TOOL: '  ', hook.THUMBPRINT: '', hook.TIMESTAMP_URL: ''}))

    def test_a_complete_configuration_is_accepted(self):
        config = hook.configuration(self.env())
        self.assertEqual((config.tool, config.thumbprint, config.timestamp_url),
                         (str(self.tool), THUMBPRINT.upper(), STAMP))
        self.assertEqual(config.describe(), {'signed': True, 'thumbprint': THUMBPRINT.upper(), 'timestamp_url': STAMP})
        for url in ('http://timestamp.example.test', 'https://t.example.test:8443/ts?x=1'):
            self.assertIsNotNone(hook.configuration(self.env(**{hook.TIMESTAMP_URL: url})))

    def test_a_half_configured_run_stops_instead_of_building_unsigned_files(self):
        for name in (hook.TOOL, hook.THUMBPRINT, hook.TIMESTAMP_URL):
            with self.assertRaises(hook.SigningConfigurationError) as caught:
                hook.configuration(self.env(**{name: None}))
            self.assertIn(name, str(caught.exception))
            self.assertIn('half configured', str(caught.exception))
        with self.assertRaises(hook.SigningConfigurationError):
            hook.configuration({hook.TOOL: str(self.tool)})
        with self.assertRaises(hook.SigningConfigurationError):
            hook.configuration(self.env(**{hook.THUMBPRINT: ''}))

    def test_unusable_values_are_refused(self):
        bad = {
            hook.THUMBPRINT: [THUMBPRINT[:-1], THUMBPRINT + '0', 'z' * 40, THUMBPRINT[:20] + ' ' + THUMBPRINT[21:],
                              '"' + THUMBPRINT[1:], THUMBPRINT + '\n/p secret'],
            hook.TIMESTAMP_URL: ['ftp://t.example.test', 'timestamp.example.test', 'javascript:alert(1)',
                                 'https://t.example.test/ a', 'https://t.example.test/"', "https://t.example.test/'",
                                 'https://', 'file:///c:/x', 'https://t.example.test/x y'],
            hook.TOOL: [str(self.tool) + '.missing', str(self.tool.parent / 'other.exe'), str(self.tool.parent),
                        '/usr/bin/true'],
        }
        for name, values in bad.items():
            for value in values:
                with self.assertRaises(hook.SigningConfigurationError, msg=f'{name}={value!r}'):
                    hook.configuration(self.env(**{name: value}))
        (self.tool.parent / 'other.exe').write_bytes(b'x')  # exists, but is not signtool.exe
        with self.assertRaises(hook.SigningConfigurationError):
            hook.configuration(self.env(**{hook.TOOL: str(self.tool.parent / 'other.exe')}))


class CommandTests(unittest.TestCase):
    def setUp(self):
        self.config = hook.Config('C:/kit/signtool.exe', THUMBPRINT.upper(), STAMP)

    def test_the_sign_command_selects_the_certificate_by_thumbprint_only(self):
        command = hook.sign_command(self.config, 'C:/out/app.exe')
        self.assertEqual(command, ['C:/kit/signtool.exe', 'sign', '/fd', 'SHA256', '/sha1', THUMBPRINT.upper(),
                                   '/tr', STAMP, '/td', 'SHA256', 'C:/out/app.exe'])
        # No password, no certificate file and no key container ever appears.
        for option in ('/p', '/f', '/csp', '/kc', '/pfx', '/ac', '/a'):
            self.assertNotIn(option, command)

    def test_the_verify_command_uses_the_default_authenticode_policy(self):
        self.assertEqual(hook.verify_command(self.config, 'C:/out/app.exe'),
                         ['C:/kit/signtool.exe', 'verify', '/pa', '/v', 'C:/out/app.exe'])


class RunTests(unittest.TestCase):
    def setUp(self):
        self.config = hook.Config('C:/kit/signtool.exe', THUMBPRINT.upper(), STAMP)
        self.calls = []

    def runner(self, fail_on=None):
        def run(command, **options):
            self.calls.append((command, options))
            if fail_on and command[1] == fail_on[0] and command[-1] == fail_on[1]:
                raise subprocess.CalledProcessError(1, command)
            return subprocess.CompletedProcess(command, 0)
        return run

    def test_each_file_is_signed_then_verified_before_the_next(self):
        # Plain strings: a Path would print with the separators of the system the test runs on.
        hook.sign_and_verify(self.config, ['/a/app.exe', '/a/helper.exe'], run=self.runner(),
                             to_windows=lambda path: 'W:' + path)
        self.assertEqual([(call[0][1], call[0][-1]) for call in self.calls],
                         [('sign', 'W:/a/app.exe'), ('verify', 'W:/a/app.exe'),
                          ('sign', 'W:/a/helper.exe'), ('verify', 'W:/a/helper.exe')])
        for _, options in self.calls:
            self.assertIs(options['check'], True)
            self.assertIs(options['stdin'], subprocess.DEVNULL)
            self.assertLessEqual(options['timeout'], 300)

    def test_a_failed_signature_or_verification_stops_everything(self):
        with self.assertRaises(subprocess.CalledProcessError):
            hook.sign_and_verify(self.config, ['one.exe', 'two.exe'], run=self.runner(fail_on=('verify', 'one.exe')))
        self.assertEqual([call[0][1] for call in self.calls], ['sign', 'verify'])
        self.calls.clear()
        with self.assertRaises(subprocess.CalledProcessError):
            hook.sign_and_verify(self.config, ['one.exe', 'two.exe'], run=self.runner(fail_on=('sign', 'one.exe')))
        self.assertEqual([call[0][1] for call in self.calls], ['sign'])


class BuildScriptWiringTests(unittest.TestCase):
    """The build script cannot run here (it needs the notices, a build receipt and Inno Setup), so
    its text is checked: configuration first, signed copies before the installer is compiled, the
    installer signed before the receipt, and the receipt says whether anything was signed."""

    def setUp(self):
        self.text = (TOOLS / 'build_experimental_installer.py').read_text(encoding='utf-8')

    def position(self, fragment):
        self.assertEqual(self.text.count(fragment), 1, fragment)
        return self.text.index(fragment)

    def test_signing_is_decided_before_any_build_work_and_wraps_the_installer_compile(self):
        configure = self.position('authenticode_hook.configuration(os.environ)')
        clippy = self.position("subprocess.run([*cargo, 'clippy'")
        sign_inputs = self.position('authenticode_hook.sign_and_verify(signing, [app, helper]')
        compile_installer = self.position('subprocess.run(command, cwd=root, timeout=1200, check=True)')
        sign_installer = self.position('authenticode_hook.sign_and_verify(signing, [artifacts[0]]')
        receipt = self.position('receipt = {')
        self.assertLess(configure, clippy)
        self.assertLess(sign_inputs, compile_installer)
        self.assertLess(compile_installer, sign_installer)
        self.assertLess(sign_installer, receipt)

    def test_copies_are_signed_not_the_build_outputs_and_the_receipt_records_it(self):
        self.assertIn("signed_inputs = output / 'signed-inputs'", self.text)
        self.assertIn('shutil.copy2(app, signed_inputs / app.name)', self.text)
        self.assertIn("'authenticode': signing.describe() if signing else {'signed': False}", self.text)


if __name__ == '__main__':
    unittest.main()
