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
- **Games and virtual machines are left alone.** While a full-screen program, a VMware,
  VirtualBox or Hyper-V console or a Remote Desktop window is in the foreground, the hook
  watchdog no longer reinstalls the hooks or warns that they are unstable: these programs take
  the keyboard themselves, and during one evening the watchdog reinstalled the hooks six times
  in a game and warned twice over it. The field check is spaced by its cost: a game that answered in about
  150 ms was asked about 6,100 times in an evening; it is now asked once per input target.
- **Hooks come back.** Unlock, reconnect, resume from sleep and an Explorer restart reinstall
  the hooks and the tray icon. A watchdog repairs hooks that stopped receiving input, under
  strict conditions and with rate limits.
- **A stuck privacy check heals.** A UI Automation provider that stops answering for 5 seconds
  is replaced instead of leaving the privacy check unavailable until restart.
- **Fast typing in a terminal keeps its conversions.** In Windows Terminal the check of the
  focused field sometimes answers late while you type, which made the word being typed
  manual-only, so one word in a quick series was not converted. A late answer now keeps the
  verdict of a check that passed less than two seconds earlier for the same window. A password
  field, a changed window, input that can move the focus and a failure that lasts still stop
  automatic conversion.
- **Pause goes to the likely language first.** The enabled languages are kept in alphabetical
  order, so a word typed in the English layout went to Estonian first and needed a second
  press to become Russian. The first press now goes to the language in which the word reads
  best, then on through the others and back to the original.
- **Pause converts in more cases.** A manual layout switch (for example Alt+Shift) no longer
  makes Pause forget the word typed before it, text typed right after a manual switch can be
  converted with Pause, and digits typed on the numeric keypad no longer stop Pause from
  reading a word back. Such digits are typed back as keypad keys: sent by scan code alone they
  arrived as navigation keys (keypad 2 is Down, which clears the line in a terminal), and a
  keypad digit typed right after a Space could arrive the same way. In applications whose edit checks the text before the caret
  (protected paste, such as messengers), Pause waits longer for the selection of the word
  and tries a second time, and after a switch to another window and back it can still
  convert the last word of that window for two minutes when the word is still exactly
  before the caret. When Pause still cannot act, the diagnostics log says why.
- **A busy clipboard no longer costs a letter.** When the clipboard belongs to a window that
  does not answer, an automatic conversion now gives up after 0.2 seconds and leaves the word
  alone instead of counting a failed edit and pausing automatic conversion, and keys held
  after a Space are handed back to the field even when it cannot be inspected at that moment.
- **Held keys no longer keep conversion off for hours.** Keys held after a Space that could
  not be handed back (for example because a click ended the hold) waited in memory until they
  were recovered or deleted from the tray menu. Meanwhile automatic conversion stayed paused
  and Exit was declined, so one unnoticed notice kept conversion off for a whole evening. They
  are now deleted after one minute (preparing a recovery gives one more minute), a second
  notice says so, and automatic conversion resumes (up to three times an hour).
- **Optional conversion sound.** A soft, quiet tick after each conversion and a lower one
  after an undo, switched on in Settings (off by default).
- **Protected paste leaves nothing behind.** The selection made by the program is collapsed
  when the outcome is not the expected one (a selection you made is never touched), and a
  clipboard that could not be restored is counted and logged without switching conversion off.
- **Configuration saves cannot jam.** Leftover temporary files no longer block every later
  save.

### Privacy and diagnostics

- The optional diagnostics log never records key codes. Earlier builds logged the virtual-key
  code of unsupported keys, digits included: **delete `diagnostics*.log` files written by
  earlier versions** (in `%LOCALAPPDATA%\AutoKeyboardLayot`).
- The diagnostics log now says why a typed word was not converted (`event=word result=` and
  one of `no-candidate`, `manual-only`, `privacy`, `suppressed`, `no-language`, with
  `reset=` naming what cancelled a suppressed word) without recording the word, and its
  privacy lines name what triggered them.
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
installed packages keep working. The published catalog had expired on 2026-09-26; renewed ones
are published (revision 6 on 2026-10-03, then revision 7, signed on 2026-10-11 and valid until
2026-11-01 15:19 UTC), so online language installation works again. The catalog has to be
renewed before every expiry with the owner's signing key; the runbook is
[`catalog-renewal.md`](catalog-renewal.md), and the next renewal is due before about 2026-10-22.

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
   `Cargo.lock`, any dependency change or a changed bundled data notice makes it stale and the
   pipeline has to run again.

   **Awaiting review:** the notices regenerated on 2026-10-19, after the layout model's
   `data/layout-model/NOTICE.md` began to credit SUBTLEX and to name the versions of its sources.
   The installer build compares the bundled data notices with the checkout, so it refuses the
   notices accepted earlier (below) until these are reviewed.

   | | |
   |---|---|
   | `THIRD-PARTY-NOTICES.txt` | `85fc4575582918abffabb4e91b3cf5734624cb16a2bf944fcd7c2f919205a9d3` |
   | `notices-report.json` | `3de30e10e48e1c76b4917e0c54911a7db0464324afb60cee6ec996b3e574ba03` |
   | files | `target/base-notices-complete-20261019-01/` |

   Only the "Bundled layout model" section differs from the accepted file (41 changed lines:
   the SUBTLEX credits with their citations and the wordfreq and Estonian source versions);
   `diff target/base-notices-complete-20261018-01/THIRD-PARTY-NOTICES.txt target/base-notices-complete-20261019-01/THIRD-PARTY-NOTICES.txt`
   shows it. The build receipt, `Cargo.toml` and `Cargo.lock` are the ones in the table below.

   **Accepted earlier (now stale):** the owner reviewed and accepted these on 2026-10-11.

   | | |
   |---|---|
   | `THIRD-PARTY-NOTICES.txt` | `4c62f14d32631cc6f16051201ec7bf83cb8a9fbe6e2403cf307e9c35f1568ae1` |
   | `notices-report.json` | `876e883cf54bd0cdc644c13a1c039f7d723f4eb1b1b141c40072f34715454291` |
   | build receipt | `1d3799ef53a07499f7c7eeddda65fa1b6cc6eba29e3144c2234cdcb4fee2aa2f` |
   | `Cargo.toml` | `a401b5b767fbf0c984f0f119243ff04b3ce00cb012860c937206d98673322bb1` |
   | `Cargo.lock` | `2c251fec8048fadbfe737220fad2f01de0eac9713fce82b8d1f5fe2b3aa0a3b6` |

   Files: `target/base-notices-complete-20261018-01/` and
   `target/base-build-receipt-20261003-01.json` (not tracked; `target/` is local). The notices were
   regenerated on 2026-10-18 because the report now also records the SHA-256 of the bundled data
   notices, which the installer build compares with the checkout; `THIRD-PARTY-NOTICES.txt` came out
   byte for byte the reviewed file, and the earlier report (`target/base-notices-complete-20261003-01/`,
   SHA-256 `83d23861…`) lacks that record, so the build refuses it. The 339
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
     --notice-directory target/base-notices-complete-20261019-01 \
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

5. **Keep the catalog current.** Revision 7 is published (see above). If the 0.1.1 release is
   going to be marked Latest, it must also carry the signed `catalog.aklc` (step 6).

6. **Publish.** Create the release, upload the installer and its checksums, download
   everything anonymously and compare the hashes (the r5 procedure in
   [`release-2026-09-19.md`](release-2026-09-19.md)). The program fetches the catalog from
   `releases/latest/download/catalog.aklc`, which follows whichever release is marked Latest, so
   a release that becomes Latest must carry the signed `catalog.aklc` too. The packages need
   no copy: the catalog names the tag that holds them.

## Checksums

Installer: built on 2026-10-11 from `1bed8d8` with the reviewed notices, for acceptance. It is not
signed with Authenticode, not yet accepted on a desktop, and not published.

| File | Bytes | SHA-256 |
|---|---|---|
| `AutoKeyboardLayot-0.1.1-setup-experimental.exe` | 11024611 | `dfb424283899ffb1425fbe78c5efa881bc787d3ceb0a1078995a64b7d792c917` |
| `AutoKeyboardLayot.exe` (inside) | 22345216 | `ca343db9f81c7ab796e681a4ea6e64575173d60f5a64ff0420d8e6fd93f06027` |
| `installer-package-helper.exe` (inside) | 1654272 | `86d5699aff779133137834d3954ad500b46c1b6ede7dcaf10a2eb0476e3a07af` |
| `THIRD-PARTY-NOTICES.txt` (inside) | | `4c62f14d32631cc6f16051201ec7bf83cb8a9fbe6e2403cf307e9c35f1568ae1` |
