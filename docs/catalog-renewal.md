# Renewing the language catalog

The published catalog (`catalog.aklc`) is valid for a window. A client refuses it
outside `issued_at <= now < expires_at`, and online language installation stops
until a valid one is published again. The revision 5 catalog expired on
2026-09-26. This page is the runbook for replacing it. The background is in
[`package-release-catalog.md`](package-release-catalog.md); the key is described
in [`package-signing-key.md`](package-signing-key.md).

**Two steps need the owner's explicit approval at the moment they are done:**
signing with the release key, and replacing a published release asset. Nothing in
this repository, no script and no agent does either on its own. The commands
below are what the owner runs; reading them authorizes nothing.

## What is already prepared

An unsigned candidate for revision 6 was produced and reviewed on 2026-10-03:

| | |
|---|---|
| Candidate | `target/catalog-candidates-20260930-06/catalog-r6.unsigned.json` |
| SHA-256 | `d71cf1fc09e1cb6eb9e5aae7cbc73c48b371bf06784652de00eba7aba362f2ba` |
| Content | the 13 package records of revision 5, unchanged; revision 6 |
| Window | 21 days: `issued_at` 2026-10-03 06:20:10 UTC, `expires_at` 2026-10-24 06:20:10 UTC |

The window starts when the candidate is made, not when it is signed. Every day
between the two shortens what the signed catalog is good for, so **make the
candidate again on the day you sign** (step 1). The checker in step 2 refuses a
candidate whose window started more than three hours earlier.

## Steps

Run steps 1, 2 and 5 on any machine with this repository; steps 3 and 4 in the
Windows profile that owns the signing key. Replace the date in the directory
name with today's. The revision must be one higher than the catalog that is
published now (6 after revision 5); equal or lower revisions are refused by every
client that accepted the earlier one.

```sh
TAG=lang-r5-20260919                # the release that holds the package files
REVISION=6
PACKAGES=target/catalog-candidates-20260919-05   # the 13 package files and the published revision 5 catalog
WORK=target/catalog-candidates-$(date -u +%Y%m%d)-0$REVISION
```

### 1. Make the unsigned candidate

```sh
mkdir -p "$WORK"
cargo run --locked --features signing-tools --example prepare_release_catalog -- \
  "$PACKAGES" "$TAG" "$REVISION" "$WORK/catalog-r$REVISION.unsigned.json" 21
```

The last argument is the window in days (1 to 31, default 21). The tool backdates
`issued_at` by one hour, so a client whose clock is slightly behind accepts a
fresh catalog.

### 2. Review it

```sh
python3 tools/check_catalog_candidate.py "$PACKAGES/catalog.aklc" "$WORK/catalog-r$REVISION.unsigned.json"
sha256sum "$WORK/catalog-r$REVISION.unsigned.json"
```

It must end with `CANDIDATE_OK`. It checks that the revision is exactly the
previous one plus one, that every package record (id, revision, size, SHA-256,
tag, asset) is identical to the previous catalog, that the window is exactly 21
days and has just started. Write the SHA-256 down. If the package list should
change (a new language or a new package revision), this is a different kind of
release: do not use this runbook unchanged.

### 3. Build the signer (key owner's Windows profile)

```powershell
cargo build --release --locked --features signing-tools --bin sign-language-package
Get-FileHash -Algorithm SHA256 target\release\sign-language-package.exe
```

The signer holds no key. It loads the encrypted key of the release signer listed in
`data/package-signing/public-key.json` from
`%LOCALAPPDATA%\AutoKeyboardLayot-Signing\<signer>\private.pkcs8.dpapi` and works
only in the profile that created it. Do not distribute it.

### 4. Sign (needs the owner's approval; uses the release key)

```powershell
$work = 'target\catalog-candidates-YYYYMMDD-06'   # the directory of step 1, with today's date
$exe  = (Resolve-Path target\release\sign-language-package.exe).Path
$in   = (Resolve-Path "$work\catalog-r6.unsigned.json").Path
$exeHash = (Get-FileHash -Algorithm SHA256 $exe).Hash.ToLowerInvariant()
$inHash  = (Get-FileHash -Algorithm SHA256 $in).Hash.ToLowerInvariant()
# $inHash must equal the value written down in step 2.
$out = Join-Path (Split-Path $in) 'catalog.aklc'
powershell -ExecutionPolicy Bypass -File tools\sign-package.ps1 -Kind catalog `
  -Executable $exe -ExecutableSha256 $exeHash -InputFile $in -InputSha256 $inHash -OutputFile $out
```

`tools/sign-package.ps1` refuses a different executable or input than the hashes
you pass, refuses to overwrite an existing output, gives the signer 15 seconds
and prints the output's SHA-256 as JSON. Any failure is final: inspect, do not
retry blindly. The signer validates the whole catalog again before it signs and
verifies its own output before it writes it.

### 5. Verify the signed file offline

```sh
cargo run --locked --example verify_release_catalog -- "$WORK/catalog.aklc" "$PACKAGES"
cargo run --locked --example check_catalog_expiry -- "$WORK/catalog.aklc"
python3 tools/check_catalog_candidate.py "$PACKAGES/catalog.aklc" "$WORK/catalog.aklc" --max-age-hours 24
```

The first command checks the signature against the trust list that is compiled in
and every referenced package against the 13 files in `$PACKAGES`; the second
needs at least 240 hours (10 days) of validity left; the third shows that the
signed catalog still has exactly the reviewed content.

### 6. Keep the catalog that is published now

```sh
mkdir -p rollback && gh release download "$TAG" --repo woffko/AutoKeyboardLayot \
  --pattern catalog.aklc --dir rollback
sha256sum rollback/catalog.aklc
```

For revision 5 these bytes have the SHA-256
`b0e1d636bd7057e6bdb0393cf77d3f8d1cbf7d9ebffd478ea70d0c340c1ea503` and are also
kept in `tests/fixtures/mutation-seed-catalog.aklc`.

### 7. Publish (needs the owner's approval; replaces a published asset)

```sh
gh release upload "$TAG" "$WORK/catalog.aklc" --clobber --repo woffko/AutoKeyboardLayot
```

`https://github.com/woffko/AutoKeyboardLayot/releases/latest/download/catalog.aklc`
resolves to the `catalog.aklc` of the release marked **Latest**, so that release
must be `$TAG` (check the releases page). If a newer release has been marked
Latest since, upload to that release instead and keep the package tag in the
catalog unchanged.

### 8. Check what users will get

```sh
curl --fail --location --proto '=https' --proto-redir '=https' --max-time 120 --max-filesize 1048576 \
  https://github.com/woffko/AutoKeyboardLayot/releases/latest/download/catalog.aklc --output published.aklc
sha256sum published.aklc "$WORK/catalog.aklc"        # the two lines must match
cargo run --locked --example check_catalog_expiry -- published.aklc
gh workflow run catalog-freshness.yml --repo woffko/AutoKeyboardLayot
```

Then, on a real desktop, open Settings, Input languages, Add language, refresh the
catalog, and check that the installer's language page lists Russian and Estonian
(checklist item M7 in [`audit-remediation-2026-09.md`](audit-remediation-2026-09.md)).

## Rollback

- **A wrong file was uploaded and nobody has loaded it yet:** upload the file from
  step 6 again with `--clobber`. That only helps while the old catalog is still
  inside its window; revision 5 is expired, so rolling back to it restores the
  outage.
- **Clients that already accepted revision N refuse every lower revision, and
  refuse revision N again with different content.** A bad revision 6 is replaced
  by a corrected revision 7 (repeat the steps with `REVISION=7`), never by
  revision 5 or by another revision 6.
- Keep every signed catalog and its SHA-256 in the release notes of the release
  that holds it.

## When to renew

`catalog-freshness.yml` runs daily and opens or updates an issue when fewer than
10 days (240 hours) of validity are left. A 21-day window therefore gives about
11 days of margin; renew as soon as the issue appears. Each renewal needs the key
owner, so put the next date in a calendar when you publish.
