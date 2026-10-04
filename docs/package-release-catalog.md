# Authenticated release catalog and selective planning (development)

`package_catalog` is a pure verifier/planner. It does not fetch a catalog, perform
HTTP requests, install packages, persist receipts or change settings. The separate
language-pack project's existing `pack-index.json` is a development source list,
not this signed release format. The selected source is the main
`woffko/AutoKeyboardLayot` repository; release trust now embeds the public key
`pkg-20260912-01`. A local revision-1 candidate now exists at
`target/ui-package-candidates-20260912-01/catalog.aklc`; its signature and all
13 referenced local artifacts passed offline verification. No signed catalog
has yet been published, and its planned GitHub URLs were not contacted.

## Signed envelope and source policy

The catalog envelope is UTF-8 JSON, limited to 1 MiB, with exactly `format` (1),
`signer`, `catalog` (a JSON string), and `signature` (hex Ed25519 signature).
The signed bytes are `AutoKeyboardLayot.release-catalog.v1`, NUL, and the exact
decoded `catalog` string bytes. This domain differs from language-package
signatures. The shared strict-verification helper preserves both domains.

The inner document is limited to 256 KiB and has exactly these fields:

- `format`: 1.
- `repository`: `owner/repository`, compared against independent application
  policy after ASCII case normalization.
- `revision`: positive unsigned 64-bit catalog generation.
- `issued_at`, `expires_at`: Unix seconds; `issued_at <= now < expires_at` and
  a maximum validity interval of 31 days.
- `packages`: at most 64 package records.

Each record contains `package_id`, positive package `revision`, `bytes` (1 through
64 MiB), `sha256` (exact raw artifact digest), `tag`, `asset`, `runtime_api`,
`input` (boolean) and optional `ui_locale`. Input/UI components remain independent,
but at least one must be present. English UI is reserved. Unsupported runtime
APIs may be displayed but cannot be selected for download.

Tags and asset names are bounded ASCII path segments; separators, escapes,
queries, fragments, absolute URLs and dot-leading segments are rejected. The
mutable `latest` tag is rejected, and assets must end in `.aklp`. URLs are built
only as `https://github.com/{repository}/releases/download/{tag}/{asset}`. Package
IDs and case-folded tag/asset pairs must be unique. No dependencies are inferred;
format 1 packages are self-contained. Unrecognized dependency metadata fails
validation instead of triggering undisclosed downloads.

## Freshness and receipts

A `CatalogCheckpoint` contains the previously authenticated catalog revision and
SHA-256 of its exact signed document. Lower revisions fail; an equal revision
with a different document hash also fails. Re-reading the identical generation
is allowed. Renewing expiry or otherwise changing metadata requires a new
generation. The manager must persist and supply this checkpoint under its writer
lock. Passing no prior checkpoint establishes no historical rollback protection.
The current module does not implement durable storage or protect the system
clock; the caller supplies trusted time. Installed-package downgrade protection
remains separate from catalog-generation protection.

## Renewing the catalog

A published catalog must be renewed before it expires. Clients refuse a catalog outside
`issued_at <= now < expires_at`, and online language installation stops until a valid catalog is
published again. The scheduled workflow `catalog-freshness.yml` fails, and opens or updates one
issue, when fewer than 10 days (240 hours) of validity remain.

A renewal is a new catalog generation: the same package records, **a new, higher `revision`**,
and a new window. A client that accepted revision N refuses a lower revision, and also revision N
again with a different document.

[`catalog-renewal.md`](catalog-renewal.md) is the runbook with the exact commands, the two steps that
need the owner's approval (signing and replacing the published asset), the checks and the rollback
rules. The summary below is the background.

1. Build the unsigned candidate from the release's `.aklp` files (all in one directory):
   `cargo run --locked --features signing-tools --example prepare_release_catalog -- DIRECTORY TAG REVISION OUTPUT.json [VALIDITY_DAYS]`.
   `VALIDITY_DAYS` defaults to 21 and may be 1 to 31. `issued_at` is placed one hour in the past,
   so a client whose clock runs slightly behind still accepts a fresh catalog, and the window is
   exactly `VALIDITY_DAYS` long, which keeps even 31 days inside the client's lifetime limit.
2. Review the candidate: package IDs, revisions, sha256 values and the tag equal the previous
   catalog, and only `revision`, `issued_at` and `expires_at` differ.
   `python3 tools/check_catalog_candidate.py PREVIOUS_CATALOG CANDIDATE` checks exactly that, plus the
   window length and its freshness, and prints the file's sha256 to record.
3. Sign it with the release key using `tools/sign-package.ps1 -Kind catalog` (key handling is
   described in `package-signing-key.md`).
4. Verify offline: `cargo run --locked --example verify_release_catalog -- CATALOG ARTIFACT_DIRECTORY`
   and `cargo run --locked --example check_catalog_expiry -- CATALOG [MIN_HOURS]` (lead time
   defaults to 240 hours).
5. Publish by replacing the `catalog.aklc` asset of the release that
   `releases/latest/download/catalog.aklc` resolves to, keeping a copy of the previous catalog for
   rollback. Download it anonymously, compare the hash, and run `check_catalog_expiry` on the download.

When a catalog has expired, Settings and the installer show a dedicated message ("The published
package catalog has expired, or the date and time on this computer are wrong. Check the system
clock, or try again later.") instead of a raw error code; a wrong system clock produces the same
message because the window is compared with the local clock.

## Explicit selection and artifact verification

`select` accepts only explicitly chosen IDs and returns an immutable plan with
their pinned records, total byte count, originating repository and catalog
checkpoint, keeping the receipt attached for the manager's commit check.
An empty selection produces an empty
plan. Any missing/incompatible selection, more than 64 choices, or more than
512 MiB selected total fails the entire plan; nothing is silently dropped.
An available unselected package never appears in the returned plan.

`PinnedPackage::verify_download` rechecks metadata freshness and compares raw
length and SHA-256 before package decoding. The authenticated package must then
match the selected ID, revision, input participation and canonical UI locale.
Truncation, modified transport bytes, another valid signed package or inaccurate
component advertising is rejected. Selection plans are not user-approval tokens:
the future UI must obtain confirmation and the manager must use only that plan.
The same verification accepts an exact cached artifact without a fresh download;
byte authentication is independent of transport. Any network request must still
follow the pinned URL and a separately reviewed redirect/source policy.

The new [Windows artifact transport](package-download-transport.md) implements
bounded single-artifact retrieval and a reviewed redirect policy. The new
[online workflow](online-package-manager.md) adds catalog fetch and selected
installation, with Windows/native validation pending. Durable local
inventory, ownership checks, high-water marks, import/update/removal and migration
are covered by the [store](package-store.md). Exact cache lookup is integrated;
cache repair and explicit rollback still require work.
No live download has been performed or verified by these planning tests.

## Local catalog source (no network)

A catalog may be loaded from an explicitly chosen local `.aklc` file. In that
case the authenticated metadata is bound to a local source directory, and every
selected artifact must resolve from that same directory by its validated `asset`
name (a single `.aklp` path segment). A local source never falls back to the
network. All pinned checks still apply: exact byte length, SHA-256, Ed25519
signature, package ID, revision, input participation and UI locale. Symlinked or
non-regular paths, separators, non-`.aklp` names, and oversized or empty files
are rejected before decoding. Online and local flows stay distinct in the UI;
network catalogs keep the pinned GitHub URL.
