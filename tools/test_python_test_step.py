import unittest
from pathlib import Path
import tempfile

import python_test_step as step

ROOT = Path(__file__).resolve().parents[1]

DISCOVERY = """....s.s..
----------------------------------------------------------------------
Ran 119 tests in 5.074s

OK (skipped=2)
"""


class PythonTestStepTests(unittest.TestCase):
    def test_unittest_output_gives_runs_skips_and_failures(self):
        self.assertEqual(step.summary(DISCOVERY), (119, 2, 0))
        self.assertEqual(step.summary('Ran 1 test in 0.1s\n\nOK\n'), (1, 0, 0))
        self.assertEqual(step.summary('Ran 7 tests in 1s\n\nFAILED (failures=2, errors=1, skipped=3)\n'), (7, 3, 3))
        self.assertEqual(step.summary('no unittest here'), (0, 0, 0))

    def test_the_torch_tests_run_on_the_cpu_with_the_layout_model_environment(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'tools').mkdir()
            (root / 'tools/test_layout_model_training.py').write_text('', encoding='utf-8')
            # Without the environment nothing runs.
            self.assertIsNone(step.torch_command(root))
            python = root / 'target/layout-model/.venv/bin/python'
            python.parent.mkdir(parents=True)
            python.write_text('', encoding='utf-8')
            self.assertEqual(step.torch_command(root), [str(python), '-B', '-m', 'unittest', 'test_layout_model_training'])
            # The autocorrect ranker's tests join where that branch has them.
            (root / 'tools/test_autocorrect_ranker.py').write_text('', encoding='utf-8')
            self.assertEqual(step.torch_command(root)[-2:], ['test_layout_model_training', 'test_autocorrect_ranker'])
        environment = step.torch_environment({'PATH': '/bin', 'CUDA_VISIBLE_DEVICES': '0'})
        self.assertEqual(environment['CUDA_VISIBLE_DEVICES'], '')
        self.assertEqual(environment['PATH'], '/bin')

    def test_a_torch_run_counts_only_when_every_test_ran_and_passed(self):
        self.assertEqual(step.torch_problem(2, 0, 0, 0), '')
        # The environment exists but lacks torch: its tests skip, which hides them as before.
        self.assertEqual(step.torch_problem(2, 2, 0, 0), 'skipped')
        self.assertEqual(step.torch_problem(2, 0, 1, 1), 'failed')
        self.assertEqual(step.torch_problem(0, 0, 0, 0), 'no-tests')
        self.assertEqual(step.torch_problem(2, 0, 0, 1), 'failed')

    def test_the_step_detail_holds_both_runs_and_the_floor(self):
        torch_line = 'torch_tests ran=2 skipped=0 failed=0\n'
        detail, ok = step.detail(DISCOVERY + torch_line, 119)
        self.assertTrue(ok)
        self.assertEqual(detail, 'tests=119 skipped=2 torch: ran=2 skipped=0')
        detail, ok = step.detail(DISCOVERY + torch_line, 120)
        self.assertFalse(ok)
        self.assertIn('below the floor of 120', detail)
        detail, ok = step.detail(DISCOVERY + 'torch_tests not-run reason=no-environment\n', 119)
        self.assertTrue(ok)
        self.assertEqual(detail, 'tests=119 skipped=2 torch: not run (no target/layout-model/.venv)')
        detail, ok = step.detail(DISCOVERY + 'torch_tests ran=2 skipped=2 failed=0 problem=skipped\n', 119)
        self.assertFalse(ok)
        self.assertIn('torch tests skipped', detail)
        detail, ok = step.detail(DISCOVERY, 119)
        self.assertFalse(ok)
        self.assertIn('no torch result', detail)


class VerifyWslWiringTests(unittest.TestCase):
    def test_the_python_step_runs_the_torch_tests_and_checks_its_floor(self):
        text = (ROOT / 'tools/verify-wsl.sh').read_text(encoding='utf-8')
        self.assertIn("run_logged env PYTHONUTF8=1 python3 -m unittest discover -s tools -p 'test_*.py' &&\n"
                      "        run_logged python3 tools/python_test_step.py torch", text)
        self.assertIn('python3 tools/python_test_step.py detail "$log" "$min_python"', text)
        self.assertIn('min_python=${AKL_MIN_PYTHON_TESTS:-', text)

    def test_ci_says_that_it_does_not_run_the_torch_tests(self):
        text = (ROOT / '.github/workflows/checks.yml').read_text(encoding='utf-8')
        self.assertIn('::notice title=Torch tests not run::', text)


if __name__ == '__main__':
    unittest.main()
