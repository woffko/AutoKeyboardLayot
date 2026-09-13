# Local language-package store (development)

`PackageStore` is an explicit filesystem backend, not a downloader or package UI.
The caller must supply an absolute dedicated path below a protected per-user
directory. Startup/settings and asynchronous agent reload now project this data
through `InstalledPackages` into immutable dictionary and catalog snapshots.

## Files and operations

The flat store contains `writer.lock`, `CURRENT`, immutable
`state-<64 lowercase SHA-256 hex>.json` and `blob-<hash>.aklp` files. CURRENT is
exactly 64 lowercase hex bytes naming one state hash, never an arbitrary path.
Package names, components and archive paths cannot select filesystem paths.

- `initialize` only creates an absent directory under an existing protected
  parent. It refuses an existing target and does not recursively create parents.
  Interrupted initialization is not silently repaired. On Unix it requests mode
  0700 for the directory and 0600 for new files; Windows inherits parent ACLs.
- `open` is non-mutating. `load` requires CURRENT and every referenced state/blob;
  no missing/corrupt file means an empty inventory. It bounds reads, checks the
  opened file type, rejects final-component links/reparse points, verifies exact
  state/blob hashes, preflights all state metadata, verifies every package under
  current trust and reconstructs the exact inventory. It never scans for an
  alternative version or promotes an orphan.
- `commit` acquires the existing writer-lock file once and holds it through the
  complete transaction. Rust `File::try_lock` requires at least Rust 1.89. Busy
  and filesystem/locking errors are distinct; locks are released by handle drop.
  The current state is reloaded/authenticated under that lock, compared to the
  expected snapshot hash, then the candidate's generation, history and source
  are checked. A stale or unrelated candidate cannot erase recorded history.
- Every candidate artifact is reauthenticated before publication; new raw bytes
  must match a candidate receipt. Missing, extra, duplicated, changed or untrusted
  bytes fail. Existing exact cached bytes can be reused. Artifacts are processed
  sequentially, but compiled inventories coexist: the 512 MiB artifact limit is
  not a bound on peak memory. No-op commits create no files.

## Publication and recovery boundary

`PreparedStoreInitialization` prepares a selected batch for migration before
creating a store. It checks required input IDs, caps file count and combined raw
bytes, and retains authenticated bytes. Confirmation can create an absent store,
use a complete empty one, or reuse exactly the same candidate after a failed
configuration save. It refuses unrelated history and partial initialization.
It never switches configuration mode. The migration caller must save that mode
last, using the configuration writer's exact legacy-mode backup and concurrency
check. The settings migration dialog now implements this order, with Windows
compilation/native validation of that dialog still pending. After confirmation,
it may create the single application-directory child before initializing its store;
it does not recursively create a missing per-user root.

`with_current_snapshot` reauthenticates the expected inventory while holding the
store writer lock through the caller's final configuration save. Stale state
rejects the callback. The lock order is store first, configuration second; callers
must not hold the config lock before entering or reenter store operations inside
the callback. This excludes cooperating package writers from the final mode
activation window, not an owner replacing ancestor directories.

`PreparedImport::from_file` provides read-only preparation for an explicit import
confirmation. It bounds the input, rejects final-component links/non-files,
authenticates the envelope and stages against the loaded inventory. Its private
buffer retains exactly the previewed bytes; replacing/deleting the source path
cannot change what confirmation installs. Dropping preparation cancels without
writes. `confirm` consumes preparation and rechecks current trust and the original
inventory through the normal store transaction. A stale preview needs a fresh
preparation and confirmation. This API does not activate managed mode or enable
input packs. The managed-mode settings import UI now calls it from a background
worker and refreshes settings snapshots after success. Windows compilation
passes; native interaction validation is pending. Legacy migration remains a
separate operation.

New files use bounded-attempt, create-only temporary names and `sync_all`.
Immutable objects are published using same-directory hard links, which cannot
overwrite an existing destination. Existing objects must match byte for byte.
Filesystems without hard-link support fail closed. The new state and its blobs
are published before CURRENT is replaced. Unix syncs the store directory before
and after rename. Windows now uses `SetFileInformationByHandle(FileRenameInfoEx)`
with replace-existing and POSIX semantics in the same directory. This preserves
already-open target readers while new opens see the new pointer. Source access
requires DELETE/read-attributes, refuses reparses/non-disk files, and does not
grant in-place write sharing. There is no copy/delete, ACL-ignore, readonly-ignore
or legacy rename fallback. See the
[Microsoft rename flag contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information).
Unsupported filesystems/API behavior fail closed. New file contents are flushed
before submission; this API has no write-through flag and successful return
does not establish namespace durability after power loss.

Errors before submitting pointer replacement preserve the old active state and
may leave safe orphan objects. A Windows rename-call error is conservatively
reported as uncertain rather than automatically retried. After pointer publication, a failed final sync/checkpoint
returns `CommitUncertain`: reload before deciding whether to retry. A successful
reload selects exactly one complete state. Ordinary cleanup removes only the
specific temporary created by this operation. Process termination may leave a
temporary, which future writers skip. There is no automatic garbage collection;
old states and artifacts are retained for a future explicit recovery operation.
Disk quota/garbage-collection UX remains to be implemented.

Revision high-water marks and catalog provenance survive removal. An older
package cannot be reinstalled merely by removing the new version. Explicit user
rollback to an older version needs a separate policy and is not implemented by
rewinding CURRENT; doing that would also rewind downgrade protection.

## Trust and acceptance limitations

The parent/root must be protected against unauthorized modification and directory
replacement. Final-component no-follow checks are not a defense against an owner
swapping ancestors concurrently. File locks coordinate cooperating writers;
they do not authenticate historical local metadata. An attacker who can replace
CURRENT and an old valid state can rewind local history. Current trust changes
can render an installed store unreadable; it is not automatically emptied to
make an update/removal succeed. An explicit recovery UI is still needed.

Tests use synthetic signed packages and isolated temporary directories, including
spaces/non-ASCII paths. They check lifecycle, cache reuse, stale writers, missing
history, same-process and child-process lock contention, replacement while an
old pointer reader remains open, commit-boundary failure injection, corruption, trust changes
and Unix link/FIFO/directory rejection. These simulate returned errors at logical
boundaries, not real process termination, torn writes or power cuts. Native
Windows filesystem/ACL/sharing tests and interruption acceptance remain required.
The manager, trusted release anchors, downloads and
installer integration remain separate unfinished work. No production store or
user installation is created by these development tests.

## Runtime integration and migration boundary

Startup and settings use the persisted schema-4 `[packages]` mode. Legacy mode
retains the transitional EN/RU/ET bootstrap and loose UI catalogs without
inspecting or activating any package directory. Managed mode requires
`%LOCALAPPDATA%/AutoKeyboardLayot/packages` to load successfully; it never creates
or repairs that directory. Missing managed storage is an error, including on a
fresh process start with the saved configuration. Managed mode uses a
separately initialized English base plus exactly the authenticated installed
packages; it never fills missing packages from bootstrap or loose catalogs.
Selected missing/disabled IDs and explicit profiles remain configuration data.

The settings process loads an immutable dictionary/catalog snapshot. Initial
rows, toggle/profile callbacks and language refresh share that registry, which
is replaced on the UI thread after an explicit successful import, without
filesystem work inside those callbacks. The input worker receives the fully
prepared dictionary Arc with the corresponding configuration/profile snapshot.
Managed UI catalogs are applied on the main thread only after the matching
configuration is acknowledged by the input worker.

Agent reload reads/verifies/compiles packages on one dedicated loader thread, not
the keyboard-hook/window-message thread. Its request and reply channels each
have capacity one. Requests while a read is outstanding coalesce into one latest
read; superseded results are discarded. Timer polling never waits. A hung read
retains the last working configuration without spawning replacement threads or
blocking shutdown; cancellation/progress UX remains future manager work.

Before input-worker enqueue, AppState checks package-source continuity against
the latest pending snapshot, or the active snapshot when none is pending. It
rejects managed-to-bootstrap fallback, older generations and same-generation
state forks. Ordered accepted snapshots keep the existing worker acknowledgement
protocol; there is no late worker rejection that could leave a handoff pending.
The source and catalog snapshot become active at the same acknowledged boundary.

Production trust is deliberately empty until release public keys are provisioned;
an empty managed store works, but optional packages cannot yet load in the actual
application. Synthetic fixture keys exist only in tests. Directory presence does
not select the mode: schema 4 requires exactly one `mode=legacy` or `mode=managed`.
Schemas 1–3 and standalone legacy files retain legacy mode when read/migrated;
reads never rewrite them. Saving a prior schema makes an exact non-overwriting
`config.schema-1.bak`, `config.schema-2.bak` or `config.schema-3.bak` before replacing
configuration. Old schema-3 readers reject schema 4 rather than interpreting
managed data as bootstrap configuration.

The future installer must use `ConfigurationDocument::english_only_managed()`
for a genuinely fresh profile, prepare and verify its empty store, and then save
the explicit configuration without enabling startup conversion. Existing-user
migration must preserve the loaded document and chosen languages instead of
substituting this fresh-profile constructor. A prepared migration directory alone
does not activate packages while the old configuration remains in legacy mode.
The manager's full migration/rollback transaction is still to be implemented.

Persisted intent assumes the configuration remains present and trusted. If the
configuration itself is deleted, the legacy-file loader still reconstructs legacy
configuration; this work does not claim recovery of a deleted profile or protection
against deliberate local configuration replacement. The binary also still includes
RU/ET bootstrap data until final packaging removes it.
