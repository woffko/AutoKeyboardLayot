# AutoKeyboardLayot handoff — 2026-09-16

## Stop point and authority

Local source snapshot; the worktree is clean and the latest work is committed as
`d79456b` on `main`. The commits below are **not pushed** to
`woffko/AutoKeyboardLayot` (no push was requested). Machine-local
AGENTS/configuration, VM scripts, credentials, build outputs and acceptance
receipts are intentionally not part of this source snapshot; a remote clone alone
cannot reproduce the existing test equipment. The Ed25519 signer
`pkg-20260912-01` private material is encrypted outside the repository — never
regenerate, export, or commit it.

## 2026-09-16 update — manual conversion, single-letter auto conversion, catalogs

Current user-visible behaviour is working on the host: manual Pause conversion
cycles a word through the enabled layouts, and opt-in single-letter automatic
conversion (`z ` → `я `, `ф ` → `a `, `Ш ` → `I `) works after installing
`ru-RU` revision 3.

### Manual (Pause) conversion
- `execute_forced_conversion` / `advance_layout_cycle` / `TransposeCycle` in
  `src/windows_agent.rs` walk a word through every enabled layout, skipping any
  layout that produces identical text (Latin Estonian vs Latin English), so one
  press always reaches a visibly different representation.
- Cycle matching uses `same_input_target` (ignores the layout), so the layout
  switch performed by a conversion no longer breaks the next press.
- A press while a manual cycle is active advances it instead of arming undo;
  forced conversions do not offer the one-press undo. Buffered keys survive a
  failed attempt. Commits `397ffcd`, `b3be64e`, `78e5b7f`, `fb0b845`.
- Buffered previous word survives profile refreshes and a following space
  (`f0da7a0`, `72c265a`).

### Privacy
- `PRIVACY_PROBE_WAIT_MS` is 300 ms.
- A user-initiated (forced) conversion is allowed when UI Automation cannot
  verify the context (for example a console), while a confirmed password field,
  an excluded process or an elevated target still blocks it
  (`forced_privacy_allows`, `privacy_blocks`); automatic conversion stays
  fail-closed and does not run in Windows Terminal. Commit `5671504`.
- Layout resolution no longer trusts `ImmIsIME` on ordinary layouts
  (`c93dfc1`); transient profile-provider failures keep the last snapshot
  (`4e07a6c`, `a274157`).

### Single-letter automatic conversion (opt-in)
- Plan and decisions: [plan-single-letter-auto-conversion](docs/plan-single-letter-auto-conversion.md).
- `DetectorConfig::single_letter_words` (default `false`). When on, one-character
  words use the list-only `detect_single_letter` policy: only the short-word tier
  or the user dictionary counts, the base tier is ignored, and excluded,
  correctly typed, identical or ambiguous cases fail closed.
- Embedded `en-US` short tier ships `a` and `i`; `ru-RU` revision 3 adds `я`.
- The two-letter tier is intentionally **not** curated; a specific unwanted
  conversion is suppressed with a user-dictionary entry (for example
  `en-US: vs`). Documented and covered by a regression test.
- UI: `general.single_letter_words` checkbox in General; the thirteen optional
  `data/package-locales` catalogs were backfilled with the keys the earlier
  language UI redesign had left missing. Commit `d79456b`.

### Language UI and packages
- Input-languages page redesigned: active-language list first, an "Add language"
  dialog for catalog downloads, advanced package tools collapsed, interface-only
  packages labelled, content-sized settings cards and a model-driven checkmark.
  Commits `e53982b`, `d513241`, `f0f622e`, `fb0b845`.

### Published assets (GitHub releases, accepted by the running app)
- `lang-r2-20260913`: catalog revision 2, thirteen UI-only packages plus the
  combined `ru-RU rev2` input package.
- `lang-r3-20260914`: catalog revision 3, adds `et-EE rev2` (input).
- `lang-r4-20260916`: catalog revision 4, replaces `ru-RU` with **rev3** (adds
  `я` to the short tier). Verified `releases/latest/download/catalog.aklc` sha
  `44bdc3998a7a210e787c7b39f1c33e560bb0f937fc06aadede42aae8c7056c41`.
- The catalog is fetched from
  `https://github.com/<repo>/releases/latest/download/catalog.aklc`.
- Signing tools `prepare_language_package`, `prepare_release_catalog` and
  `sign-language-package` embed the English catalog, so they must be rebuilt
  whenever `data/locales/en.json` changes; the current signer sha is
  `6042c9abeac2d65b0a39bfe6d9158fdd2372fd38ccd7dbd0c369e1c187848c7b`
  (the earlier pin `af80acd1…` is stale).

### Host state
- Config: `single_letter_words=true`, `enabled_input_packs=en-us,et-ee,ru-ru`,
  hotkey `Pause/Break`, diagnostics on. The installed store has `et-ee rev2` and
  `ru-ru rev2`; the accepted catalog is revision 3, so `ru-RU rev3` still needs
  installing through "Add language" before `z ` → `я ` works.

### Verification gates
- `cargo test --lib`: 269 passed, 1 ignored; `cargo test --test
  package_locale_sources`: passed; `cargo clippy --target x86_64-pc-windows-msvc
  --features installer-tools --all-targets -- -D warnings`: clean;
  `cargo check … --no-default-features --features installer-tools`: clean;
  `examples/validate_locales --require-complete`: passes for all 14 locales.

### Open items
1. Install `ru-RU rev3` on the host and run the Slice 5 typing checklist from the
   single-letter plan.
2. No push; `d79456b` and the preceding local commits exist only locally.
3. Automatic conversion in Windows Terminal remains blocked (privacy by design).
4. Two-letter false positives remain uncurated by decision (variant C).
5. The historical Stage 5/6 gates below (composition/IME, physical typing
   acceptance, all-language review) are unchanged.

---

## Previous handoff (2026-09-13) — historical

## Stop point and authority

The user requested this handoff, a GitHub source update, and a Project Memory
checkpoint. This is an experimental source snapshot, NOT a release or a claim
that the product is ready. The durable Goal is currently **paused**; do not
rewrite its objective, reactivate it, or start tests merely to finish this
handoff. No Longrun job is pending. Preserve the existing worktree and fixtures.

Authoritative product plan: [localization-and-language-packs](docs/localization-and-language-packs.md).
Main repository: `woffko/AutoKeyboardLayot`. Project Memory key:
`AutoKeyboardLayot`. Machine-local AGENTS/configuration, VM scripts, credentials,
build outputs and acceptance receipts are intentionally not part of this source
snapshot. A remote clone alone cannot reproduce the existing test equipment.

## 2026-09-13 update — offline local-package import implemented

This update supersedes the "not yet diagnosed" wording below for the immediate
failure. The local-catalog failure cause is confirmed by code inspection and a
native reproduction: a locally loaded `.aklc` populated rows, but the download
stage used `PinnedPackage::url()` (GitHub release URLs) and never read the
adjacent `.aklp` files, so an unpublished tag failed. The host helper reply/log
was not retained in the Windows temp path, so the managed-store-open edge case
was not separately observed.

Implemented on top of commit `6e6863d` (uncommitted worktree):

- A local catalog now binds a local source directory; selected artifacts are
  read from that directory by the validated asset name and never fall back to
  the network (`CatalogSource`, `PreparedCatalog::from_local_bytes`,
  `read_local_artifact`). All pinned checks remain: length, SHA-256, Ed25519,
  package ID, revision, input participation and UI locale.
- The installer helper uses the local source for a `local_file` catalog and
  reports a coarse, path-free failure reason code that the Inno page appends to
  `AkPackageFailed`.
- Documentation updated: `online-package-manager.md`,
  `installer-package-selection.md`, `package-release-catalog.md`; plan saved at
  `docs/plan-2026-09-13.md`.

Verified evidence:

- Linux `cargo test --lib`: 261 passed; `cargo fmt --check`; strict Clippy on
  Linux and `x86_64-pc-windows-msvc`; 26 Python tool tests.
- Windows cross-build checks: `tools/check_windows_base.py` and
  `tools/check_installer_helper.py` passed (`cargo xwin` build + clippy).
- Native Windows helper test `tools/test-installer-helper-offline.ps1` passed
  `check_catalog(local) -> select -> confirm_download -> confirm_install` with
  `network_used=false`, an advanced store pointer and the installed blob present
  (receipt `target/offline-helper-native-20260913-01/result.json`).

Still open: the full native `verify-windows.ps1` pipeline; a real local-catalog
install through the actual setup UI (candidate3 below); the live simultaneous
different-session check; and all Stage 3-6 gates. The `pkg-20260912-01` key
handling and no-publication rules are unchanged. The user grants standing
permission to run the Windows build pipeline at any time.

Stage 2 core passed in the approved VM: the fence refused installation while a
shared reader was held (helper code 12), then the candidate installed with the
expected app hash and preserved profile/store/sentinel and no started agent, then
uninstalled cleanly with the fence released
(`target/vm-installer-fence-20260913-03/transaction-result.json`, `state=passed`).
The fence script was fixed to discover the actual Inno uninstaller via the
registration `UninstallString` and to retry the final fence check while the
relayed silent uninstaller child finishes. The live different-session check is
still separate.

Installer candidate3 was built with the offline fix and the reason-code message
(`target/installer-vm-candidate-20260913-03/`, ISCC successful compile). It is
the artifact for Stage 3 acceptance and is not published.

Stage 3 is largely passed. The native Windows helper lifecycle test
(`tools/test-installer-helper-lifecycle.ps1`) covers zero-selection no-op,
multiple-package install, cancel revocation, tampered-local-artifact rejection
with the `verification` reason code, and retry
(`target/offline-helper-lifecycle-20260913-01/result.json`). The real candidate3
setup was then driven through its UI in the VM's interactive console session
(user `w0w`) with a local catalog and one package: download disabled without a
selection, 13 catalog rows, explicit RU selection, local download with no
network, license review showing the package identity and MIT text, confirmed
installation with the expected executable hash, normal close, and a follow-up
uninstall that restored the baseline while preserving the profile/store
(`target/vm-ui-offline-session-20260913-13/result.json`, `state=passed`, with
page screenshots). A separate parent-exit scenario passed too:
`tools/test-installer-helper-parent-exit.ps1` confirmed the helper exits with
code 0 when its installer parent is terminated
(`target/offline-helper-parent-exit-20260913-04/result.json`). The live
simultaneous different-session exclusion check also passed: a session-0 holder
kept the shared installation fence for the target user open while the setup ran
silently as that user in the interactive session 1; it was refused with helper
code 12 and the fence was released once the holder stopped
(`target/vm-cross-session-20260913-08/result.json`, `state=passed`).

Stage 4 (upgrade/interruption) also passed: candidate1 was installed, user data
was added, candidate3 was installed over it with the binary changed and the
config/store/lexicon/sentinel preserved, a later install was terminated
mid-flight and a re-install recovered a complete install with the data intact,
and a silent uninstall removed the app while keeping the profile/store
(`target/vm-upgrade-20260913-01/result.json`, `state=passed`). Both builds use
the same `0.1.0` version string, so this is a binary replacement between builds
rather than a version-number upgrade. Stage 5 (input/physical) and Stage 6
(language, accessibility, notices) remain, plus the full native
`verify-windows.ps1` pipeline. Stage 5 currently has only EN/RU/ET input data and
only the `physical-key-v1` implementation; `altgr-v1`, `dead-key-v1`,
composition/IME and grapheme adapters are unimplemented, and physical typing
acceptance is interactive and outside the no-global-input-automation boundary.
`tools/notepad-uia-probe.ps1` is a TextPattern capability check, not a typing or
conversion test. Stage 5 and Stage 6 therefore need product direction and human
or interactive acceptance before they can be completed. The user chose the
composition/IME adapter first and will run the manual checklist
`docs/physical-input-acceptance.md`; the first-slice design and verified/UNKNOWN
facts are in `docs/composition-ime-adapter-plan.md`. The offline-import work and
harnesses were committed as `1f8f8c1`.

## Immediate user-visible issue

The user received installer candidate2 and the entire local UI-package candidate
folder in Windows Downloads. They first saw the legacy-profile refusal. Advice
was to close the app and setup and rename the profile to a backup for a clean
test, never delete it. Whether they actually renamed it is **not verified**.
This is NOT a completed migration of legacy dictionaries/settings.

Their next screenshot shows a successfully populated local catalog, selected
Estonian and Russian rows, and a generic package-operation failure. Exact
failure cause is **not yet diagnosed from logs**. The suspected cause is that
opening a local catalog still uses its GitHub download URLs, while candidate
assets have never been published. Do not report that hypothesis as confirmed.
Copying adjacent `.aklp` files does not establish offline installer import.

Next diagnostic action after resumption: identify the failed helper operation
and its saved reply/log, read the actual local-catalog/download path, and verify
whether it uses network URLs, cache, or an explicit local source. Do not blindly
retry an uncertain install. Do not publish assets just to hide the failure.
Then implement/test a clear supported local-package workflow if needed under
the existing offline-import requirement, with signature/hash/size checks and
explicit review. Improve the generic failure message with useful safe context.
Keep online download and local import distinct in UI and documentation.

## Existing implementation and verified evidence

- Base `--no-default-features` build embeds English only. Default legacy build
  retains EN/RU/ET. UI language is independent of input readiness.
- Data-driven registry/config migration, signed package/catalog verification,
  package store, manager and installer helper are implemented; broad acceptance
  remains incomplete. Never bypass fail-closed profile/store checks.
- User selected MIT for app and project-authored translations; root LICENSE and
  dependency notices exist. Third-party dictionaries retain their own terms.
- Ed25519 signer `pkg-20260912-01` already exists, with public metadata in
  `data/package-signing/public-key.json`. Private material is encrypted outside
  the repository. **Never regenerate, export, or commit it.** Package signing
  is not Windows Authenticode signing.
- 13 signed UI-only `.aklp` candidates plus embedded English cover 14 UI locales;
  this does not establish 14-language correction/IME support. Local store and
  selected-catalog rehearsals passed on Linux and Windows; no remote download
  acceptance or public asset publication has occurred.
- Windows CURRENT replacement now uses `SetFileInformationByHandle` with
  `FileRenameInfoEx` and replace/POSIX flags. Win32 class value 22 is intentional;
  an external advisory confused it with NT class 65. No power-loss namespace
  durability guarantee. See [pointer publication](docs/windows-pointer-publication.md).
- Latest recorded native results: 257 Windows core tests and 11 selected
  installer boundaries passed. These were not rerun for this documentation handoff.
- Prior installer VM tests passed clean install, same-version repair/uninstall
  with preserved profile/store/sentinel, and legacy-profile refusal. This is
  not an actual upgrade from an installed old binary or interactive typing test.
- Candidate2 adds a shared agent/settings lifetime file handle and exclusive
  installer handle across sessions. A VM exclusive-lock probe blocked app and
  settings startup and preserved config. Actual simultaneous interactive
  different-session acceptance remains pending.

## Local artifacts and exact unfinished VM test

Paths below are relative to the existing workspace and ignored by Git:

- `target/installer-vm-candidate-20260913-02/build.json`
- `target/installer-vm-candidate-20260913-02/AutoKeyboardLayot-0.1.0-setup-experimental.exe`
  SHA-256: `9207d1c215a2c4c73defb8ffde5b6f05de40de7450d33160457c46307e8a11ba`.
- `target/ui-package-candidates-20260912-01/`: 13 `.aklp`, `catalog.aklc`,
  verification/rehearsal receipts and unsigned preparation files. Use signed
  assets, not `.unsigned.json`, for installation.
- Catalog revision 1 references planned tag `ui-candidates-20260912-r1`; expiry
  is September 19, 2026. Recheck expiration before reuse and increment revisions
  when updating accepted catalogs. No release exists from this workflow.
- `target/base-notices-complete-20260913-01/`: complete collected license texts,
  bound to `target/base-build-receipt-20260913-01.json` (339 third-party packages).
- `target/windows-core-tests-20260913-01/`,
  `target/installer-boundary-tests-20260913-01/`.
- `target/vm-install-receipts-20260913-01/first-install-result.json`,
  `target/vm-repair-uninstall-20260913-02/controller-result.json`,
  `target/vm-legacy-refusal-20260913-01/controller-result.json`,
  `target/vm-fence-probe-20260913-01/controller-result.json`.

Downloads copies: `AutoKeyboardLayot-0.1.0-setup-experimental-candidate2.exe`
and folder `ui-package-candidates-20260912-01`. Copy equality was checked.

VM snapshot was manually created and confirmed. Do not demand another snapshot
name or restore it without considering later user changes. The guest was left
uninstalled with its managed profile and unknown-file sentinel preserved;
recheck live state before acting. Tests so far used SSH session 0.

Local-only `tools/vm_installer_fence.ps1` and
`tools/run_vm_installer_fence.py` are prepared but **not executed**. Python syntax
and diff checks passed; PowerShell parse validation remains pending. Planned
new receipt directory `target/vm-installer-fence-20260913-01` does not exist.
Test sequence: held shared reader must refuse setup with helper code 12;
release reader, install candidate2, uninstall, verify profile/CURRENT/sentinel
preserved and fence released. Check scripts and baseline before submitting once.
Latest completed startup-fence job was already read once; do not fetch/restart it.
Use enrolled encrypted test asset via Project Memory protected Longrun stdin;
never expose its scalar value or key. Recover uncertain operations through
durable per-operation process/exit/result receipts transferred over SFTP, not
by replaying setup. Windows stdout encoding previously obscured a successful
operation. A timeout is not proof of process termination.

## Remaining plan and release gates

1. Diagnose the screenshot failure and establish a usable local-package path.
2. Finish candidate2 refusal/install/uninstall test and actual cross-session
   runtime exclusion. Preserve all user data and avoid global input automation.
3. Test installer selection, explicit license review/install, zero/one/multiple
   selections, cancellation, failed downloads, helper/parent exit and recovery.
4. Validate old-binary upgrade and interruption behavior in the approved VM;
   do not equate same-version repair with upgrade. Do not assume reboot or
   snapshot rollback is authorized as part of a diagnostic step.
5. Complete missing input packs, calibration and composition/IME adapters;
   physically test typing, focus, undo, clipboard and protected-input boundaries
   in Notepad, Terminal and browser. Keep readiness conservative.
6. Finish all-language UI, RTL, fonts, accessibility and linguistic review.
7. Review OS/filesystem requirements, release notices and signing/recovery.
   Publish versioned package assets/catalog and installer only as a separately
   reviewed release action; this source handoff does not perform that action.

Do not mark the broad Goal complete until its original acceptance gates pass.
Automatic `ask-about-bug` use is now disabled by the latest user instructions;
only an explicit request for that exact tool permits it.
