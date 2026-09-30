#!/usr/bin/env bash
# Local verification gate for AutoKeyboardLayot, for use from WSL.
#
# Runs formatting, Clippy in three configurations, tests in two configurations,
# strict locale validation, the Python unit tests and the Windows unit tests.
# The Windows tests are cross-built with cargo-xwin and executed on the Windows
# host with LOCALAPPDATA pointing at a temporary directory, so the real user
# profile is never touched. Nothing here needs credentials.
#
# Usage: tools/verify-wsl.sh [--keep-going] [--only STEPS] [--list] [--help]

set -u
set -o pipefail

STEP_IDS=(01 02 03 04 05 06 07 08 09)
STEP_NAMES=(fmt clippy-default clippy-installer clippy-windows test-default test-no-default validate-locales python-tests windows-tests)

usage() {
    cat <<'EOF'
Usage: tools/verify-wsl.sh [--keep-going] [--only STEPS] [--list] [--help]

Runs the complete local verification gate from WSL, one logged step at a time:
  01 fmt               cargo fmt --all -- --check
  02 clippy-default    cargo clippy --locked --all-targets -- -D warnings
  03 clippy-installer  cargo clippy --locked --no-default-features --features installer-tools --all-targets -- -D warnings
  04 clippy-windows    cargo clippy --locked --target x86_64-pc-windows-msvc --all-targets -- -D warnings
  05 test-default      cargo test --locked --all-targets
  06 test-no-default   cargo test --locked --no-default-features
  07 validate-locales  cargo run --locked --example validate_locales -- data/package-locales --require-complete
  08 python-tests      python3 -m unittest discover -s tools -p "test_*.py"
  09 windows-tests     cross-build the Windows unit tests with cargo-xwin and run them on the
                       Windows host with LOCALAPPDATA pointing at a temporary directory

Options:
  --keep-going   run every step even after a failure (default: stop at the first failure)
  --only STEPS   run only the listed steps; comma-separated numbers or names, for example 01,07,windows-tests
  --list         print the steps and exit
  -h, --help     print this help

Logs and summary.txt are written to target/verify/<UTC timestamp>/. The exit status is 0 only when
every selected step passed.

Test-count floors: AKL_MIN_LINUX_LIB_TESTS (default 281) applies to the library tests of step 05 and
AKL_MIN_WINDOWS_TESTS (default 126) to step 09. Raise them when tests are added; never lower them to
hide a deleted test.
EOF
}

list_steps() {
    local index
    for index in "${!STEP_IDS[@]}"; do
        printf '%s %s\n' "${STEP_IDS[$index]}" "${STEP_NAMES[$index]}"
    done
}

keep_going=0
only=""
while [ $# -gt 0 ]; do
    case "$1" in
        --keep-going) keep_going=1 ;;
        --only)
            if [ $# -lt 2 ]; then
                echo "verify-wsl: --only needs a value" >&2
                exit 2
            fi
            only=$2
            shift
            ;;
        --list)
            list_steps
            exit 0
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "verify-wsl: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
    shift
done

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd) || exit 2
cd "$repo_root" || exit 2

min_linux_lib=${AKL_MIN_LINUX_LIB_TESTS:-281}
min_windows=${AKL_MIN_WINDOWS_TESTS:-126}

# Step selection ---------------------------------------------------------------

# True when the step (number or name) is part of this run.
selected() {
    local id=$1 name=$2 item
    local -a items
    [ -z "$only" ] && return 0
    IFS=',' read -r -a items <<<"$only"
    for item in "${items[@]}"; do
        if [ "$item" = "$id" ] || [ "$item" = "$name" ]; then
            return 0
        fi
    done
    return 1
}

if [ -n "$only" ]; then
    IFS=',' read -r -a requested <<<"$only"
    for item in "${requested[@]}"; do
        known=0
        for index in "${!STEP_IDS[@]}"; do
            if [ "$item" = "${STEP_IDS[$index]}" ] || [ "$item" = "${STEP_NAMES[$index]}" ]; then
                known=1
            fi
        done
        if [ "$known" -eq 0 ]; then
            echo "verify-wsl: unknown step in --only: $item (see --list)" >&2
            exit 2
        fi
    done
fi

# True when at least one of the named steps is selected.
any_selected() {
    local wanted index
    for wanted in "$@"; do
        for index in "${!STEP_NAMES[@]}"; do
            if [ "${STEP_NAMES[$index]}" = "$wanted" ] && selected "${STEP_IDS[$index]}" "$wanted"; then
                return 0
            fi
        done
    done
    return 1
}

# Preflight --------------------------------------------------------------------

missing=0

need_tool() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "verify-wsl: missing required tool: $1 ($2)" >&2
        missing=1
    fi
}

need_tool cargo "install Rust with rustup"
if command -v cargo >/dev/null 2>&1; then
    cargo fmt --version >/dev/null 2>&1 || {
        echo "verify-wsl: rustfmt is missing (rustup component add rustfmt)" >&2
        missing=1
    }
    cargo clippy --version >/dev/null 2>&1 || {
        echo "verify-wsl: clippy is missing (rustup component add clippy)" >&2
        missing=1
    }
fi
if any_selected python-tests; then
    need_tool python3 "needed for the Python unit tests"
fi
if any_selected clippy-windows windows-tests; then
    if command -v rustup >/dev/null 2>&1 && ! rustup target list --installed 2>/dev/null | grep -qx 'x86_64-pc-windows-msvc'; then
        echo "verify-wsl: Rust target x86_64-pc-windows-msvc is missing (rustup target add x86_64-pc-windows-msvc)" >&2
        missing=1
    fi
fi
if any_selected windows-tests; then
    need_tool powershell.exe "run from WSL with Windows interop enabled; the Windows unit tests execute on the host"
    need_tool iconv "encodes the PowerShell command"
    need_tool base64 "encodes the PowerShell command"
    need_tool timeout "bounds the Windows test run"
    if command -v cargo >/dev/null 2>&1 && ! cargo xwin --version >/dev/null 2>&1; then
        echo "verify-wsl: cargo-xwin is not installed (cargo install --locked cargo-xwin)" >&2
        missing=1
    fi
fi
if [ "$missing" -ne 0 ]; then
    echo "verify-wsl: preflight failed; nothing was run" >&2
    exit 2
fi

# Steps ------------------------------------------------------------------------

run_logged() {
    printf '+ %s\n' "$*"
    "$@"
}

step_fmt() { run_logged cargo fmt --all -- --check; }
step_clippy_default() { run_logged cargo clippy --locked --all-targets -- -D warnings; }
step_clippy_installer() { run_logged cargo clippy --locked --no-default-features --features installer-tools --all-targets -- -D warnings; }
step_clippy_windows() { run_logged cargo clippy --locked --target x86_64-pc-windows-msvc --all-targets -- -D warnings; }
step_test_default() { run_logged cargo test --locked --all-targets; }
step_test_no_default() { run_logged cargo test --locked --no-default-features; }
step_validate_locales() { run_logged cargo run --locked --example validate_locales -- data/package-locales --require-complete; }
step_python_tests() { run_logged env PYTHONUTF8=1 python3 -m unittest discover -s tools -p 'test_*.py'; }

# Runs one test executable on the Windows host with a temporary LOCALAPPDATA.
# The executable path reaches PowerShell through WSLENV, which translates it to a Windows path.
run_windows_executable() {
    local executable=$1 script encoded host_dir
    script=$(
        cat <<'POWERSHELL'
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$work = Join-Path ([IO.Path]::GetTempPath()) ('akl-verify-' + [guid]::NewGuid().ToString('N'))
$profileDirectory = Join-Path $work 'profile'
New-Item -ItemType Directory -Path $profileDirectory | Out-Null
$exitCode = 1
try {
    $env:LOCALAPPDATA = $profileDirectory
    $stdout = Join-Path $work 'stdout.log'
    $stderr = Join-Path $work 'stderr.log'
    $process = Start-Process -FilePath $env:AKL_TEST_EXE -ArgumentList '--test-threads=1' -WorkingDirectory $work `
        -PassThru -NoNewWindow -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $null = $process.Handle
    if (-not $process.WaitForExit(600000)) {
        $process.Kill()
        throw 'the Windows test executable did not finish within 600 seconds'
    }
    $process.WaitForExit()
    $exitCode = $process.ExitCode
    Get-Content -LiteralPath $stdout
    Get-Content -LiteralPath $stderr
    Write-Output ('windows test executable exit code: ' + $exitCode)
} finally {
    Remove-Item -Recurse -Force -LiteralPath $work -ErrorAction SilentlyContinue
}
exit $exitCode
POWERSHELL
    )
    encoded=$(printf '%s' "$script" | iconv -f UTF-8 -t UTF-16LE | base64 -w0) || return 1
    host_dir=$(dirname "$(command -v powershell.exe)")
    (
        cd "$host_dir" || exit 1
        AKL_TEST_EXE="$executable" WSLENV="AKL_TEST_EXE/p${WSLENV:+:$WSLENV}" \
            timeout 900 powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand "$encoded" 2>&1 | tr -d '\r'
    )
}

step_windows_tests() {
    local build_output status executable_relative found
    build_output=$(mktemp) || return 1
    printf '+ %s\n' "cargo xwin test --locked --no-run --target x86_64-pc-windows-msvc --bin AutoKeyboardLayot --target-dir target/xwin-hotkeys"
    cargo xwin test --locked --no-run --target x86_64-pc-windows-msvc --bin AutoKeyboardLayot --target-dir target/xwin-hotkeys >"$build_output" 2>&1
    status=$?
    cat "$build_output"
    if [ "$status" -ne 0 ]; then
        rm -f "$build_output"
        return 1
    fi
    executable_relative=$(sed -n 's/^[[:space:]]*Executable .* (\(.*\.exe\))[[:space:]]*$/\1/p' "$build_output")
    rm -f "$build_output"
    found=$(printf '%s\n' "$executable_relative" | grep -c .)
    if [ "$found" -ne 1 ]; then
        echo "verify-wsl: expected exactly one test executable in the cargo output, found $found"
        return 1
    fi
    if [ ! -f "$repo_root/$executable_relative" ]; then
        echo "verify-wsl: test executable not found: $executable_relative"
        return 1
    fi
    printf '+ running %s on the Windows host with a temporary LOCALAPPDATA\n' "$executable_relative"
    run_windows_executable "$repo_root/$executable_relative"
}

# Test counts ------------------------------------------------------------------

# Prints "passed failed ignored lib_passed" summed over every "test result:" line of a libtest log.
# lib_passed is the count of the library unit-test binary (the block that starts with "Running unittests src/lib.rs").
parse_test_counts() {
    tr -d '\r' <"$1" | awk '
        /^[[:space:]]*Running / { current = $0 }
        /^test result: / {
            for (i = 2; i <= NF; i++) {
                if ($i == "passed;") { passed += $(i - 1); if (current ~ /src\/lib\.rs/) { lib += $(i - 1) } }
                if ($i == "failed;") { failed += $(i - 1) }
                if ($i == "ignored;") { ignored += $(i - 1) }
            }
        }
        END { printf "%d %d %d %d\n", passed, failed, ignored, lib }
    '
}

# Sets step_detail and returns non-zero when a test count violates its floor or a test failed.
evaluate_counts() {
    local name=$1 log=$2 passed failed ignored lib
    read -r passed failed ignored lib < <(parse_test_counts "$log")
    step_detail="passed=$passed failed=$failed ignored=$ignored"
    if [ "$name" = test-default ]; then
        step_detail="$step_detail lib=$lib"
    fi
    if [ "$failed" -ne 0 ]; then
        step_detail="$step_detail (failed tests)"
        return 1
    fi
    if [ "$passed" -eq 0 ]; then
        step_detail="$step_detail (no test results found)"
        return 1
    fi
    if [ "$name" = test-default ] && [ "$lib" -lt "$min_linux_lib" ]; then
        step_detail="$step_detail (lib tests below the floor of $min_linux_lib)"
        return 1
    fi
    if [ "$name" = windows-tests ] && [ "$passed" -lt "$min_windows" ]; then
        step_detail="$step_detail (below the floor of $min_windows)"
        return 1
    fi
    return 0
}

# Runner -----------------------------------------------------------------------

format_duration() {
    if [ "$1" -ge 60 ]; then
        printf '%dm%02ds' $(($1 / 60)) $(($1 % 60))
    else
        printf '%ds' "$1"
    fi
}

stamp=$(date -u +%Y%m%dT%H%M%SZ)
out_dir="target/verify/$stamp"
mkdir -p "$out_dir" || {
    echo "verify-wsl: cannot create $out_dir" >&2
    exit 2
}

declare -a record_lines=()
step_detail=""
started_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
run_start=$SECONDS

record() {
    local line
    line=$(printf '  %s %-18s %-8s %7s  %s' "$1" "$2" "$3" "$(format_duration "$4")" "$5")
    record_lines+=("$line")
}

run_one() {
    local id=$1 name=$2 function_name log status begin elapsed
    function_name="step_${name//-/_}"
    log="$out_dir/$id-$name.log"
    printf '[%s] %s ...\n' "$id" "$name"
    begin=$SECONDS
    "$function_name" >"$log" 2>&1
    status=$?
    step_detail=""
    case "$name" in
        test-default | test-no-default | windows-tests)
            evaluate_counts "$name" "$log" || status=1
            ;;
    esac
    elapsed=$((SECONDS - begin))
    if [ "$status" -eq 0 ]; then
        record "$id" "$name" PASS "$elapsed" "$step_detail"
        printf '[%s] %s PASS (%s) %s\n' "$id" "$name" "$(format_duration "$elapsed")" "$step_detail"
        return 0
    fi
    record "$id" "$name" FAIL "$elapsed" "$step_detail"
    printf '[%s] %s FAIL (%s) %s\n' "$id" "$name" "$(format_duration "$elapsed")" "$step_detail"
    printf -- '--- last lines of %s ---\n' "$log"
    tail -n 25 "$log"
    printf -- '---\n'
    return 1
}

overall=0
stopped=0
for index in "${!STEP_IDS[@]}"; do
    id=${STEP_IDS[$index]}
    name=${STEP_NAMES[$index]}
    selected "$id" "$name" || continue
    if [ "$stopped" -eq 1 ]; then
        record "$id" "$name" SKIPPED 0 "an earlier step failed"
        continue
    fi
    if ! run_one "$id" "$name"; then
        overall=1
        if [ "$keep_going" -eq 0 ]; then
            stopped=1
        fi
    fi
done

elapsed_total=$((SECONDS - run_start))
if git rev-parse --git-dir >/dev/null 2>&1; then
    commit=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)
    branch=$(git branch --show-current 2>/dev/null)
    changes=$(git status --porcelain --untracked-files=no 2>/dev/null | grep -c .)
    if [ "$changes" -eq 0 ]; then
        tree="tree clean"
    else
        tree="tree has $changes tracked change(s)"
    fi
    commit_line="$commit (branch ${branch:-detached}, $tree)"
else
    commit_line="not a git checkout"
fi

{
    echo "verify-wsl summary"
    echo "commit:   $commit_line"
    echo "rustc:    $(rustc --version 2>/dev/null || echo unknown)"
    echo "started:  $started_at"
    echo "finished: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "elapsed:  $(format_duration "$elapsed_total")"
    echo "logs:     $out_dir"
    echo "steps:"
    printf '%s\n' "${record_lines[@]}"
    if [ "$overall" -eq 0 ]; then
        echo "result:   PASS"
    else
        echo "result:   FAIL"
    fi
} | tee "$out_dir/summary.txt"

exit "$overall"
