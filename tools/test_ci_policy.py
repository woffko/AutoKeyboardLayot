"""Keeps the dependency and CI policy from being weakened by accident.

The workflow cannot run locally, so these tests read the files that define the
policy: the accepted advisories, the license and source rules, and the CI steps
that enforce them.
"""
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = (ROOT / '.github/workflows/checks.yml').read_text(encoding='utf-8')

ACCEPTED_ADVISORIES = {
    'RUSTSEC-2025-0141',  # bincode, optional dependency listed in Cargo.lock only
    'RUSTSEC-2024-0436',  # paste, build-time proc macro of the Slint compiler
    'RUSTSEC-2026-0206',  # rustybuzz, Slint SVG text shaping
    'RUSTSEC-2026-0192',  # ttf-parser, Slint SVG font parsing
}


class AuditPolicy(unittest.TestCase):
    def test_exactly_the_four_reviewed_notices_are_ignored(self):
        policy = tomllib.loads((ROOT / '.cargo/audit.toml').read_text(encoding='utf-8'))
        self.assertEqual(set(policy['advisories']['ignore']), ACCEPTED_ADVISORIES)
        self.assertEqual(len(policy['advisories']['ignore']), len(ACCEPTED_ADVISORIES))

    def test_every_ignored_notice_has_a_reason_in_the_file(self):
        text = (ROOT / '.cargo/audit.toml').read_text(encoding='utf-8')
        for advisory in ACCEPTED_ADVISORIES:
            comment = [line for line in text.splitlines() if line.startswith('#') and advisory in line]
            self.assertTrue(comment, f'{advisory} needs a comment that says why it is accepted')

    def test_ci_denies_warnings_with_a_pinned_cargo_audit(self):
        self.assertIn('cargo audit --deny warnings', WORKFLOW)
        self.assertRegex(WORKFLOW, r'cargo install cargo-audit --version \d+\.\d+\.\d+ --locked')


class DenyPolicy(unittest.TestCase):
    def setUp(self):
        self.policy = tomllib.loads((ROOT / 'deny.toml').read_text(encoding='utf-8'))

    def test_only_crates_io_is_a_source(self):
        self.assertEqual(self.policy['sources']['unknown-registry'], 'deny')
        self.assertEqual(self.policy['sources']['unknown-git'], 'deny')
        self.assertNotIn('allow-git', self.policy['sources'])
        self.assertNotIn('allow-registry', self.policy['sources'])

    def test_license_allow_list_is_explicit(self):
        allowed = self.policy['licenses']['allow']
        self.assertIn('LicenseRef-Slint-Royalty-free-2.0', allowed)
        self.assertEqual(len(allowed), len(set(allowed)))
        for copyleft in ('GPL-3.0-only', 'GPL-3.0-or-later', 'AGPL-3.0-only', 'LGPL-3.0-only'):
            self.assertNotIn(copyleft, allowed)

    def test_both_ci_platforms_are_in_the_graph(self):
        targets = self.policy['graph']['targets']
        self.assertIn('x86_64-pc-windows-msvc', targets)
        self.assertIn('x86_64-unknown-linux-gnu', targets)
        self.assertTrue(self.policy['graph']['all-features'])

    def test_ci_runs_a_pinned_cargo_deny(self):
        self.assertIn('cargo deny check licenses bans sources', WORKFLOW)
        self.assertRegex(WORKFLOW, r'cargo install cargo-deny --version \d+\.\d+\.\d+ --locked')


class SigningToolsInCi(unittest.TestCase):
    def test_clippy_and_tests_build_the_signing_tools(self):
        self.assertIn('cargo clippy --locked --features signing-tools --all-targets -- -D warnings', WORKFLOW)
        self.assertIn('cargo test --locked --features signing-tools', WORKFLOW)

    def test_the_matrix_covers_both_systems(self):
        self.assertRegex(WORKFLOW, r'os:\s*\[ubuntu-latest,\s*windows-latest\]')
        # A plain step has no `if:` line below it, so it runs on every system of the matrix.
        lines = WORKFLOW.splitlines()
        for index, line in enumerate(lines):
            if 'features signing-tools' in line:
                self.assertTrue(line.startswith('      - run: '), line)
                self.assertFalse(lines[index + 1].lstrip().startswith('if:'), line)

if __name__ == '__main__':
    unittest.main()
