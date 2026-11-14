"""Step 08 of tools/verify-wsl.sh beyond the discovery run of tools/test_*.py.

  python_test_step.py torch             run the torch tests with the layout-model environment
  python_test_step.py detail LOG FLOOR  print the step's detail from its log; exit 1 when it fails

The system Python has no torch, so the discovery run skips the tests that train the layout model
(and, on the autocorrect branch, the ranker's). When target/layout-model/.venv exists
(docs/layout-model.md) they run again with its Python, on the CPU only: CUDA_VISIBLE_DEVICES is
empty, so no GPU of the machine is touched. A skip there means the environment lacks torch and
counts as a failure, as does a test that fails.
"""
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TORCH_TESTS = ('test_layout_model_training.py', 'test_autocorrect_ranker.py')


def summary(output):
    """(tests run, tests skipped, failures and errors) from the last unittest summary in `output`."""
    runs = re.findall(r'^Ran (\d+) tests? in ', output, re.M)
    endings = re.findall(r'^(OK|FAILED)(?: \(([^)]*)\))?\s*$', output, re.M)
    counts = dict(re.findall(r'(\w+)=(\d+)', endings[-1][1])) if endings else {}
    return (int(runs[-1]) if runs else 0, int(counts.get('skipped', 0)),
            int(counts.get('failures', 0)) + int(counts.get('errors', 0)))


def torch_command(root=ROOT):
    """The command that runs the torch tests of this checkout, or None without the environment."""
    python = root / 'target/layout-model/.venv/bin/python'
    tests = [name[:-len('.py')] for name in TORCH_TESTS if (root / 'tools' / name).is_file()]
    if not python.is_file() or not tests:
        return None
    return [str(python), '-B', '-m', 'unittest', *tests]


def torch_environment(base=None):
    environment = dict(os.environ if base is None else base)
    environment['CUDA_VISIBLE_DEVICES'] = ''
    environment['PYTHONUTF8'] = '1'
    return environment


def torch_problem(ran, skipped, failed, exit_code):
    """'' when the torch run counts as passed, otherwise why it does not."""
    if failed or exit_code:
        return 'failed'
    if ran == 0:
        return 'no-tests'
    if skipped:
        return 'skipped'
    return ''


def run_torch(root=ROOT):
    command = torch_command(root)
    if command is None:
        print('torch_tests not-run reason=no-environment')
        return 0
    run = subprocess.run(command, cwd=root / 'tools', env=torch_environment(), capture_output=True, text=True,
                         timeout=1800)
    output = run.stdout + run.stderr
    print(output)
    ran, skipped, failed = summary(output)
    problem = torch_problem(ran, skipped, failed, run.returncode)
    print(f'torch_tests ran={ran} skipped={skipped} failed={failed}' + (f' problem={problem}' if problem else ''))
    return 1 if problem else 0


def detail(log, floor):
    """(the step's detail, whether it passed) from the step's log and the floor of its test count."""
    torch_lines = re.findall(r'^torch_tests (.*)$', log, re.M)
    discovery = log.split('\ntorch_tests')[0] if torch_lines else log
    tests, skipped, failed = summary(discovery.split('+ python3 tools/python_test_step.py torch')[0])
    text = f'tests={tests} skipped={skipped}'
    ok = failed == 0 and tests >= floor
    if not torch_lines:
        return text + ' (no torch result)', False
    torch = dict(re.findall(r'(\w+)=(\S+)', torch_lines[-1]))
    if 'not-run' in torch_lines[-1]:
        text += ' torch: not run (no target/layout-model/.venv)'
    else:
        text += f" torch: ran={torch.get('ran', 0)} skipped={torch.get('skipped', 0)}"
        if torch.get('problem'):
            text += f" (torch tests {torch['problem']})"
            ok = False
    if failed:
        text += ' (failed tests)'
    if tests < floor:
        text += f' (below the floor of {floor})'
    return text, ok


if __name__ == '__main__':
    if sys.argv[1:] == ['torch']:
        raise SystemExit(run_torch())
    if len(sys.argv) == 4 and sys.argv[1] == 'detail':
        text, passed = detail(Path(sys.argv[2]).read_text(encoding='utf-8', errors='replace'), int(sys.argv[3]))
        print(text)
        raise SystemExit(0 if passed else 1)
    raise SystemExit(__doc__)
