#!/usr/bin/env bash
# The local verification gate on Linux (the counterpart of tools/verify-wsl.sh, which needs a
# Windows host). One logged step at a time; see --help.
set -u

STEP_IDS=(01 02 03 04 05 06 07 08 09 10 11)
STEP_NAMES=(fmt clippy-default clippy-no-default clippy-installer clippy-signing test-default test-no-default test-signing validate-locales python-tests windows-check)

usage() {
    cat <<'EOF'
Usage: tools/verify-linux.sh [--keep-going] [--only STEPS] [--list] [--help]

Runs the complete local verification gate on Linux, one logged step at a time:
  01 fmt                cargo fmt --all -- --check
  02 clippy-default     cargo clippy --locked --all-targets -- -D warnings
  03 clippy-no-default  cargo clippy --locked --no-default-features --all-targets -- -D warnings
  04 clippy-installer   cargo clippy --locked --no-default-features --features installer-tools --all-targets -- -D warnings
  05 clippy-signing     cargo clippy --locked --features signing-tools --all-targets -- -D warnings
  06 test-default       AKL_REQUIRE_IBUS=1 cargo test --locked --all-targets
                        (includes the end-to-end tests against a private ibus-daemon)
  07 test-no-default    cargo test --locked --no-default-features
  08 test-signing       cargo test --locked --features signing-tools
  09 validate-locales   cargo run --locked --example validate_locales -- data/package-locales --require-complete
  10 python-tests       python3 -m unittest discover -s tools -p "test_*.py"
  11 windows-check      cargo clippy --locked --target x86_64-pc-windows-msvc --all-targets -- -D warnings, with the
                        default features and with the installer tools (type-checks the Windows agent without
                        linking it; skipped when the target's standard library is not installed)

Options:
  --keep-going   run every step even after a failure (default: stop at the first failure)
  --only STEPS   run only the listed steps; comma-separated numbers or names
  --list         print the steps and exit
  -h, --help     print this help

Logs and summary.txt are written to target/verify-linux/<UTC timestamp>/. The exit status is 0 only
when every selected step passed or was skipped for a stated reason.

Test-count floors: AKL_MIN_LINUX_LIB_TESTS (default 337) for the library tests and
AKL_MIN_LINUX_AGENT_TESTS (default 37) for the Linux agent tests of step 06. Raise them when tests
are added; never lower them to hide a deleted test.
EOF
}

keep_going=0
only=""
while [ $# -gt 0 ]; do
    case "$1" in
        --keep-going) keep_going=1 ;;
        --only)
            [ $# -ge 2 ] || { echo "verify-linux: --only needs a value" >&2; exit 2; }
            only=$2
            shift
            ;;
        --list)
            for index in "${!STEP_IDS[@]}"; do printf '%s %s\n' "${STEP_IDS[$index]}" "${STEP_NAMES[$index]}"; done
            exit 0
            ;;
        -h | --help) usage; exit 0 ;;
        *) echo "verify-linux: unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd) || exit 2
cd "$repo_root" || exit 2
min_lib=${AKL_MIN_LINUX_LIB_TESTS:-337}
min_agent=${AKL_MIN_LINUX_AGENT_TESTS:-37}

selected() {
    local id=$1 name=$2 item
    local -a items
    [ -z "$only" ] && return 0
    IFS=',' read -r -a items <<<"$only"
    for item in "${items[@]}"; do
        [ "$item" = "$id" ] || [ "$item" = "$name" ] && return 0
    done
    return 1
}

missing=0
for tool in cargo python3; do
    command -v "$tool" >/dev/null 2>&1 || { echo "verify-linux: missing required tool: $tool" >&2; missing=1; }
done
command -v ibus-daemon >/dev/null 2>&1 || {
    echo "verify-linux: ibus-daemon is missing (apt install ibus); step 06 needs it" >&2
    missing=1
}
[ "$missing" -eq 0 ] || { echo "verify-linux: preflight failed; nothing was run" >&2; exit 2; }

windows_target_installed() {
    command -v rustup >/dev/null 2>&1 && rustup target list --installed 2>/dev/null | grep -qx 'x86_64-pc-windows-msvc'
}

run_logged() {
    printf '+ %s\n' "$*"
    "$@"
}

step_fmt() { run_logged cargo fmt --all -- --check; }
step_clippy_default() { run_logged cargo clippy --locked --all-targets -- -D warnings; }
step_clippy_no_default() { run_logged cargo clippy --locked --no-default-features --all-targets -- -D warnings; }
step_clippy_installer() { run_logged cargo clippy --locked --no-default-features --features installer-tools --all-targets -- -D warnings; }
step_clippy_signing() { run_logged cargo clippy --locked --features signing-tools --all-targets -- -D warnings; }
step_test_default() { run_logged env AKL_REQUIRE_IBUS=1 cargo test --locked --all-targets; }
step_test_no_default() { run_logged cargo test --locked --no-default-features; }
step_test_signing() { run_logged cargo test --locked --features signing-tools; }
step_validate_locales() { run_logged cargo run --locked --example validate_locales -- data/package-locales --require-complete; }
step_python_tests() { run_logged env PYTHONUTF8=1 python3 -m unittest discover -s tools -p 'test_*.py'; }
step_windows_check() {
    if ! windows_target_installed; then
        echo "SKIP: the Rust target x86_64-pc-windows-msvc is not installed (rustup target add x86_64-pc-windows-msvc)"
        return 3
    fi
    run_logged cargo clippy --locked --target x86_64-pc-windows-msvc --all-targets -- -D warnings &&
        run_logged cargo clippy --locked --target x86_64-pc-windows-msvc --no-default-features --features installer-tools --all-targets -- -D warnings
}

# The number of passed tests in the result line that follows "Running unittests <source>".
passed_after() {
    awk -v source="$2" '
        index($0, "Running unittests " source) { armed = 1; next }
        armed && /^test result:/ { for (i = 1; i <= NF; i++) if ($i == "passed;") print $(i - 1); exit }
    ' "$1"
}

step_detail=""
evaluate_counts() {
    local log=$1 lib agent
    lib=$(passed_after "$log" "src/lib.rs")
    agent=$(passed_after "$log" "src/main.rs")
    step_detail="lib=${lib:-?} agent=${agent:-?}"
    if [ -z "$lib" ] || [ "$lib" -lt "$min_lib" ]; then
        step_detail="$step_detail (library floor $min_lib)"
        return 1
    fi
    if [ -z "$agent" ] || [ "$agent" -lt "$min_agent" ]; then
        step_detail="$step_detail (agent floor $min_agent)"
        return 1
    fi
}

stamp=$(date -u +%Y%m%dT%H%M%SZ)
out_dir="target/verify-linux/$stamp"
mkdir -p "$out_dir" || { echo "verify-linux: cannot create $out_dir" >&2; exit 2; }
declare -a record_lines=()
run_start=$SECONDS

record() {
    record_lines+=("$(printf '  %s %-18s %-8s %5ss  %s' "$1" "$2" "$3" "$4" "$5")")
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
    log="$out_dir/$id-$name.log"
    printf '[%s] %s ...\n' "$id" "$name"
    begin=$SECONDS
    "step_${name//-/_}" >"$log" 2>&1
    status=$?
    step_detail=""
    if [ "$status" -eq 0 ] && [ "$name" = test-default ]; then
        evaluate_counts "$log" || status=1
    fi
    elapsed=$((SECONDS - begin))
    if [ "$status" -eq 3 ]; then
        detail=$(head -n 1 "$log")
        record "$id" "$name" SKIPPED "$elapsed" "$detail"
        printf '[%s] %s SKIPPED: %s\n' "$id" "$name" "$detail"
    elif [ "$status" -eq 0 ]; then
        record "$id" "$name" PASS "$elapsed" "$step_detail"
        printf '[%s] %s PASS (%ss) %s\n' "$id" "$name" "$elapsed" "$step_detail"
    else
        record "$id" "$name" FAIL "$elapsed" "$step_detail"
        printf '[%s] %s FAIL (%ss) %s\n--- last lines of %s ---\n' "$id" "$name" "$elapsed" "$step_detail" "$log"
        tail -n 25 "$log"
        overall=1
        [ "$keep_going" -eq 1 ] || stopped=1
    fi
done

commit=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)
changes=$(git status --porcelain --untracked-files=no 2>/dev/null | grep -c .)
{
    printf 'verify-linux %s at %s (%s)\n' "$stamp" "$commit" "$([ "$changes" -eq 0 ] && echo 'tree clean' || echo "$changes tracked files changed")"
    printf '%s\n' "${record_lines[@]}"
    printf 'result:   %s in %ss\n' "$([ "$overall" -eq 0 ] && echo PASS || echo FAIL)" "$((SECONDS - run_start))"
} | tee "$out_dir/summary.txt"
printf 'logs:     %s\n' "$out_dir"
exit "$overall"
