# Installer package-selection integration (in progress)

`installer_packages::InstallerPackageSelection` owns one `PreparedCatalog` and
an initially empty selection. Its rows expose authenticated package identity,
revision, byte size, UI locale, input presence and compatibility. Checkbox
changes pass the existing full-plan verifier; failures leave the previous
selection unchanged. Refresh must create a new model, clearing selection.
Confirmation rechecks trust/time and returns the existing `SelectedDownload`,
or no transaction for the English-only path. It does not write to the store,
download, enable input or change the UI preference.

The portable fixture test verifies empty defaults, selected-only totals and
plans, failed-choice preservation, no store writes and revoked-trust refusal.
The model is now connected to the Inno source pages through the native helper;
the guarded English preview is exercised below, not full installer acceptance.

The native installer adapter retains the authenticated catalog and selection,
runs the existing selected-download worker and exposes authenticated review
files. The Inno source connects these to selection/review pages and separate
confirmations. Production download/commit, cancellation and shutdown across
page transitions still require end-to-end native acceptance.
It must use the same per-user store and installer lifecycle leases; it must not
invoke the settings window or bypass its singleton by copying package files.
UI preference and input participation remain separate, explicit configuration
choices. No helper IPC or Inno page implementation is claimed by this module.

Reuse the existing `PreparedCatalog::from_github`, `select`,
`SelectedDownload::accept_and_download_with_progress` and
`PreparedOnlineInstall::confirm` boundaries rather than reimplementing signature,
cache, rollback or transaction rules in Pascal. The license choice, published
signed catalog/assets, native UI/network and isolated install/upgrade acceptance
remain open gates.

## Session and native bridge direction

The selected bridge is a separate helper executable, not a Rust DLL loaded by
Inno. This avoids tying setup cancellation/unload to an in-process synchronous
network worker whose timeout is cooperative, and avoids setup/helper DLL bitness
coupling. The abstract advisory review favored DLL state ownership; the main
implementation instead retains that ownership in the helper process.

`installer_session::InstallerSession` now consumes selection on download
approval, associates worker results and license review with generation tickets,
rejects late/duplicate completions after cancellation, and consumes installation
approval exactly once. The transaction worker still owns actual confirmation,
error/uncertain-commit reporting and its lifetime. Cancellation of UI approval
does not stop a worker by itself. Installation cannot be revoked midway through
the commit by this state model.

The executable/IPC adapter is still pending. It must create a fresh session,
monitor the actual setup process handle, use bounded sequenced requests and
responses, never resume old on-disk approvals after restart, monotonically cancel
on parent death/disconnection, and observe worker completion before a new worker.
Display INI data from `page_data` is not a trusted authorization token; requests
must resolve against the retained authenticated Rust state. Fresh-profile preview
must not initialize a real profile merely because the package page was visited.

The request decoder is now implemented in `installer_protocol`: format-1 nested
JSON, at most 8192 bytes, exact fresh-session ID, strictly increasing sequence,
known actions only, and at most 64 unique normalized selected IDs. Approval
commands carry a view number which the native adapter must compare with the
retained session before invoking consuming core operations. Decoding alone never
authorizes an operation. The native executable, IPC filesystem/parent-handle
lifetime and Inno UI integration need native acceptance. The helper executable
is now implemented behind `installer-tools`; it has not been included in an
installer or connected to its wizard pages.

The existing-store helper preview passed natively on Windows using the actual
signed local catalog: handshake, 13-row catalog, explicit selection, stale-view
download rejection, cancellation, normal close and unchanged store pointer.
Receipt: `target/ui-package-candidates-20260912-01/helper-preview-windows.json`.
The fixture is retained at the exact path in that receipt for inspection; no
network or live application profile was used.

Fresh-store support is implemented as an empty **session-only** preview
store. Download/install always opens the real store and rechecks its expected
snapshot; the helper never initializes the real profile. Once a real store has
been observed, its disappearance cannot select the empty preview again. The
native fresh-profile preview passed with the real signed local catalog, explicit
selection, stale approval refusal, cancel and normal close; the requested real
store path remained absent. Receipt:
`target/ui-package-candidates-20260912-01/helper-fresh-preview-windows.json`.
Its test-only session directory is retained for inspection. The caller must provide a trusted
per-user session parent and hold both actual installer leases during mutations;
same-user hostile path/IPC replacement is not a claimed security boundary.

The Inno adapter can use `CreateCallback` with the documented native timer
signature to poll short control replies without blocking the wizard's UI thread:
https://jrsoftware.org/ishelp/topic_isxfunc_createcallback.htm . This is a planned
UI integration mechanism, not evidence that wizard navigation/cancellation or
helper shutdown has been accepted. No DLL worker is loaded into setup.

The actual signed revision-1 candidate catalog and packages passed the isolated
selection scenario on Linux and Windows: one RU selection causes one local
artifact fetch, writes the catalog receipt but installs nothing before review,
commits only RU after confirmation, leaves input English-only, and reuses exact
verified cache bytes without fetching again. Receipts are
`target/ui-package-candidates-20260912-01/catalog-selection-linux.json` and
`catalog-selection-windows.json`. These are offline transaction checks, not
GitHub transport, installer UI or physical typing acceptance. Both temporary
stores were removed by their owning test process; live profiles were untouched.
## Guarded English UI probe (2026-09-12)

`InstallerUiProbe` gives the probe a distinct AppId and visible installation-
disabled title. It overrides the helper store path to the setup temporary
directory and returns from `PrepareToInstall` before closure helpers, profile
initialization or file replacement. This is not the distributable installer.

The guarded binary is recorded in
`target/installer-ui-probe-20260912-01/build.json`. Dependency notices in that
probe are deliberately incomplete; they remain a release blocker.

Native per-process UI messages verified English navigation, local catalog
selection, all 13 rows, selecting RU, the exact 27671-byte total, download
confirmation followed by probe refusal, absent real store and normal close.
The helper received only read-only catalog/poll/selection commands before
closing. Successful receipts and own-window screenshots are under
`target/installer-ui-probe-20260912-01/catalog-02` and `catalog-03`.
`navigation-01` and `catalog-01` were failed harness attempts (Next caption and
dialog-button targeting), not passing acceptance. Only their fixture processes
were stopped; later runs closed normally. The screenshot capture was adjusted
to repaint the owned window before capture.

This does not establish production network/installation, the actual license
review page, every language/RTL/fonts, crashes, upgrade/repair/uninstall, signing
of the installer executable, dependency-notice completeness or physical typing.

## Local catalog artifacts

`Command::CheckCatalog` may carry a `local_file`. The session then uses a local
catalog source: selected artifacts are read from the catalog's directory by the
validated asset name and pass the same pinned hash/length/signature checks as a
network download. The network fetch closure is never invoked for a local source,
so copying `.aklp` files next to the `.aklc` supports an explicit offline install
without any published release. Failure replies carry a coarse, path-free reason
code (for example `download_http`, `local_read`, `verification`) that the Inno
page appends to its generic failure caption.

