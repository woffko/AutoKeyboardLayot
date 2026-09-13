# AutoKeyboardLayot handoff — 2026-09-13

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
