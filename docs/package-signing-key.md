# Package signing key

The user authorized creation of the separate Ed25519 signing key
`pkg-20260912-01`. Public metadata is in
`data/package-signing/public-key.json`; it is not a private key.

The encrypted PKCS8 key lives outside the repository and application data path:

`%LOCALAPPDATA%\AutoKeyboardLayot-Signing\pkg-20260912-01\private.pkcs8.dpapi`

The sibling `public.json` records the algorithm, signer, public key, fingerprint,
and entropy domain. Keep both files when backing up the encrypted key.
DPAPI uses CurrentUser scope and entropy formed from the domain, a NUL byte,
signer ID, a NUL byte, and public-key hex, encoded as UTF-8. The key directory has
a protected ACL granting access only to the creating user and SYSTEM.

## Verified creation

`tools/create_package_signing_key.py` uses the absolute root-owned OpenSSL binary
with user OpenSSL configuration and loader environment overrides excluded.
Plain PKCS8 is passed only through captured anonymous pipes/process memory.
PowerShell never receives secret material as a cmdlet parameter or string;
the C# helper reads binary stdin and writes only DPAPI ciphertext to disk.
Files/directories are created exclusively, with restrictive directory ACLs set
at creation. Existing operations are never overwritten.

The creation passed an Ed25519 sign/verify self-test, a flushed ciphertext
read-back comparison, and a second-process DPAPI decrypt/hash comparison.
The nonsecret receipt is `target/package-signing-key-20260912-01/creation.json`.
Creation did not change application trust anchors or publish any assets.

## Application trust provisioning

After creation, `PackageTrust::release()` was added as the explicit application
policy. It embeds only the public metadata at compile time, validates its
repository/algorithm/fingerprints and delegates key validation to Ed25519-Dalek
(see [Trust metadata formats](#trust-metadata-formats)).
Runtime store loading and the settings manager use this policy for downloads,
imports and rollback. They never discover keys in packages or adjacent files.
`PackageTrust::default()` remains empty for fixtures and explicitly untrusted
contexts. Fresh empty-store initialization also keeps that empty trust boundary.
This source change does not publish a catalog or establish end-to-end download
acceptance; rebuilt binaries and signed package artifacts require separate checks.

## Trust metadata formats

`data/package-signing/public-key.json` is the only place the application learns
which keys it trusts. It is compiled in; a package, a catalog or a file in the
user's profile can never add a key. Two formats are accepted.

Format 1 lists one key, which is the release key (this is the published file):

```json
{ "format": 1, "algorithm": "Ed25519", "signer": "pkg-20260912-01",
  "public_key_hex": "...", "fingerprint_sha256": "...",
  "repository": "woffko/AutoKeyboardLayot" }
```

Format 2 lists one to eight keys, each with a role:

```json
{ "format": 2, "algorithm": "Ed25519", "repository": "woffko/AutoKeyboardLayot",
  "keys": [
    { "signer": "pkg-20260912-01", "public_key_hex": "...", "fingerprint_sha256": "...", "role": "release" },
    { "signer": "rec-20261001-01", "public_key_hex": "...", "fingerprint_sha256": "...", "role": "recovery" } ] }
```

The parser refuses the file unless all of this holds: the file is at most 4096
bytes; the format is 1 or 2 and no field is unknown; the algorithm is `Ed25519`
and the repository is `woffko/AutoKeyboardLayot`; every `fingerprint_sha256` is
the SHA-256 of that entry's 32 raw public-key bytes; signer names are 1 to 32
characters of lowercase letters, digits and dashes; no signer name and no key
appears twice; no key is a weak (small-order) point; the role of every key is
`release` or `recovery`; and exactly one key has the role `release`.

Every listed key verifies signatures. Only the release key is used for signing:
`PackageTrust::release_signer()` returns it to the signing tool and to the two
preparation examples, and a recovery key is never chosen. Existing signatures
stay valid when a file in format 1 becomes a file in format 2, because the
release key keeps its signer name. Hex may be written in either case, and the
same key in two cases under two names is still a duplicate; the signing tool
always uses the lowercase form, which is what the key tool records, what the
published file contains and what the key-protection entropy is built from. The
tests are in `src/language_package.rs` (`release_metadata_*`,
`hex_may_be_written_*`, `swapping_the_roles_*`, `trust_registry_*`) and
`tools/test_signing_key_metadata.py`.

## Recovery key

With one key, losing the release key (see the DPAPI limits below) means that
nothing new can be signed in a way that installed applications accept, because
the trust list lives in the application. A recovery key is a second key that is
generated offline and shipped in the trust list **before** it is needed. It is
the owner's decision and ceremony: this repository does not contain a recovery
key, and no script or agent generates, stores or commits one.

Ceremony:

1. Use a Windows machine with WSL that stays offline from now on (the creation
   tool needs WSL, OpenSSL and PowerShell, and DPAPI ties the key to that
   Windows profile). Create the key with the same verified tool as the release
   key, under a new signer name, for example `rec-YYYYMMDD-01`:
   `python3 tools/create_package_signing_key.py --operation rec-YYYYMMDD-01 --receipt-directory NEW_EMPTY_DIRECTORY`.
   Run it with `--probe-only` first. It never overwrites anything.
2. Take only the public receipt out of the machine: the signer name, the public
   key and the fingerprint. The encrypted private key stays where it was made.
3. Verify the fingerprint twice, in two independent ways, before it goes into the
   repository. The fingerprint is the SHA-256 of the raw key bytes, not of the
   hex text: `printf %s PUBLIC_KEY_HEX | xxd -r -p | sha256sum` must print the
   value in the receipt. Compare it again from a second copy of the receipt.
4. Turn `public-key.json` into format 2 with the existing key as `release` and the
   new key as `recovery`, keeping the existing key's fields byte for byte. Run
   `cargo test --locked` and `python3 -m unittest tools.test_signing_key_metadata`,
   and have a second person review the diff. Only applications built from that
   commit trust the recovery key, so release them well before they are needed.
5. Rehearse once while the release key still works. On the offline machine build
   the signing tool from a local, uncommitted copy of `public-key.json` in which
   the two roles are swapped, sign a throw-away catalog candidate into a scratch
   directory, check it with `verify_release_catalog` built from the real file,
   and delete the scratch files. A swap changes which key signs, not who is
   trusted, so nothing has to be republished (`swapping_the_roles_changes_the_signing_key_and_not_who_is_trusted`).

Fingerprints are also how people check a build: the application's trust list can
be compared with the fingerprints published in the release notes and in this
file's history. Never accept a key because it arrived with a package.

Storage advice. DPAPI protects the key for one Windows profile; a copy of the
encrypted file alone cannot be opened elsewhere. Keep the offline machine, or a
verified encrypted image of its disk, in two separate places, and write down
who may open them. If you prefer another protection (a hardware token, or a
PKCS #8 file under a long passphrase), the application only needs the public
half; the signing tool in this repository only loads DPAPI-protected keys.
Never keep the recovery key on the release machine or in a backup that the
release machine can reach.

Compromise response. A stolen release key stays trusted by every installed
application until that application is updated, and an attacker with the key can
also issue a catalog with a higher revision, so the revision rule does not stop
them. If the release key may be stolen: stop signing and publishing; create a
new release key and, if needed, sign interim catalogs with the recovery key as
described above; ship an application release whose trust list no longer
contains the stolen key and tell users to update; and write down the dates of
every catalog and package the old key signed. If the recovery key may be stolen,
treat it the same way and create a new one. A key that is lost but not stolen
needs no announcement; replace it at the next release.

## Limits and recovery

This is encryption at rest, not protection from malware executing as the same
user. Managed buffers are cleared where possible and Linux core dumps are
disabled; complete erasure of all runtime/pipe copies, OS paging, hibernation
and Windows crash dumps is not guaranteed.

Decryption depends on the original Windows DPAPI credentials/profile. A copy of
the encrypted file alone is not a guaranteed portable backup. Profile loss or
an administrative password reset can prevent recovery. Do not rely on this as the
sole long-term publication key: see [Recovery key](#recovery-key) for the second
key that the trust metadata can carry. See
[Microsoft's DPAPI documentation](https://learn.microsoft.com/en-us/dotnet/api/system.security.cryptography.protecteddata).

The operation receipt is created before generation and atomically updated. If
creation is interrupted, inspect that receipt and the exact key directory only
after confirming all prior processes have ended. If ciphertext exists, preserve
it and recover that operation; do not mint a replacement automatically. If only
an intent receipt exists, verify ciphertext absence before authorizing a new
operation ID. No code automatically deletes or resets an uncertain operation.

Never print, paste, commit, or pass plaintext private material through argv,
environment variables or logs. Do not reuse personal SSH/authentication keys.
