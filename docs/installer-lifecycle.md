# Installer lifecycle prerequisites

The installer milestone is not complete. This document records the implemented
agent boundary and the remaining coordination requirements; it is not a release
acceptance certificate.

## Agent close

Tray Exit and `WM_CLOSE` use the same guarded request. Active input gates,
retained input, a busy input worker, pending configuration publication, and
unfinished hotkey release cause a refusal with a localized notification. A
refusal neither discards retained input nor schedules a later surprise exit.
The user can finish/recover the operation and explicitly retry.

An accepted request disconnects the input channel, advances its epoch, prevents
new hook-side processing and rejects new worker gate-arm requests. The window
continues pumping messages until the input thread has actually returned. Only
then does normal window destruction join it and release the hooks. A snapshot
of `worker_busy == false` is not used as evidence of thread termination.

An installer must wait for **process termination**, not a successful delivery
or return value of `WM_CLOSE`. No forced termination or timeout-based binary
replacement is authorized by this boundary. Unexpected process termination and
OS shutdown are not covered by the guarded close path.

## Installer technology and helper contract

The user selected the existing `woffko/AutoKeyboardLayot` repository for package
Releases. No separate GitHub repository is required. The signed catalog follows
`releases/latest/download/catalog.aklc`; each selected package is a separate
asset pinned by its catalog entry. Publication and signing-key provisioning
remain separate steps; repository selection grants neither implicitly.

Use Inno Setup for the per-user binary/uninstaller shell and the existing Rust
package APIs for signed-catalog selection and transactions. Do not reproduce
signature validation or archive extraction in Pascal Script. Configure
`PrivilegesRequired=lowest`, `CloseApplications=no`, and
`RestartApplications=no`. The upstream contracts are documented under
[non-administrative installation](https://jrsoftware.org/ishelp/topic_setup_privilegesrequired.htm),
[automatic application closure](https://jrsoftware.org/ishelp/topic_setup_closeapplications.htm),
and [application mutexes](https://jrsoftware.org/ishelp/topic_setup_appmutex.htm).

`AutoKeyboardLayot.exe --prepare-upgrade <absolute-install-directory>` is the
bounded process-close helper. It never starts hooks, installs packages, edits
files, or terminates a process forcibly. It handles settings before the agent.
Before sending `WM_CLOSE`, it opens the window's process, checks its canonical
executable path against `<directory>/AutoKeyboardLayot.exe`, and rechecks the
window PID. It also requires the versioned `AutoKeyboardLayot.GracefulClose.v1`
window property advertised by the guarded agent/settings code. Older versions
without that capability require manual closure; no `WM_CLOSE` is sent to them.
A portable instance from another directory is not closed. A request
waits at most 15 seconds per process. A missing window with a live singleton is
treated as startup/busy, not proof of process absence.

Exit codes: `0` means the observed processes exited and both singleton probes
were clear; `10` means invalid arguments/relative target; `11` means identity
could not be established; `12` means busy/refused/timeout or a failed request.

After exit code 0, the installer must atomically create and hold both exact
singleton names (`Local\AutoKeyboardLayot.Agent` and
`Local\AutoKeyboardLayot.Settings.Singleton`), checking `ERROR_ALREADY_EXISTS`
for **each** creation, with initial ownership enabled. On conflict, release its
own handles and stop before any
file replacement. Holding both handles prevents new instances through their
existing startup guards. Release the handles only after the operation finishes.
This lease is necessary: helper success alone cannot exclude a new startup.
The initial source in `installer/AutoKeyboardLayot.iss` uses this discipline for
setup and uninstall. Inno Setup 6.7.3 accepted it with output disabled (`/O-`);
this validates compilation, not installation, rollback or uninstall behavior.

`--initialize-installation` requires both mutexes to be owned by another thread
(the waiting installer), then acquires the configuration writer mutex. It only
initializes a profile when all known unified/legacy source files are absent.
The empty managed store is published first, then the English-only config with
autoconversion off. Existing profiles remain byte-for-byte unchanged; malformed
configuration and partial/unrelated stores fail without reset. Exit `20` means
missing lease/path/writer lock; `21` means profile initialization failed.

Initialization precedes binary installation in the current prototype. If a
later installer step fails or is cancelled, the new valid English-only profile
may remain; it is not recursively removed or claimed to be a completed install.

## Remaining work

The settings window refuses normal close while a package operation or file
picker is active. It also refuses if the current editor values are invalid or
different from the last saved document. Apply/OK saves explicitly; Cancel is
the explicit discard action. This guard is shared by native window close and
an externally delivered normal close request. It does not save on the user's
behalf or turn a refused close into a deferred exit.

- Coordinate the separate settings process from the installer; agent termination
  does not close settings. Test native close delivery to the pinned UI backend.
- Compile and exercise installer-side process waits and both owned leases.
- Complete per-user install/upgrade/repair/uninstall, explicit startup opt-in,
  Start menu integration, backup and retained-user-data policy.
- Complete the installer connection to the selected-package manager and signed
  GitHub distribution, without automatically fetching all languages. The runtime
  manager now uses the explicitly embedded `pkg-20260912-01` public release key.
- Add the selected-package page and complete visual/linguistic setup acceptance.
  All 14 product languages are wired and structurally checked.
- Validate the boundary on Windows during typing, retained-input recovery,
  package installation, and configuration reload. Compilation and unit tests do
  not establish physical input preservation or complete installer acceptance.

## Build variants

Default Cargo features include `legacy-bundled-input` for the transitional
EN/RU/ET application and its existing regression corpus. Build the modular base
with `--no-default-features`: the build script emits only EN FSTs, and the Rust
registries exclude RU/ET dictionaries, scoring models and input descriptors.
Legacy-data export/audit examples explicitly require the legacy feature.

The base refuses to activate an old bootstrap profile whose selected input
languages need absent bundled data. Installer helper exit `22` reports this
before replacing binaries; migrate those selections explicitly first. Managed
profiles and user data are preserved. Disabling selected languages or silently
downloading replacements is not a migration strategy.

Validation uses the full default-feature regression suite plus focused
`--no-default-features --test english_base_build` and
`--no-default-features --lib base_build` checks for the actual base registry and
profile transition. Default-feature tests are not evidence that a base Windows
artifact has been compiled, installed, or physically accepted.

The current helper also supports `--verify-modular-base`: exit `0` requires the
base feature set, while `23` identifies a legacy-bundled build. The setup source
checks this before closure or profile initialization. This is a build-variant
check on the reviewed payload, not a signature, source-provenance certificate,
or substitute for validating the installer bundle itself.

## Dependency notice collection

`python3 tools/collect_dependency_notices.py <new-output-directory>` collects
locally bundled license/notice files from Cargo's resolved Windows base metadata,
including build/test dependencies, plus the English dictionary license. It
records the Cargo.lock hash and each license file hash. It does not select among
license alternatives or determine redistribution permission.

Missing texts produce exit code 2, a missing-package report, and an explicitly
named `THIRD-PARTY-NOTICES.incomplete.txt`, not the finished installer input.
The initial complete filename scan found 379 dependency records and 21 packages
without local license text. Their upstream, version-matched texts still need
collection/review before supplying `BundleNotices` to the installer.

Upstream collection is available through `tools/fetch_dependency_notices.py`.
It reads public repository/revision metadata from the exact cached crate and
fetches only license texts at that commit. Merge its report using
`collect_dependency_notices.py <new-directory> --upstream <upstream-notices.json>`.
The merge checks Cargo.lock, crate revision/repository, dirty status and each
downloaded file hash before inclusion. URLs with queries/fragments are rejected.

The earlier metadata-projection collection had two unresolved entries: `simd_helpers 0.1.0`
has no located license text, and `skia-bindings 0.99.0` has collected reference
text but published VCS metadata with `dirty: true`. The output deliberately
remains `.incomplete.txt`; neither case is silently considered verified.

For the actual installer base, supply `--build-receipt <base-build-receipt.json>`
from `export_windows_test_artifacts.py`. The receipt binds Cargo.toml and
Cargo.lock hashes and records package IDs emitted by Cargo's actual
`compiler-artifact` messages. Collection then loads metadata for all platforms
and selects only those IDs, retaining both host build tools and Windows target
dependencies. Missing IDs or a changed configuration cause refusal, not omission.
[Cargo metadata](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html)
provides package descriptions; the compiler receipt supplies build membership.

The verified base test build emitted 340 package IDs including this application,
so its third-party collection contains 339 packages. Skia was not built in this
configuration; its dirty-source notice is not a base-bundle blocker. This does
not resolve its licensing/provenance status for other configurations. The base
collection still lacks the `simd_helpers 0.1.0` text and remains incomplete.

## Bounded Windows checks

The retained base and legacy executables passed the direct Windows feature
probe (`--verify-modular-base` returned 0 and 23 respectively), after SHA-256
verification. Launch used `ProcessStartInfo.UseShellExecute=false` and did not
start the keyboard agent. An earlier ShellExecute-based probe stalled and only
its identified PowerShell wrapper was stopped; it was not counted as a pass.

`tools/export_windows_test_artifacts.py` exports the exact base test executable
hashes from Cargo JSON. `tools/run-installer-boundary-tests.ps1` runs only the
following reviewed filters with temporary LOCALAPPDATA, direct process launch,
timeouts and expected nonzero test counts:

- Base model/descriptor absence: 2 tests.
- Base registry and profile transition: 2 tests.
- Installer identity/owned-mutex boundaries: 3 tests.
- Graceful worker termination guards: 2 tests.
- Unsaved settings close guard: 1 test.

All 10 passed on Windows. The receipt is
`target/windows-variants-20260909/installer-boundary-tests/result.json`.
This is neither a full Windows regression run nor physical typing acceptance.
The installer output, language-selection page and upgrade VM gates remain open.

## Setup message catalogs

Application-owned setup labels and errors live in `installer/messages/*.isl`,
not Pascal string literals. Each language loads its stock Inno messages, then
the English application fallback, then its own overrides. This ordering avoids
global English entries overwriting translations, following Inno's
[message-file ordering](https://jrsoftware.org/ishelp/topic_languagessection.htm).
Current EN/RU/DE/ES/FR/PT-BR/JA/AR/ZH-CN/ID/UR/ET/HI/BN catalogs contain the same 11 keys and preserve `%1`
arguments; source-coverage/placeholder tests and Inno 6.7.3 `/O-` compilation pass.
`LanguageDetectionMethod=uilanguage` configures OS display-language selection.
`UsePreviousLanguage=no` prevents the previous installer language from silently
overriding that default; this does not change any application preference.
The setup language does not rewrite application UI/input preferences. This is
14-language source coverage, not a visual/upgrade acceptance result.

The installed compiler provides stock translations for EN/RU/DE/ES/FR/PT-BR/JA/AR.
The Chinese Simplified and Indonesian stock catalogs are vendored separately
from upstream revision `1ae7bf81dc0d2013235dfe4bb0b6f4e4a0b6b25c`.
`installer/vendor/inno/SOURCES.json` records source and normalized file hashes;
upstream attribution comments are retained. The committed 6.7.3 reference schema
checks all 281 message keys, including the 277 nonempty required values and their
parameters. Chinese is listed upstream as official and Indonesian as unofficial;
schema compatibility does not establish translation quality or visual acceptance.
Both compile with the existing Inno 6.7.3 using `/O-` (no installer output).

The remaining candidate audit is in `target/inno-language-audit-20260912/audit.json`:
The original Urdu candidate was missing 23 required messages; these have now
been translated in a clearly marked local adaptation. Three obsolete keys were
removed, the native language name was added, and RTL was explicitly enabled.
Schema/hash/parameter tests and Inno 6.7.3 `/O-` compilation passed. Linguistic
review and visual RTL acceptance remain pending.
Bengali received 66 missing messages, a native language label and removal of
obsolete message/font keys. Its two earlier audit flags were valid repetitions
of `%1`, not translation errors; those repetitions are retained. The auditor
and regression tests now permit repeated uses while still rejecting missing or
unexpected parameter identities. The original candidate audit is historical.
All 14 catalogs pass the schema tests and Inno 6.7.3 `/O-` compilation without
warnings; this does not approve translation quality or visual behavior.
Hindi received 66 missing messages and
six corrections to existing text, including reversed OS-version requirements.
Two ignored legacy font options were removed after compiler diagnostics. Its
updates are maintained in `hindi-updates.json` and checked against the adapted
vendor file. Schema checks and Inno compilation pass; native linguistic and
visual review remain separate requirements.
Estonian was decoded from CP1257 to UTF-8, with accents retained; 66 messages
were added and seven obsolete keys removed. Its maintained additions are in
`installer/vendor/inno/estonian-additions.json`, checked against the vendored
catalog by tests. The adapted catalog passes schema/parameter tests and Inno
6.7.3 `/O-` compilation; linguistic and visual acceptance remain pending.
Do not label English stock pages with a different language name and call that
complete localization. Portuguese is explicitly the Brazilian variant here.
# Updated notice collection (2026-09-12)

The fresh `target/base-build-receipt-20260912-01.json` records 340 compiled
package IDs including the application (339 third-party packages; no Skia).
`target/base-notices-complete-20260912-01/notices-report.json` now reports
`complete_text_collection=true` and an empty `missing` list for that exact graph.
The 17 cached upstream notice groups were revalidated against current package
VCS revisions, clean flags, pinned URLs and content hashes before reuse under
the current lockfile. The old supplement/receipts were not overwritten.

The former simd_helpers text gap is resolved by the upstream license-only child
commit documented in `data/dependency-notices/simd_helpers-0.1.0/PROVENANCE.md`.
The narrow collector supplement checks package identity, source revision, MIT
metadata and exact text hash; it does not weaken general provenance checks.

This is complete text collection for the recorded Rust base test graph, not
blanket redistribution approval, a native installer acceptance report, or proof
that a previously built probe contains the updated notices. The first guarded
UI probe still contains its recorded incomplete notice file.
