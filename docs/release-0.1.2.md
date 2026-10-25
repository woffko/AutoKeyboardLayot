# AutoKeyboardLayot 0.1.2: release notes

**Status: in preparation.** The installer, the language packages and catalog revision 8 are built
from this commit; signing with the release key and publication need the owner's approval at the
moment they are done. The source commit and the checksums are filled in below once they exist.

0.1.2 is the first published build since 0.1.0 (release `lang-r5-20260919`). 0.1.1 was built for
acceptance on 2026-10-11 and never published; everything in its draft notes
([`release-0.1.1.md`](release-0.1.1.md)) is part of 0.1.2, together with the fixes of the two
October 2026 audits and the fixes found on the owner's machine after them.

## What changes for users

### Since 0.1.0

All of [`release-0.1.1.md`](release-0.1.1.md), "What changes for users": Exit always works, held
keys are never stranded, faults no longer end the program, games and virtual machines are left
alone, hooks come back after unlock, reconnect and sleep, Pause goes to the likely language
first and converts in more cases, the optional conversion sound, and the diagnostics log that
says why a word was not converted and never records key codes (**delete `diagnostics*.log` files
written by 0.1.0**).

### Messengers and other busy programs (Viber)

- **A conversion in a busy program no longer pauses automatic conversion.** After the Ctrl+V of
  a conversion the program checks that the field shows the replacement. Viber answers that
  check only once it has handled the paste; when it took longer than 150 ms, the conversion
  went through but was counted as failed, and automatic conversion paused in every program.
  The check now waits up to 400 ms more for a field that does not answer, as far as the time the
  correction gate gives it allows.
- **A clipboard another program holds open for a moment** (Explorer, the clipboard history
  service and many programs open it after every change) is tried again up to three times
  instead of leaving the word unconverted. When the replacement still cannot be put on the
  clipboard, the diagnostics log says why and which program held it.

### Words left unconverted now and then

- **The privacy check of the field waits longer.** Before each word the program asks the
  focused field whether it is a password field. The question is now one call instead of three
  and may take 250 ms instead of 100 ms, so a program busy right after a conversion or under
  fast typing still answers.
- **In a terminal, a word begun while the field could not be checked becomes automatic again**
  when a check of the same field passes before the word ends. Password fields, excluded
  programs and elevated windows stop conversion as before.
- **A failed check leans on the last good one for five seconds** (two before), for the same
  window and focus with nothing in between that can move the focus, and a conversion made in
  that field counts as such a check: Windows Terminal fails the check for seconds after an
  edit it handles.

### From the October 2026 audits

The fixes of the audits of 2026-10-16 (goals A–K) and 2026-10-21 (goals R1–R8); the reasoning and
the tests are in the audit fix plans and their status files:

- **Held keys and recovery:** keys held across a slow edit, a hand-back and a deadline are
  released or recovered without loss, and a configuration reload no longer drops them.
- **Privacy:** an excluded program and a password field are blocked from their first key, and
  nothing about the typing in them is written to the diagnostics log.
- **Clipboard:** a clipboard that could not be given back is counted, and a tray notice says
  so, so that you can copy it again.
- **Configuration and hotkey:** settings merge and save safely; a hotkey that would type text is
  refused, and a configuration naming one loads with the hotkey off.
- **Settings window:** layout and buttons work at the smallest width, in right-to-left
  languages, and while a refresh or the installer runs.
- **Detection, Pause, tray and watchdog:** the keypad period, a tray menu left open, Pause
  across a manual layout switch, a layout of no enabled language and digit-first words behave
  as expected.
- **Language packages and installer:** a failed download, a cancelled upgrade and a late error
  no longer leave a half-installed state; setup restarts an agent it closed.

## Language catalog and packages

All 13 language packages get a new revision, so that the messages added since 0.1.0 reach every
language:

| Package | Revision | Changes |
|---|---|---|
| `ru-RU` | 4 → 5 | interface messages; short words `был`, `мою`, `рук`, `лун` |
| `et-EE` | 2 → 3 | interface messages; short words `töö`, `ära`, `ühe`, `üle` |
| `ar-SA`, `bn-BD`, `de-DE`, `es-ES`, `fr-FR`, `hi-IN`, `id-ID`, `ja-JP`, `pt-BR`, `ur-PK`, `zh-CN` | 1 → 2 | interface messages |

The dictionaries, the scoring data and the input rules are unchanged. Every new package requires
runtime API 2, which 0.1.0 already supports. Catalog revision 8 lists the 13 new packages; the
previous packages stay in the release `lang-r5-20260919`.

## Known limits

- Typing machine-generated lowercase text (random identifiers, hashes written in letters) in a
  terminal can still be converted; see the Limits section of [`layout-model.md`](layout-model.md).
- A field busy for longer than the late wait after a paste (about half a second) still ends
  unconfirmed and pauses automatic conversion.
- Neither the program nor the installer is Authenticode-signed; expect SmartScreen and antivirus
  warnings for a new file.
- There is still no update mechanism: install the new release yourself.

## Third-party notices

Reviewed and accepted by the owner on 2026-10-25:

| | |
|---|---|
| `THIRD-PARTY-NOTICES.txt` | `fa74a6d90b20d697469f26dc6578589c9e86efbded6e35080a7113f7681a5d04` |
| `notices-report.json` | `ace88fe4d0557f262db6cdfa2cb9b96047b7c7ccf64c00edb56056474977812d` |
| build receipt | `target/base-build-receipt-20261025-01.json` (340 compiled package ids) |
| files | `target/base-notices-complete-20261025-01/` |

Against the notices accepted on 2026-10-11 (`4c62f14d…`): `clipboard-win` 5.4.1 → 5.4.3 with the
same BSL-1.0 text, and the "Bundled layout model" section names wordfreq 3.1.1 and credits the
SUBTLEX authors, as wordfreq's distribution of SUBTLEX requires. The 17 upstream license texts
were fetched again at the revisions Cargo records; 16 are byte for byte the cached ones.

## Checksums

Filled in once the release exists.
