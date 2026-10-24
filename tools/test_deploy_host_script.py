"""Static checks of tools/deploy-host.ps1, which only a Windows host can execute."""
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / 'tools/deploy-host.ps1'
DOCUMENT = ROOT / 'docs/host-deployment.md'


class DeployHostScriptTests(unittest.TestCase):
    def test_script_and_document_contain_no_machine_local_identifiers(self):
        forbidden = [r'192\.168\.', r'[A-Za-z]:\\Users\\', r'/home/', r'root[@]', r'wsl\.localhost', r'\\\\[A-Za-z0-9.-]+\\']
        for path in (SCRIPT, DOCUMENT):
            text = path.read_text(encoding='utf-8')
            for pattern in forbidden:
                with self.subTest(file=path.name, pattern=pattern):
                    self.assertIsNone(re.search(pattern, text), pattern)

    def test_script_is_plain_ascii_for_windows_powershell(self):
        # Windows PowerShell 5.1 reads a file without a byte order mark in the ANSI code page.
        self.assertTrue(all(byte < 128 for byte in SCRIPT.read_bytes()))

    def test_parameters_match_the_documented_interface(self):
        text = SCRIPT.read_text(encoding='utf-8')
        self.assertIn('[CmdletBinding(SupportsShouldProcess = $true)]', text)
        for parameter in ('$Source', '$ExpectedSha256', '$BackupSuffix'):
            self.assertRegex(text, r'Mandatory = \$true\)\][^\n]*' + re.escape(parameter))
        self.assertRegex(text, r'\[int\]\$KeepBackups = 3')
        document = DOCUMENT.read_text(encoding='utf-8')
        for name in ('-Source', '-ExpectedSha256', '-BackupSuffix', '-KeepBackups', '-WhatIf'):
            self.assertIn(name, document)

    def test_nothing_changes_before_the_dry_run_gate(self):
        text = SCRIPT.read_text(encoding='utf-8')
        gate = text.index('if ($dryRun) {')
        changes = ['Copy-Item -LiteralPath', 'Remove-Item -LiteralPath', 'Start-Process -FilePath',
                   'Stop-Process -Id', '[System.Diagnostics.Process]::Start(', 'Close-Agent $process']
        for change in changes:
            with self.subTest(change=change):
                self.assertIn(change, text)
                self.assertGreater(text.index(change), gate)
        # The preference is cleared before the read-only checks so that they run for real.
        self.assertLess(text.index('$WhatIfPreference = $false'), text.index('Get-FileHash -LiteralPath'))

    def test_an_agent_that_declines_to_close_keeps_running_and_nothing_changes(self):
        # The agent declines while it holds typed input; stopping it by force would lose that
        # input and, as on 2026-10-11, leave no agent running.
        text = SCRIPT.read_text(encoding='utf-8')
        body = text[text.index('if ($dryRun) {'):]
        close = body.index('if (-not (Close-Agent $process)) {')
        replacement = body.index('try {')
        self.assertLess(close, replacement)
        self.assertLess(close, body.index('Copy-Item -LiteralPath'))
        declined = body[close:replacement]
        self.assertIn('nothing was changed', declined)
        self.assertNotIn('Stop-Process', declined)

    def test_rollback_waits_for_the_agent_and_restores_only_a_replaced_executable(self):
        text = SCRIPT.read_text(encoding='utf-8')
        attempt = text[text.index('try {'):text.index('} catch {')]
        marked = attempt.index('$replaced = $true')
        self.assertLess(attempt.index('Copy-Item -LiteralPath $destination -Destination $backup'), marked)
        self.assertLess(marked, attempt.index('Copy-Item -LiteralPath $Source -Destination $destination -Force'))
        catch = text[text.index('} catch {'):]
        stop = catch.index('Stop-Process -Id $process.Id -Force')
        wait = catch.index('$process.WaitForExit(')
        restore = catch.index('if ($replaced) { Copy-Item -LiteralPath $backup -Destination $destination -Force }')
        self.assertLess(stop, wait)
        self.assertLess(wait, restore)
        self.assertIn('if ($running.Count -gt 0) { Start-Process -FilePath $destination }', catch)
        self.assertIn('Deployment failed and was rolled back', catch)


if __name__ == '__main__':
    unittest.main()
