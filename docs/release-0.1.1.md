# AutoKeyboardLayot 0.1.1: release notes (draft)

**Status: draft.** Nothing has been built for release, signed, uploaded or published. The
installer is built only after the owner has reviewed the third-party notices and passed
their SHA-256 to the build; the steps are under "Before publishing". Replace this line with
the source commit and the installer checksums when the release exists.

0.1.1 is the first build after the September 2026 audit. It fixes behavior that could leave
automatic conversion switched off, keystrokes held back, the tray menu unable to exit, or
the keyboard hooks silently dead. The reasoning and the test evidence for each item are in
[`audit-remediation-2026-09.md`](audit-remediation-2026-09.md).

## What changes for users

### Reliability

- **Exit always works.** A worker that crashed or is busy no longer makes tray, Exit
  impossible. If Exit is declined, a prompt names the reasons and offers to exit anyway, and
  warns when keystrokes held back for recovery would be lost. Installers that close the
  program never force it.
- **Held keys are never stranded.** After a Space, every key held back by the correction gate
  is released on every path, not only when the word was converted. A gate that timed out but
  got all its keys back no longer leaves automatic conversion off; it resumes (up to three
  times an hour).
- **A fault no longer ends the program.** A panic in a hook callback or the window procedure
  passes the event on untouched, releases held keys unchanged, pauses automatic conversion and
  shows a tray notice. A worker that panics three times within a minute stops.
- **Hooks come back.** Unlock, reconnect, resume from sleep and an Explorer restart reinstall
  the hooks and the tray icon. A watchdog repairs hooks that stopped receiving input, under
  strict conditions and with rate limits.
- **A stuck privacy check heals.** A UI Automation provider that stops answering for 5 seconds
  is replaced instead of leaving the privacy check unavailable until restart.
- **Protected paste leaves nothing behind.** The selection made by the program is collapsed
  when the outcome is not the expected one (a selection you made is never touched), and a
  clipboard that could not be restored is counted and logged without switching conversion off.
- **Configuration saves cannot jam.** Leftover temporary files no longer block every later
  save.

### Privacy and diagnostics

- The optional diagnostics log never records key codes. Earlier builds logged the virtual-key
  code of unsupported keys, digits included: **delete `diagnostics*.log` files written by
  earlier versions** (in `%LOCALAPPDATA%\AutoKeyboardLayot`).
- `crash.log` (one line per panic: time, thread, source location, version; never the message
  or any typed text) is written next to the configuration. It is capped at 256 KiB.

### Messages and startup

- When the language catalog has expired, Settings and the installer say so in all 14 languages
  instead of showing an error code. (The expired catalog itself is renewed separately; see
  below.)
- A second copy of the program now exits quietly without reading the configuration.
- If `config.ini` cannot be read, the start-up message names the folder and says how to start
  again (move the file away) or restore a `config.schema-N.bak` copy. The new text is a
  machine-translated draft in 13 languages; language packs that are already installed show it
  in English until they are replaced.

### Smaller changes

- Settings and About show `0.1.1 (abc1234)`, with `-dirty` after the commit for a build made
  from uncommitted changes.
- The language of a word is remembered only when a conversion was attempted, which gives the
  experimental layout model the right context.
- A damaged layout model file can no longer make the program reserve a large amount of memory
  before it checks the data.

## Language catalog and packages

No package changes in this release: the 13 packages of catalog revision 5 are unchanged, and
installed packages keep working. The published catalog had expired on 2026-09-26; a renewed one
(revision 6, signed on 2026-10-03 and valid until 2026-10-24 14:39 UTC) is published, so online
language installation works again. The catalog has to be renewed before every expiry with the
owner's signing key; the runbook is [`catalog-renewal.md`](catalog-renewal.md), and the next
renewal is due before about 2026-10-14.

## Known limits

- Typing machine-generated lowercase text (random identifiers, hashes written in letters) in
  a terminal can still be converted; the measured rates are in the Limits section of
  [`layout-model.md`](layout-model.md). Real command and package names are converted in
  0.36% of the corpus tested.
- Neither the program nor the installer is Authenticode-signed unless the owner signs them
  with a certificate; expect SmartScreen and antivirus warnings for a new file.
- There is still no update mechanism: install the new release yourself.
- The tray icon is still refreshed on the thread that owns the keyboard hook; the hook
  watchdog repairs the hooks if that call ever blocks.

## Before publishing

These steps are the owner's. The installer cannot be built earlier: the build refuses notices
that nobody has reviewed.

1. **Review the third-party notices.** The regenerated file includes the layout-model notice
   and the dictionary license, and is bound to the exact `Cargo.toml` and `Cargo.lock` below.
   Read it, then take its SHA-256 as the reviewed value. A different `Cargo.toml` or
   `Cargo.lock`, or any dependency change, makes it stale and the pipeline has to run again.

   | | |
   |---|---|
   | `THIRD-PARTY-NOTICES.txt` | `4c62f14d32631cc6f16051201ec7bf83cb8a9fbe6e2403cf307e9c35f1568ae1` |
   | `notices-report.json` | `83d23861b676235c3d5026c7ddf0278d55ce0071c693f07c12c3177ff28552b4` |
   | build receipt | `1d3799ef53a07499f7c7eeddda65fa1b6cc6eba29e3144c2234cdcb4fee2aa2f` |
   | `Cargo.toml` | `a401b5b767fbf0c984f0f119243ff04b3ce00cb012860c937206d98673322bb1` |
   | `Cargo.lock` | `2c251fec8048fadbfe737220fad2f01de0eac9713fce82b8d1f5fe2b3aa0a3b6` |

   Files: `target/base-notices-complete-20261003-01/` and
   `target/base-build-receipt-20261003-01.json` (not tracked; `target/` is local). The 339
   third-party packages all have license text, and `missing` is empty.

   What changed since the notices of the previous installer (SHA-256
   `27938ef2e8d9266f57d732bd6fffc8581e1be72eca214d2c23332da4ed296ceb`): exactly one section was
   added, "Bundled layout model" (28 lines, the CC BY-SA 4.0 notice of the model data), and
   nothing was removed or altered. Compare the two files to confirm it:
   `diff target/base-notices-complete-20260913-01/THIRD-PARTY-NOTICES.txt target/base-notices-complete-20261003-01/THIRD-PARTY-NOTICES.txt`.
   The pipeline that made them (`tools/export_windows_test_artifacts.py`,
   `tools/collect_dependency_notices.py`, `tools/revalidate_notice_supplement.py`) ran offline
   and did not fetch anything; the 17 cached upstream notice groups were revalidated against the
   current sources and hashes.

2. **Build the installer** with the reviewed hash:

   ```sh
   python3 tools/build_experimental_installer.py --output target/installer-0.1.1 \
     --notice-directory target/base-notices-complete-20261003-01 \
     --build-receipt target/base-build-receipt-20261003-01.json \
     --notice-sha256 REVIEWED_NOTICE_SHA256 \
     --iscc PATH_TO_ISCC.exe
   ```

   The output directory must not exist. `build.json` in it records the SHA-256 of the
   program, the package helper and the installer; copy them into this page.

3. **Optional: sign with Authenticode.** Set all three variables before step 2, or none:
   `AKL_SIGNTOOL` (path of `signtool.exe`), `AKL_SIGN_THUMBPRINT` (SHA-1 thumbprint of the
   code-signing certificate in a Windows store) and `AKL_SIGN_TIMESTAMP_URL` (an RFC 3161
   timestamp server). The build signs copies of the program and the helper, compiles the
   installer from them, signs the installer, verifies every signature with `signtool verify`
   and records the thumbprint in `build.json`. No password or key file is ever passed.

4. **Accept it on a desktop.** Run the manual checklist M1 to M8 in
   [`audit-remediation-2026-09.md`](audit-remediation-2026-09.md) and, if the installer is the
   change under test, the VM scripts (see `tools/vm_config.py` for the settings they need).

5. **Keep the catalog current.** Revision 6 is published (see above). If the 0.1.1 release is
   going to be marked Latest, it must also carry the signed `catalog.aklc` (step 6).

6. **Publish.** Create the release, upload the installer and its checksums, download
   everything anonymously and compare the hashes (the r5 procedure in
   [`release-2026-09-19.md`](release-2026-09-19.md)). The program fetches the catalog from
   `releases/latest/download/catalog.aklc`, which follows whichever release is marked Latest, so
   a release that becomes Latest must carry the signed `catalog.aklc` too. The packages need
   no copy: the catalog names the tag that holds them.

## Checksums

Installer: not built yet.
