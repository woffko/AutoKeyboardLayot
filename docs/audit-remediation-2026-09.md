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
| Local verification | Several checks were not run anywhere locally. | `tools/verify-wsl.sh` runs formatting, Clippy in five configurations, tests, locale validation, Python tests and the Windows unit tests on the Windows host. |

## How it was verified

`tools/verify-wsl.sh` passes at the final commit (12 steps). The Windows unit tests run on the host
with a temporary profile. Library tests on Linux went from 281 to 314 and the Windows test suite from
126 to 149. The tests for the Exit and correction-gate behavior were written first and failed on the
audited commit. The behavior that only a real desktop can show (the exit prompts, the hook and tray
repairs, the paste selection) is covered by the manual checklist below.

## Not done here

- Renewing and publishing the catalog, signing, releasing, installing on a host and deleting host files
  need the owner's approval and are listed with their commands in the progress notes.
- Regression nets for terminal false positives, parser mutation smoke tests and layout-model limits.
- Release staging for the next version, trust-root resilience, repository hygiene, a threat model and CI
  policy (dependency advisories, license checks).
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
