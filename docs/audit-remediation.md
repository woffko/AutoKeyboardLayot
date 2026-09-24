# Input integrity and release-readiness remediation

## Contracts

- Retained manual cycles and space boundaries are tied to input epochs and
  profile generations. External injection, configuration changes, privacy
  blocking and lost input revoke them. An unchanged profile poll does not.
- A planned replacement is not a completed cycle. Preflight failure preserves
  the source when its context remains valid. Once an edit can submit input,
  retained state is revoked; only successful completion publishes the next
  cycle. Physical replay is still labelled `input-submitted-text-unverified`.
- A second space, punctuation or another non-modifier key invalidates retained
  adjacency. Failed edits cannot restore a pre-edit boundary on return.
- UI projection preserves the selected set exactly. Refresh never enables an
  installed but deselected language. Package installation and enabling remain
  separate actions followed by Apply.
- The manual terminal exception is explicit and default off. Only field-level
  UIA unavailability qualifies, with independently verified process identity,
  integrity and context. Transport and process errors fail closed.
  This exception preserves bounded volatile word/cycle state for Pause/Break,
  including words without a trailing space. Words observed under the exception
  remain manual-only through their boundary even if UIA recovers in the meantime.
  It never authorizes automatic conversion or recovery of input lost during a
  hard privacy failure.
- Post-write failure and worker loss report an uncertain outcome, never
  "nothing changed" or "not started". Failed writes are inspected once where
  possible, without repeating confirmation or activating a failed migration.
- English catalog IDs and placeholder sets remain compatibility contracts.
  New packages declare API 2, old packages remain readable, and catalog pins
  must match a package's authenticated API requirement.

## Repeatable checks

```sh
cargo fmt --all -- --check
cargo test --locked
cargo test --locked --no-default-features
cargo clippy --locked --no-default-features --features installer-tools --all-targets -- -D warnings
cargo run --locked --example validate_locales -- data/package-locales --require-complete
cargo audit
```

On Windows run the binary tests as well: they cover input-queue resets and
settings-row projection, which Linux library tests cannot execute. Run tests
serially when they share native process/window facilities.

The CI matrix runs Linux and Windows. A separate scheduled job verifies the
published catalog's signature and requires at least 72 hours remaining. It does
not sign or publish a renewal. An operator must review and publish one before
expiry. Frozen-release package verification is separate from live-catalog
expiry checks so historical fixtures remain useful after expiry.

`.gitattributes` fixes text checkouts to LF and preserves the exact bytes of
hash-pinned vendored catalogs and notices. CI enables Python UTF-8 mode on both
platforms. Integrity tests compare original bytes, not normalized substitutes.

## Installer builder

The reviewed builder is portable between Windows and WSL. Supply all reviewed
inputs explicitly; machine-local paths and notice hashes are not embedded:

```sh
python tools/build_experimental_installer.py --output target/installer-candidate \
  --notice-directory PATH --build-receipt PATH --notice-sha256 REVIEWED_SHA256 \
  --iscc PATH_TO_ISCC
```

Windows uses native Cargo; WSL uses cargo-xwin and wslpath. Existing notice,
dependency-set and fresh-output checks remain mandatory. The remaining local
VM scripts are not required for this portable builder and are not automatically
enrolled or published.

## Deployment acceptance

Before replacing an installed binary, verify all currently installed signed
packages using the new verifier. Then check startup in an isolated copy of the
profile. Keep the real profile unchanged during this check.

Physical typing checks still require an interactive user: repeated Pause,
external text insertion followed by Pause, cancellation during replay, undo,
the terminal opt-in policy and checkbox Apply/reopen. Passing native unit tests
or observing SendInput success alone does not establish these checks.
