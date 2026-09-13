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
repository/algorithm/fingerprint and delegates key validation to Ed25519-Dalek.
Runtime store loading and the settings manager use this policy for downloads,
imports and rollback. They never discover keys in packages or adjacent files.
`PackageTrust::default()` remains empty for fixtures and explicitly untrusted
contexts. Fresh empty-store initialization also keeps that empty trust boundary.
This source change does not publish a catalog or establish end-to-end download
acceptance; rebuilt binaries and signed package artifacts require separate checks.

## Limits and recovery

This is encryption at rest, not protection from malware executing as the same
user. Managed buffers are cleared where possible and Linux core dumps are
disabled; complete erasure of all runtime/pipe copies, OS paging, hibernation
and Windows crash dumps is not guaranteed.

Decryption depends on the original Windows DPAPI credentials/profile. A copy of
the encrypted file alone is not a guaranteed portable backup. Profile loss or
an administrative password reset can prevent recovery. Arrange a recovery/export
procedure before relying on this as the sole long-term publication key. See
[Microsoft's DPAPI documentation](https://learn.microsoft.com/en-us/dotnet/api/system.security.cryptography.protecteddata).

The operation receipt is created before generation and atomically updated. If
creation is interrupted, inspect that receipt and the exact key directory only
after confirming all prior processes have ended. If ciphertext exists, preserve
it and recover that operation; do not mint a replacement automatically. If only
an intent receipt exists, verify ciphertext absence before authorizing a new
operation ID. No code automatically deletes or resets an uncertain operation.

Never print, paste, commit, or pass plaintext private material through argv,
environment variables or logs. Do not reuse personal SSH/authentication keys.
