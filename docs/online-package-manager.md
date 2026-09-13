# Selected online package workflow (development)

The managed-mode settings UI now connects catalog checking, explicit selection,
download, per-package review and installation. It does not migrate an existing
legacy installation online; that installation still uses the separate explicit
local-file migration flow. Installer-assisted online migration remains work.

## Three approval boundaries

1. **Check catalog.** The operator supplies an independent `owner/name` source,
   or leaves it blank to reuse the source already bound in the store, falling
   back to the user-approved `woffko/AutoKeyboardLayot` for an unbound store. It is never
   taken from untrusted metadata. A different bound repository is refused.
   Checking fetches at most 1 MiB from the publication convention
   `https://github.com/{repository}/releases/latest/download/catalog.aklc` and
   verifies its signature, repository, freshness and persisted checkpoint.
   It writes no receipt or package. Selecting the repository does not provision
   a signing key or prove that a catalog/package asset has been published.
2. **Download selected.** All checkboxes start clear. The UI shows selected count,
   total artifact bytes, revisions, components and API compatibility. The core
   also checks package capacity, the protected base and known revision history
   before retrieval. The explicit click accepts the catalog: its anti-rollback
   checkpoint is committed before any package download. This receipt remains if
   a download fails or is cancelled, but no package blobs are written or installed
   by download. Only absent selected artifacts reach the network; exact cache
   objects are hash/size checked and reauthenticated. Corrupt cache objects fail
   closed instead of being silently overwritten.
3. **Install selected.** Authenticated artifacts remain in memory for a separate
   per-package metadata/license review. Confirmation reauthenticates the retained
   catalog, checks current time and the whole plan, and commits all selected
   artifacts against the exact accepted store snapshot. No subset is installed
   on error. Input participation, UI preference, lexicons and exclusions remain
   separate configuration data and are not enabled or erased by installation.

The mutable `latest` pointer is only for authenticated catalog metadata; pinned
package URLs still use immutable explicit release tags. Merely viewing newer
metadata is not accepting it. A user-approved catalog receipt is retained even
if no packages are ultimately installed, preventing a later download from
silently forgetting that accepted catalog generation.

Because application and language assets now share one repository, the release
chosen as GitHub's `latest` must include `catalog.aklc`. Package-only releases
must not accidentally replace that pointer without also carrying the catalog.
If the asset is absent, checking reports an error; it never downloads the source
archive or guesses a different unverified catalog. This is a publication contract,
not a claim that the remote assets already exist.

## Background work and error handling

Network work shares the manager's single-worker busy guard. A separate cancel
action sets a monotonic per-operation flag; cancelling never closes a synchronous
WinHTTP handle from another thread. A queued successful result is discarded if
the user cancelled before the UI consumed it. Progress uses one shared bounded
snapshot, updated per verified artifact, including cache hits, not an unbounded
message queue. The selected operation checks a 600-second elapsed budget between
stages; individual transport timeouts remain cooperative rather than a hard
wall-clock guarantee.

Errors/cancellation discard the temporary catalog view; an explicit recheck is
required before retrying. Source/selection controls cannot change underneath a
pending review. Online labels and a pending online review are refreshed when the
user changes interface language. Ordinary settings close/save remains guarded
while work is active.

If an import, removal or online install reports `CommitUncertain`, managed mode
requests an agent reload and rereads the authenticated store without repeating
the write. A successful reread updates the visible package state with an explicit
durability warning, not an ordinary success claim. Failure to reread remains an
error. Migration never activates configuration after an uncertain store commit.

## Verification limits

Portable fixture tests cover explicit-only fetching, no package installation
before confirmation, receipt retention on cancellation, exact cache reuse,
revoked trust, stale confirmation and refusal of a substituted verified artifact.
All 14 UI catalogs have the new messages (176 English keys). Windows compilation
and strict Clippy passed in job `0c416f3ce34a41e2b18468a055220434`.
Actual UI/network/proxy/filesystem acceptance of this integration remains pending.
No real package was downloaded, published or installed during those fixture tests.
The application now explicitly uses `PackageTrust::release()` with the generated
`pkg-20260912-01` public key embedded at build time. Empty default trust is retained
for explicit untrusted/test contexts. Provisioning is not live publication or
end-to-end acceptance; the installer and real signed-asset workflow remain open.
