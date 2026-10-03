# Audit remediation, September 2026

A code audit of the repository state at commit `eec5b4b` (2026-09-30) found 26 issues. This page lists
what was changed in response, what is deliberately left for later, and how to check the result. It
describes behavior and verification, not attack details.

## What changed

| Area | Before | Now |
|---|---|---|
| Diagnostics privacy | The optional diagnostics log recorded the virtual-key code of unsupported keys, which includes digits. | Key codes are never logged. A test scans every diagnostic format string; the README states exactly what is kept in memory and what the log contains. Delete `diagnostics*.log` files written by earlier builds. |
| Crash visibility | A panic in the GUI process left no trace. | `crash.log` records one line per panic (time, thread, source location, version; never the message), capped at 256 KiB and rotated. |
| Tray Exit | A worker that panicked while busy made Exit impossible for ever. | A finished worker never blocks Exit. A declined Exit asks whether to exit anyway and names the reasons; it warns when keystrokes held back for recovery would be lost. A graceful exit that does not finish offers the same prompt. Installer-driven closes never force. |
| Contained faults | A panic in a hook callback or the window procedure aborted the process. | The event is passed on untouched, held keys are released unchanged, automatic conversion pauses and a tray notice points to `crash.log`. A worker that panics three times within a minute stops. |
| Correction gate | Early-return paths left held keys waiting for the 1.2 second deadline, which aborted the gate and paused automatic conversion. | Every armed gate is released after the worker handled the Space, on any path. A gate timeout whose keys come back in full no longer leaves automatic conversion off (up to three times an hour). New diagnostics: `armed`, `release_forced`, `auto_resumed`, `ui_stall`, `hook_slow`. |
| Protected paste | A selection that could not be confirmed stayed selected; a confirmed edit with an unrestored clipboard counted as a failure. | The selection made by the agent is collapsed on every non-matching outcome; a selection made by the user is never touched. An unrestored clipboard is logged and counted, and automatic conversion stays on. Restoring waits up to 60 ms. |
| Hook health | Hooks were never checked again after a session change, sleep, or an Explorer restart. | The tray icon returns after an Explorer restart. Unlock, reconnect and resume reinstall both hooks. A watchdog reinstalls silent hooks under strict conditions (user typing, default desktop, foreground window of no higher privilege), with rate limits and a notice. Tray updates are throttled. |
| Privacy probe | A UI Automation call that never returned left privacy unavailable until restart. | A provider stuck for 5 seconds is replaced by a fresh one, at most three times. |
| Configuration saves | A single stale `config.tmp` blocked every later save. | Temporary files get unique names and stale ones are cleaned up. A failed write removes its file. |
| Layout model parser | A corrupt header could make the parser reserve tens of megabytes before checking the data. | The announced payload size is checked first; a test measures the largest allocation. |
| Build identity | The version shown in Settings did not reveal a build from uncommitted changes. | `0.1.0 (abc1234-dirty)` when tracked build inputs differ from the commit. The build script stays cheap to re-run. |
| Word-language context | The language of a word was recorded as converted even when no conversion was attempted. | The target language is recorded only when a conversion is attempted; a conversion failure clears the context. |
| Package catalog | The published catalog expired on 2026-09-26; the tool hard-coded seven days; users saw a raw error code. | `prepare_release_catalog` takes `VALIDITY_DAYS` (default 21, at most 31) and backdates `issued_at` by one hour; `check_catalog_expiry` takes `MIN_HOURS`; the daily workflow alerts through an issue; Settings and the installer show a dedicated message in all 14 languages. **Renewing the catalog itself needs the release key and is not part of this change.** |
| Licensing notes | The README said no license had been selected. | The README states the MIT license and where third-party licenses live; the layout-model notice joins the installer notices; `docs/licensing-notes.md` lists open questions for the owner. |
| Host deployment | An untracked script with machine-specific values; old backups piled up. | `tools/deploy-host.ps1` with `-WhatIf`, rollback and backup pruning, described in `docs/host-deployment.md`. |
| Local verification | Several checks were not run anywhere locally. | `tools/verify-wsl.sh` runs formatting, Clippy in six configurations, tests, locale validation, Python tests and every Windows test executable (library, agent, integration; default and modular configurations) on the Windows host. |
| Terminal false positives | No measurement of how often real command and package names are converted. | `tests/fixtures/dev-tokens.txt` holds 1 659 such names; `tests/dev_tokens.rs` fails when more than 6 are converted by the dictionary stage or more than 1 more by the layout model (0.36% and 0.06% today). `cargo run --example measure_token_false_positives` lists them. Detection policy is unchanged. |
| Parser robustness | Untrusted-input parsers were only checked on hand-written cases. | `tests/parser_mutation_smoke.rs` feeds seeded mutations of in-repo seeds to 13 parsers (configuration, settings, lexicons, rules, hotkeys, models, locale catalogs, installer protocol, layout model, signed catalog and package) and requires zero panics, in about 8 seconds. |
| Layout-model limits | The thresholds and gates of the second stage were only partly tested and not documented. | New tests for the confidence thresholds, the known-word gate, the three-letter rule and context-settled ambiguity; `docs/layout-model.md` has a Limits section with measured numbers for real tokens and random text. |
| CI policy | CI ran `cargo audit` without a policy, never built the signing tools and checked no licenses. | `.cargo/audit.toml` ignores exactly four accepted "unmaintained" notices, each with its reason, and CI runs `cargo audit --deny warnings`; `deny.toml` allows a short list of licenses and crates.io only, checked by a pinned `cargo-deny`; CI runs Clippy and the tests with `signing-tools` on Linux and Windows. The Windows Clippy error that blocked the signing tools (`items_after_test_module`) is fixed, and the local gate has two new steps for the signing utility on Windows. |
| Release staging | The published build was older than `main`, always 0.1.0, and its notices were not rebuilt. | Version 0.1.1. `THIRD-PARTY-NOTICES.txt` is regenerated through the reviewed pipeline, with the layout-model notice, and its SHA-256 is recorded in `docs/release-0.1.1.md` for the owner to review; the installer is built only with that reviewed hash and is **not built yet**. Draft release notes. An optional Authenticode step in the installer build (off unless `AKL_SIGNTOOL`, `AKL_SIGN_THUMBPRINT` and `AKL_SIGN_TIMESTAMP_URL` are all set). Nothing is uploaded. |
| Catalog renewal | The published catalog expired on 2026-09-26 and nothing described how to replace it. | `docs/catalog-renewal.md` is the runbook with the exact commands, the two steps that need the owner (signing, replacing the asset), verification and rollback. `tools/check_catalog_candidate.py` reviews a candidate against the catalog it renews (same records, revision plus one, exact window, fresh). **Revision 6 was signed and published on 2026-10-03** (valid until 2026-10-24): the anonymous download matches and all 13 packages match the catalog. |
| Trust root | One release key; losing it with its Windows profile would have made new catalogs impossible to sign. | `public-key.json` may use format 2: one to eight keys with the roles `release` (exactly one) and `recovery`. Every listed key verifies, only the release key signs, and the signing tool and preparation examples ask `PackageTrust::release_signer()`. The published file is unchanged (format 1) and no recovery key exists yet; the owner's offline ceremony is in `docs/package-signing-key.md`. |
| Fast typing in terminals | In Windows Terminal the check of the focused field sometimes answered late while the user typed, which made the word being typed manual-only, so a word in a quick series was occasionally not converted. Found in the first manual run on the host. | A late field-inspection answer keeps the verdict of a check that passed less than two seconds earlier for the same window, focus and input epoch, unless input that can move the focus happened in between; a password field, a changed context and a failure that lasts stand as before (README, `docs/threat-model.md`). The diagnostics log now gives the reason for every typed word that is not converted (`event=word result=`). |
| Repository hygiene | Tracked VM scripts named an internal address, an account and a profile path; `handoff.md` was tracked; no `SECURITY.md`; no Dependabot. | The five tracked VM scripts read the VM from `AKL_VM_*` variables through `tools/vm_config.py` and refuse to run without them (`tools/test_vm_scripts.py` runs the drivers against a fake ssh). `handoff.md` is untracked and ignored (the file stays on disk). `SECURITY.md`, `docs/threat-model.md` (code running as the same user is out of scope; F13 is an accepted risk) and weekly Dependabot checks are added. The GitHub settings are the owner's: `docs/repository-settings.md`. |

## How it was verified

`tools/verify-wsl.sh` passes at the final commit (15 steps). Every Windows test executable (the
agent's, the library's, the integration tests and the signing utility's) runs on the host with a
temporary profile, in the default and in the modular configuration. Library tests on Linux went from
281 to 325 and the Windows agent tests from 126 to 150; the Python tests cover the CI policy, the VM
scripts, the catalog checker and the Authenticode hook. The first pull request showed why the last
step exists: two tests passed on Linux and in the default Windows configuration but failed natively
on Windows (a directory in the way of a temporary file name reports "access denied", not "exists") and
in the modular configuration (a test needed the bundled Russian dictionary). The tests for the Exit
and correction-gate behavior were written first and failed on the audited commit. The behavior that only a real desktop can show
(the exit prompts, the hook and tray repairs, the paste selection, the startup message for an
unreadable configuration) is covered by the manual checklist below.
The first manual run on the owner's desktop found one more problem that no unit test could show: in a quick
series of words in Windows Terminal an occasional word was not converted, because the field check answered late
(after 10 of 14 conversions in that log, a check failed for about a tenth of a second). The fix and its tests are
described in the row "Fast typing in terminals".

## Not done here

- Releasing needs the owner's approval. The catalog was renewed and the build was installed on the owner's
  host on 2026-10-03 (old diagnostics logs and old backups removed); the next catalog renewal is due before
  about 2026-10-14 (`docs/catalog-renewal.md`).
- Building, signing and publishing the 0.1.1 installer, and creating its release: the owner's steps, listed in
  `docs/release-0.1.1.md`.
- The 16 other VM scripts under `tools/` and `docs/vm-installer-acceptance.md` are still untracked on the
  owner's machine; they carry the same machine-specific values and need the same change before they are
  committed.
- Splitting the large Windows adapter into modules.
- `Shell_NotifyIconW` still runs on the thread that owns the hooks; moving it to its own thread is part of a
  later dedicated-hook-thread change. The watchdog repairs the hooks if a blocked call costs them.
- Open licensing questions and the application icon's provenance statement.

## Manual acceptance checklist

1. **M1** Tray Exit while idle: the process ends within 2 seconds; starting it again works.
2. **M2** If Exit is ever declined: the dialog names the reason, "Yes" ends the process, and
   `diagnostics.log` has `phase=shutdown`.
3. **M3** `taskkill /f /im explorer.exe`, then start Explorer: the tray icon returns within about 3
   seconds, Settings opens, and `ghbdtn ` still becomes `привет`.
4. **M4** Ten minutes of typing with many spaces: no `phase=abort` and no automatic pause.
5. **M5** Notepad: copy some text, convert 20 words, paste: the clipboard is preserved and no word stays
   selected.
6. **M6** Win+L lock and unlock, and sleep and resume: automatic conversion still works afterwards.
7. **M7** After the catalog is renewed: Settings, Add language, check the catalog; the installer language
   page lists Russian and Estonian.
8. **M8** Ten minutes of terminal work with automatic conversion on: note unwanted conversions and the
   `stage=` values in the log.
