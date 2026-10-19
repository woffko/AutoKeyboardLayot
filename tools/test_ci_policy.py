"""Keeps the dependency and CI policy from being weakened by accident.

The workflow cannot run locally, so these tests read the files that define the
policy: the accepted advisories, the license and source rules, and the CI steps
that enforce them.
"""
import re
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = (ROOT / '.github/workflows/checks.yml').read_text(encoding='utf-8')
LINUX_ONLY = "runner.os == 'Linux'"
# Commands that enforce the policy, with the condition of their step (None: every system). Each must
# be a command line of its own in a step that runs it for real: not commented out, not followed by
# `|| true` or similar, in a step without continue-on-error and without any other `if:`.
ENFORCING = {
    r'cargo install cargo-audit --version \d+\.\d+\.\d+ --locked': LINUX_ONLY,
    r'cargo audit --deny warnings': LINUX_ONLY,
    r'cargo install cargo-deny --version \d+\.\d+\.\d+ --locked': LINUX_ONLY,
    r'cargo deny check licenses bans sources': LINUX_ONLY,
    r'cargo clippy --locked --features signing-tools --all-targets -- -D warnings': None,
    # --all-targets runs the unit tests of the examples that need signing-tools, too.
    r'cargo test --locked --features signing-tools --all-targets -- --test-threads=1': None,
    r'python -m unittest discover -s tools -p "test_\*\.py"': None,
}
# The reviewed license allow-list of deny.toml, exactly: any addition is a decision to review.
ALLOWED_LICENSES = {
    'MIT', 'Apache-2.0', 'Apache-2.0 WITH LLVM-exception', 'BSD-2-Clause', 'BSD-3-Clause', 'ISC',
    'Unicode-3.0', 'Zlib', '0BSD', 'CC0-1.0', 'BSL-1.0', 'Unlicense', 'LicenseRef-Slint-Royalty-free-2.0',
}


def workflow_steps(text):
    """The steps of the workflow's job, as dicts of key to value; `run` is a list of command lines.

    Written for the flat layout of this repository's workflows: steps at six spaces (`      - `),
    their keys at eight, block scalars (`run: |`) indented further. Job-level keys are returned
    under the key `job`.
    """
    steps, job, current, block = [], {}, None, None
    for line in text.splitlines():
        if block is not None:
            if line.startswith(' ' * 10) or not line.strip():
                current['run'].append(line.strip())
                continue
            block = None
        if line.startswith('      - '):
            current = {'run': []}
            steps.append(current)
            line = '        ' + line[8:]
        if current is not None and line.startswith('        ') and not line.startswith('         '):
            key, _, value = line.strip().partition(':')
            value = value.strip()
            if key == 'run':
                if value in ('|', '>'):
                    block = key
                else:
                    current['run'].append(value)
            else:
                current[key] = value
        elif line.startswith('    ') and not line.startswith('     ') and ':' in line:
            key, _, value = line.strip().partition(':')
            job[key] = value.strip()
    return steps, job

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
        enforced(self, r'cargo audit --deny warnings')
        enforced(self, r'cargo install cargo-audit --version \d+\.\d+\.\d+ --locked')


class DenyPolicy(unittest.TestCase):
    def setUp(self):
        self.policy = tomllib.loads((ROOT / 'deny.toml').read_text(encoding='utf-8'))

    def test_only_crates_io_is_a_source(self):
        self.assertEqual(self.policy['sources']['unknown-registry'], 'deny')
        self.assertEqual(self.policy['sources']['unknown-git'], 'deny')
        self.assertNotIn('allow-git', self.policy['sources'])
        self.assertNotIn('allow-registry', self.policy['sources'])

    def test_license_allow_list_is_exactly_the_reviewed_one(self):
        allowed = self.policy['licenses']['allow']
        self.assertEqual(len(allowed), len(set(allowed)))
        self.assertEqual(set(allowed), ALLOWED_LICENSES)
        self.assertNotIn('exceptions', self.policy['licenses'])

    def test_both_ci_platforms_are_in_the_graph(self):
        targets = self.policy['graph']['targets']
        self.assertIn('x86_64-pc-windows-msvc', targets)
        self.assertIn('x86_64-unknown-linux-gnu', targets)
        self.assertTrue(self.policy['graph']['all-features'])

    def test_ci_runs_a_pinned_cargo_deny(self):
        enforced(self, r'cargo deny check licenses bans sources')
        enforced(self, r'cargo install cargo-deny --version \d+\.\d+\.\d+ --locked')


def enforced(case, pattern):
    """Asserts that the command `pattern` runs for real in the workflow, in a step as ENFORCING wants."""
    steps, job = workflow_steps(WORKFLOW)
    case.assertNotIn('continue-on-error', job)
    found = [(step, line) for step in steps for line in step['run'] if re.fullmatch(pattern, line)]
    case.assertTrue(found, f'no command line runs {pattern}')
    for step, line in found:
        case.assertNotIn('continue-on-error', step, line)
        case.assertEqual(step.get('if'), ENFORCING[pattern], line)
        case.assertFalse(any(line.startswith('set +e') for line in step['run']), line)
    return found


class WorkflowSteps(unittest.TestCase):
    def test_every_enforcing_command_runs_for_real(self):
        for pattern in ENFORCING:
            with self.subTest(pattern=pattern):
                enforced(self, pattern)

    def test_no_step_may_fail_without_failing_the_job(self):
        steps, job = workflow_steps(WORKFLOW)
        self.assertNotIn('continue-on-error', job)
        for step in steps:
            self.assertNotIn('continue-on-error', step, step)
            for line in step['run']:
                self.assertNotRegex(line, r'\|\|\s*(true|exit 0|:)\s*$', line)
            self.assertIn(step.get('if'), (None, LINUX_ONLY), step)

    def test_the_reader_finds_what_a_weakened_workflow_hides(self):
        global WORKFLOW
        original = WORKFLOW
        weakened = {
            'commented out': original.replace('cargo audit --deny warnings', '# cargo audit --deny warnings'),
            'ignored failure': original.replace('cargo audit --deny warnings', 'cargo audit --deny warnings || true'),
            'allowed to fail': original.replace("      - name: Dependency advisories\n",
                                                 "      - name: Dependency advisories\n        continue-on-error: true\n"),
            'never run': original.replace("        if: runner.os == 'Linux'\n        run: |\n          cargo install cargo-audit",
                                          "        if: false\n        run: |\n          cargo install cargo-audit"),
        }
        try:
            for case, text in weakened.items():
                self.assertNotEqual(text, original, case)
                WORKFLOW = text
                with self.subTest(case=case), self.assertRaises(AssertionError):
                    enforced(self, r'cargo audit --deny warnings')
        finally:
            WORKFLOW = original


class SigningToolsInCi(unittest.TestCase):
    def test_clippy_and_tests_build_the_signing_tools(self):
        enforced(self, r'cargo clippy --locked --features signing-tools --all-targets -- -D warnings')
        enforced(self, r'cargo test --locked --features signing-tools --all-targets -- --test-threads=1')

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
