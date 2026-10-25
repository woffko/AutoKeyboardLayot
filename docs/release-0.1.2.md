# AutoKeyboardLayot 0.1.2: release notes

**Status: published** on 2026-10-25 as the release
[`v0.1.2`](https://github.com/woffko/AutoKeyboardLayot/releases/tag/v0.1.2) (Latest), built from
`59b425725ba7d30a1154e9e647d585dfe386be33`. The packages and the catalog were signed with the
release key, and the release published, at the owner's approval at the moment of each step.

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

## Acceptance

- `tools/verify-wsl.sh` on the source commit: 15 of 15 PASS.
- Installer on the test VM (`tools/run_vm_installer_ui.py`): `retry`, `uninstall-confirmation`,
  `startup-removal`, `settings-open`, `helper-failure`, `no-store`, `missing-exe`, `fence-invalid`
  and `timer-exception` pass. `download-failure`, `cancel-restarts-agent` and `late-exception`
  need test builds of the package helper whose downloads or catalog replies fail; they passed
  with those in goal R7, and neither the setup scripts nor the helper changed since. After the
  publication, `retry` passed again against the published catalog revision 8 (13 rows; a
  language downloaded and installed).
- Every package verified offline (`verify_language_package`: `RELEASE_SIGNATURE_VERIFIED`), and
  the catalog with all 13 packages (`verify_release_catalog`); 502 hours of validity.
- After the publication, downloaded without credentials: `releases/latest/download/catalog.aklc`,
  the installer from the same path, and all 13 package URLs the catalog names; every file
  matched the local one byte for byte.

## Checksums

| File | Bytes | SHA-256 |
|---|---|---|
| `AutoKeyboardLayot-0.1.2-setup-experimental.exe` | 11208740 | `91d9ddfd6920499e8bd78167883e24090fbc50833e0d48a94df05cc29d16c613` |
| `AutoKeyboardLayot.exe` (inside) | 22827520 | `c36b09080659c7a19b93129f21b7a5747ca27e85762abd16284c3059a6e4110d` |
| `installer-package-helper.exe` (inside) | 1773056 | `779f0706695f01882ee9741db421f058e8c32c96d0f7cab2aa229d2bd761f57a` |
| `catalog.aklc` (revision 8, valid 2026-10-25 03:58:38 to 2026-11-15 03:58:38 UTC) | 3462 | `733f6a4c93bf69bceeb4c90f994e14093db15508b3fd9cab67a1e976e17e3815` |
| `ru-RU-r5.aklp` | 34486974 | `ad619a22c783ae83d2593ee86446e05b5ecd1fb91d0a29d904b64fbfb4482743` |
| `et-EE-r3.aklp` | 3539659 | `72d3a3fd20eff661fd2b4b613aeccf45aafea810164f64963d5d943abcd43361` |
| `ar-SA-r2.aklp` | 34056 | `4b0f07647b127be4559d6c025cfda962dfcc769363ac738389aeea7f958ee0f2` |
| `bn-BD-r2.aklp` | 49590 | `f86b26df4214f80d612cad2fe410183cf93fc06c26fa4817aee313b03bf78ec3` |
| `de-DE-r2.aklp` | 29528 | `1f6bc23f2ea41dc2924f36df1bca4382d5a1f26572c96dad27b4dba746553b8c` |
| `es-ES-r2.aklp` | 28698 | `11b9075625fc76ea81ff09546cf6312d856b1f1a012212ada69455d236f64e1e` |
| `fr-FR-r2.aklp` | 29880 | `2e53e6c19bdf64fe006605e1ae05f311534b864183e078912edb9e932a71d1d2` |
| `hi-IN-r2.aklp` | 48498 | `680c1e92c17905b30a783ce7b049d95e50e0f602135267e2add83ae74c2f2223` |
| `id-ID-r2.aklp` | 26911 | `1c4b9a21d1e2db41e6825071496b4b29be54146483391e3aa82c100c21255991` |
| `ja-JP-r2.aklp` | 33092 | `8cbc2fb66c6a18fa53aa7251d31cac415c8feabfeca3e360c193cec1c8c81f2b` |
| `pt-BR-r2.aklp` | 28467 | `b3c351b0775a682f6c4722ab98da94cc07fd2d2246433d1f29fe70f9d90e951e` |
| `ur-PK-r2.aklp` | 36632 | `3b7350e333bdc6a402cc4f48c8ebc0eeafea9d30394f6b5c0a900e9d46109785` |
| `zh-CN-r2.aklp` | 25141 | `5031a04ea7abd2af4aea30e9a178f5035f5eb5cc86a3d8c16fa6128b23d120a1` |

Signer: `sign-language-package.exe` built from the source commit, SHA-256
`93ccbcbeef5dd69eb686e9380e7775ce167cb3acbdc96902d2ca68ce6ad6f4ca`; key `pkg-20260912-01`.
