# Package inventory and candidate transactions (development)

`PackageInventory` owns immutable authenticated package objects and their exact
artifact receipts. It prepares candidate states; it does not write files, commit
an installation, notify the worker or obtain user approval. The Windows
worker/settings now receive the [installed-data projection](package-store.md)
from managed storage when present, with an explicit transitional bootstrap mode.

## Transaction rules

- An explicit local import receives already signature-verified packages. A
  catalog-based candidate must match the entire chosen `DownloadPlan`, including
  artifact ID/revision/length/hash and advertised components. Missing, extra,
  duplicate or substituted objects fail the candidate.
- Plan freshness is rechecked even for an empty selection. Its repository and
  checkpoint remain attached to the candidate. An earlier catalog revision or a
  changed body at an equal revision is rejected against the current inventory.
- All stages clone the prior snapshot. A failure after considering part of a
  batch cannot change the prior inventory. A real change increments its generation;
  an identical retry does not. Generation overflow fails instead of wrapping.
- English base identity is protected from replacement/removal. There are at most
  63 optional packages, leaving one input-registry slot for the base. Active
  artifact bytes total at most 512 MiB; this is not a peak-memory guarantee.
- Per-package high-water receipts survive removal. Ordinary imports/plans reject lower revisions and a different
  exact artifact at the same revision are rejected, including after remove/reinstall.
  Transport representation is intentionally part of artifact identity: repackaging
  the same semantic content requires a new revision unless raw bytes are identical.
- A separate `stage_rollback` accepts only the exact immediately preceding
  installed artifact retained by an update. It creates a fresh generation without
  lowering high-water or catalog checkpoints. Removal clears rollback eligibility,
  not the high-water tombstone. Repeated rollback is ineligible; returning to the
  exact high-water artifact remains an ordinary explicit update.
- UI canonical locales and aliases have one installed owner; conflicting data
  rejects the whole candidate. Shared keyboard profiles are not installation
  conflicts: detector selection/eligibility handles those independently.
- User dictionaries, exclusions and enabled-ID preferences are not stored here and
  cannot be deleted by these operations. Dictionary snapshots take the caller's
  immutable base and explicit selected IDs, preserving missing selections without
  enabling newly installed data implicitly.

The inventory repository identifies its catalog checkpoint, not the provenance of
every artifact. Explicitly approved local imports can coexist with that metadata;
they still need a trusted signature and must pass package revision checks. A future
UI must label import provenance accurately rather than treating the catalog's
repository as an assertion about every installed package.

## State representation and restoration

State format 1 is bounded JSON (256 KiB): `format`, `generation`, optional paired
`repository` and `catalog`, `installed`, and `highest_seen`. Each package receipt
contains `id`, positive `revision`, `bytes`, and `sha256`; the optional catalog
checkpoint contains its positive `revision` and exact document `sha256`.
Inventories with recorded rollback targets use format 2 and an additional bounded
`rollback_targets` receipt list (at most 63). A lower installed revision must
exactly equal its recorded target; targets must be older than the high-water mark
and belong to currently installed IDs. States without targets retain format 1
and its previous byte representation. Old binaries cannot read format 2; do not
rewind CURRENT to make them work. No scan reconstructs targets for older format-1
stores: eligibility starts when this implementation observes an actual update.
At most 1,024 historical package IDs are retained. Reaching this limit fails an
addition instead of silently discarding downgrade protection.

`required_artifacts` validates the entire structure, generation, source/checkpoint
pair, all historical receipts, unique active IDs, active/high-water agreement and
aggregate bytes before returning a blob list. A loader must call it before reading
artifacts, use bounded no-follow file reads, authenticate each package under the
current trust policy, then call `restore`. Restoration requires exactly the listed
artifacts and rechecks receipt identity and cross-package UI ownership. Missing or
corrupt data fails closed; it is not an empty-installation/default fallback.

This is a serialization/restoration contract. The separate
[local store](package-store.md) now implements writer locking, expected-state
checks, no-overwrite immutable blob publication and pointer replacement.
The store-side successor guard requires an identical state or exactly one new
generation, preserving all prior high-water marks and catalog provenance.
High-water state assumes protected local storage; the codec cannot defend against
an attacker who can replace that trusted state.
The successor guard independently derives target changes from current installed
receipts, so restoring a structurally valid document does not authorize invented
history. `PreparedRollback::from_store` reads only the exact recorded cached blob,
checks length/hash/current trust, and stages the rollback without writes. Confirm
uses the store's ordinary exact-state CAS and reauthentication. Missing/corrupt
history fails closed; no network request, filename scan or pointer rewind occurs.
The managed settings UI now provides a separate previous-version review button,
enabled only for a recorded eligible target. This is logical eligibility, not a
promise that the cache is intact: inspection reauthenticates on a worker. Failure
explains missing/corrupt/untrusted/incompatible data or changed inventory and
disables another inspection until an explicit refresh. No full-cache scan is
performed for the list. A confirmed preview shows from/to revisions, a downgrade
warning and complete target metadata/license/notice; cancellation drops the
in-memory candidate. Confirmation shares ordinary CAS and uncertain-write handling
and does not change user choices. Native Windows interaction remains unaccepted.

An abstract independent review highlighted intentional intermediate-version
lockout: after 1 -> 3 -> 1, ordinary installation of 2 is still rejected. The
explicit warning and core test cover this; rollback is not a general bypass of
anti-downgrade policy. Format-2 compatibility remains a binary-downgrade caveat.
Packages have no executable scripts or cross-package dependency manifest; current
runtime API and locale ownership checks still run for the whole candidate.
If garbage collection is introduced later, live rollback-target blobs must be
pinned alongside installed blobs. No automatic cache collection exists today.
