# AutoKeyboardLayot

AutoKeyboardLayot is an experimental privacy-first Windows user-session agent
that detects words typed under the wrong keyboard layout. English and Russian
are the first automatic-correction pair. Estonian is a separate
dictionary-backed direct-layout pack using the installed Windows HKL. Japanese
is a separate IME pack and remains fail-closed until its composition adapter is
physically validated.

## Current milestone

This repository currently contains an experimental source snapshot, not a stable
release. Automatic replacement is triggered by Space, not Enter or Tab. Privacy
inspection timeouts can still cause skipped words, and physical input acceptance
remains incomplete. Use a disposable document for initial testing, not a live
terminal command or important unsaved text.

On a new profile, the alpha defaults to observation-only mode:

- deterministic offline EN/RU candidate detection backed by embedded FST
  dictionaries rather than a toy word list;
- a data-driven registry for EN, RU, ET, and JA packs, including `ET`/`JA`
  tray identities;
- an in-memory current-word buffer with edit/navigation suppression rules;
- a Windows foreground-layout monitor;
- a dynamic tray indicator;
- one agent instance per interactive Windows session;
- a low-level keyboard hook whose callback only queues bounded raw events;
- UI Automation password-field suppression and executable exclusions;
- an explicit `Automatic conversion: ON/OFF` tray toggle, with a blue badge when
  enabled and a gray badge when disabled, both showing white current-layout letters;
- a singleton Slint Fluent settings window in the same EXE's `--settings`
  process, with pages for
  general behavior, switching rules, languages, user words, word/process
  exclusions, backend routing, diagnostics, and About;
- executable browsing and selection from running processes for exclusions;
- atomic apply-and-hot-reload without opening managed lists in an external text
  editor or blocking the keyboard-hook thread;
- optional local diagnostics on a dedicated bounded writer thread, disabled by
  default and containing stages/counters rather than typed text;
- a versioned data-driven foreground-host backend rule table that runs before
  any UIA selection or clipboard mutation;
- a protected-paste pipeline for the currently validated Notepad host: exact
  UIA source selection,
  clipboard history/cloud exclusion, one paste chord, exact corrected tail,
  clipboard restoration, then a stable target-layout request;
- protected paste also requires an editable UIA `Document`/`Edit` control, but
  UIA control type alone never selects the backend;
- the seeded rules route Windows Terminal, OpenConsole, conhost, and Firefox to
  physical scan-code replay, and Notepad/Viber to protected paste;
- unknown foreground hosts use a capability probe: an exact editable UIA text
  barrier selects protected paste, while mismatch cancels without fallback;
- a physical-replay fallback for UIA-unsupported hosts is persisted separately
  and enabled by default on new profiles;
- focused-child layout switching with a single request and a continuously
  stable target-layout confirmation;
- a bounded correction-only key gate that replays held raw key edges behind
  the correction in boundary-sized chunks and opens or hands off only after a
  worker-acknowledged in-stream fence;
- a short decision gate armed on physical Space-down, before Space-up or the
  next rolled-over key can invalidate an already detected candidate;
- stale-input and physical-modifier guards that cancel a replacement if the
  caret context may already have changed;
- tracked asynchronous layout changes without optimistic local state changes;
- contextual Pause/Break: undo while an undo record is valid, otherwise force
  conversion of the word still under the caret without requiring Space;
- a configurable contextual hotkey, with modifier release checked before any
  injected correction, and opt-in dictionary additions after manual conversion;
- configurable suppression after Backspace, Delete, each arrow, Home/End, and a
  manual layout change, plus selectable EN/RU/ET automatic packs;
- no typed-text logging.

Automatic replacement is off at process start by default. A user can explicitly
enable startup conversion in settings. A conversion failure turns it off again
and exposes a technical failure counter in the tray status.

## Validated so far

- Linux formatting, 210 deterministic core/state-machine/configuration/localization tests,
  and strict Clippy checks;
- Windows target check, strict Clippy, and a native release build for the
  Fluent candidate;
- latest native Windows verification: 68 core tests and 75 adapter/UI-helper tests;
- Fluent window launch and responsiveness on the physical Windows host;
- UI Automation checks of Apply, hotkey modifier persistence, and stale-draft
  rejection in an isolated temporary profile;
- an explicit native protected-clipboard round-trip probe with format and text
  restoration;
- native Windows 11 MSVC test and release builds;
- an interactive-session launch without a visible error window;
- readable `EN` and `RU` tray icons and live foreground-layout updates;
- manually accepted physical observer typing and tray Exit behavior;
- native password-field and executable-exclusion privacy probes;
- an initial physical Firefox check in both EN→RU and RU→EN directions with the
  opt-in fallback enabled; diagnostics confirmed `strategy=PhysicalReplay`.

Two active-replacement builds passed terminal checks but were rejected after
corrupt output in Notepad; a UIA/RichEdit candidate failed closed, and the
first physical scan-code candidate still mixed queued source input with layout
changes. The current candidate resolves the stable top-level foreground
process through an in-memory backend rule table before touching UI Automation
or the clipboard. The validated Notepad path then uses protected paste: UI
Automation must select and
re-read the exact source range, the temporary
clipboard item carries all three Windows history/cloud exclusion formats, and
the exact replacement must be visible before the original clipboard is
restored. Clipboard sequence and ownership checks prevent overwriting a newer
external Copy. Windows terminal hosts take the physical path directly. Other
hosts probe the focused UIA element for one collapsed selection, an editable
control type, and an exact preceding source string before the clipboard is
opened. The probe walks a bounded ancestor chain but accepts text access only
from the exact foreground process; password or property errors fail closed. A
mismatch never falls back to replay. If UIA is wholly unavailable and the
explicit fallback setting is enabled, physical replay performs the correction
without opening the clipboard. Physical keys arriving during a
correction are held and replayed after the correction. Held input is
released only through the
first Space-down; the worker then either hands the still-closed gate to that
candidate or acknowledges the chunk before the suffix can continue. Undo is
exposed only when no held tail invalidated it. Each newly built candidate still
requires fresh physical Notepad, terminal, Viber, Firefox continuous-typing,
focus/newline, clipboard-preservation, and Pause/Break acceptance. Injected
test input is intentionally ignored by the hook and cannot stand in for
physical typing.

The EN/RU detector now keeps physical punctuation keys inside a pending token
when those keys represent letters in the other layout. This covers words such
as `hf,jnftn` → `работает` and `gthtrk.xtybt` → `переключение`. A valid word in
the current layout is a hard veto; automatic correction does not override it
even when another pack also contains a plausible word.

After a successful Pause/Break undo, the original source-layout word may be
held in memory for at most two minutes. A passive tray notification enables an
explicit "Do not correct the last undone word" action. The word is not
written anywhere unless the user selects that action.

A successful forced conversion can similarly offer its result for the user
dictionary. Suggestions expire after two minutes and are only persisted after
an explicit tray action. Selecting a hotkey with modifiers does not inject
corrections until those modifiers have been released. Selected-text conversion
is planned separately and is not implemented.

Enter and Tab end the current word without converting it, then re-check privacy
without suppressing the first word in the next context. A partially edited word
remains suppressed after Backspace, while a fully erased tracked word starts a
fresh detectable word immediately. After an observed Enter, Ctrl+Backspace can
also re-arm the first word when its setting is enabled. An additional Backspace
on an already empty line still invalidates the context because it joins the
line with preceding text. Pause/Break is reserved when its setting is enabled:
it triggers undo when available, otherwise it forces the current buffered word
through the same replacement pipeline even when automatic mode is off. Repeats
and key-up are swallowed so the hotkey cannot leak into the target editor.
Changing automatic mode clears stale queued state without suppressing the
first new word; only a real queue overflow keeps suppression until a boundary.

Session identity uses the stable top-level window and process, while the
focused input thread is tracked separately for HKL and focused-child routing.
Ordinary focus changes and mouse clicks re-check privacy on the first printable
key without suppressing that first word.

## Privacy boundary

The process retains at most the current word in volatile memory. It does not
write typed text to logs, configuration, diagnostics, or network services.
Unsupported input, editing, navigation, focus changes, layout changes, and
queue overflow clear or suppress the current context.

For the protected-paste backend, the replacement is placed on the global
clipboard only for the bounded paste transaction. The item opts out of Windows
clipboard monitoring, local history, and cloud upload. The prior clipboard is
materialized by a short-lived STA broker into local clipboard-compatible
`STGMEDIUM` values before replacement. The broker owns its hidden clipboard
window, pumps OLE messages, and restores those handles only while the temporary
sequence and owner are unchanged. This preserves delayed, private, GDI, and
enterprise-protection formats instead of discarding them. Password, excluded,
elevated, non-materializable clipboard, and ambiguous UIA contexts fail closed.
Third-party local clipboard readers remain outside the confidentiality
guarantees of the Win32 clipboard.

Settings and explicitly managed lists are stored atomically in the versioned,
human-readable `%LOCALAPPDATA%\AutoKeyboardLayot\config.ini`. On a profile that
does not yet have this file, the previous `settings.ini`, `user_dictionary.txt`,
`word_exclusions.txt`, and `exclusions.txt` files are imported in memory and
migrated on the first explicit Save/Apply. The legacy files are not deleted.
GUI and tray writes share a cross-process named Windows mutex. Apply rejects an
outdated draft if the configuration has changed since the window was opened,
preserving newly added dictionary entries. The settings application itself is
excluded from conversion.

Dictionary and word-exclusion entries accept `en-US: word`, `ru-RU: слово`, or
`et-EE: tere`. The GUI validates, normalizes, deduplicates, and rejects a word
present in both lists. Program exclusions are normalized to executable names.
The whole configuration is bounded to 1 MiB, loaded once into in-memory sets and
maps, and reloaded through the worker queue. No list is populated from typed
text without a direct user action.

Backend rules use `executable = backend`, where the backend is
`capability-probe`, `protected-paste`, `physical-replay`, or `observe-only`.
Exact full-path rules win over executable-name rules, and `*` defines the
fallback. User routing never bypasses password, integrity, foreground PID, UIA
ownership, or exact-text checks.

The `physical_fallback_for_unsupported_apps` setting allows scan-code replay
only when UIA is unavailable for an explicitly safe resolver reason. It is on
by default for new profiles and can be disabled in the settings window. A UIA
text mismatch or password/property failure always cancels, regardless of this
setting.

Automatic conversion is considered only when a physical Space completes a
word. The opt-in `single_letter_words` setting additionally evaluates
one-character words, but only against the pack's short-word tier or the user
dictionary; it is off by default and every other single-letter word stays
opt-in through the user dictionary.

`manual_terminal_uia_fallback` is a separate, default-off permission for manual
conversion in Windows Terminal, OpenConsole and Conhost when only the focused
field's UIA inspection is unavailable. The process and integrity checks, current
input epoch and target identity still have to succeed. A transport timeout,
changed target, unreadable process or confirmed password field does not qualify.
Routing an application to `physical-replay` alone never grants this permission.

Capability diagnostics expose only backend states such as `uia-paste`,
`physical-replay`, `capability-probe`, `unsupported`, or `text-mismatch`; they
never include the typed source or replacement.

`diagnostics_enabled` is off by default. When explicitly enabled, a dedicated
bounded queue writes `%LOCALAPPDATA%\AutoKeyboardLayot\diagnostics.log`. The
log contains timestamps, process basename/PID, backend stages, language IDs,
character counts, gate results, and edit outcomes. It never records words,
clipboard contents, window titles, URLs, or message text. The file rotates at
1 MiB and has no network transport.

## Language packs

Pack metadata is exposed by `language::language_packs()`. Dictionary sources,
checksums, and exact redistribution notices live under
`data/language-packs/`.

- `en-US`: direct layout, dictionary embedded, automatic correction;
- `ru-RU`: direct layout, dictionary embedded, automatic correction;
- `et-EE`: direct layout, dictionary embedded, and mapped from the buffered
  physical scan codes through the installed Windows Estonian HKL;
- `ja-JP`: `ImeRomaji` pack and tray identity; automatic correction awaits an
  adapter that can observe and preserve Windows IME composition.

The Windows mapper uses a fresh keyboard-state buffer and
`TO_UNICODE_DO_NOT_CHANGE_KEYBOARD_STATE`; it never mutates the foreground
thread's dead-key state. Caps Lock and extended scan codes are preserved.
Dead-key or multi-character mappings fail closed, and two plausible target
languages are treated as ambiguous rather than guessed.

An optional, experimental second stage for English, Russian and Estonian, the
[layout model](docs/layout-model.md), corrects wrong-layout words that the
dictionaries miss (for example inflected Estonian forms). It is off by default
and switched on or off in Settings → General; the same document explains how
to train a model for other languages.

The Windows adapter blocks known password-manager/system credential processes
by default. Additional executable names are managed on the
process-exclusions page; the built-in security list cannot be removed by a
user profile.

## Local checks

Use the Rust 2024 edition toolchain; the current snapshot was built with Rust
1.95.0. On Windows, install the MSVC toolchain and Windows SDK, then run:

```powershell
cargo build --locked --release
.\target\release\AutoKeyboardLayot.exe
```

The in-development localization layer embeds English and selects translations
using the Windows display language, independently of the keyboard layout.
External UI catalogs are validated against the embedded messages; missing
translations fall back to English. Thirteen external catalog drafts cover the
planned language set with all 141 current message keys each, including the new
input-package status messages. Strict completeness and placeholder validation
pass. Chinese uses an explicit Simplified-script catalog with regional
aliases; Arabic and Urdu require RTL layout acceptance. Linguistic/UI acceptance and
the input-plugin manager are still in progress; see
[the implementation plan](docs/localization-and-language-packs.md).
Repository documentation is English; dictionary data and language-specific
test fixtures retain their original languages.

```bash
cargo fmt --all -- --check
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
cargo check --target x86_64-pc-windows-msvc
```

`tools/verify-windows.ps1` runs native format/tests/Clippy/release checks, using
an isolated temporary profile for adapter tests. `tools/start-windows-build.ps1`
launches that script independently of SSH through WMI and monitors its result.
Each run requires a fresh log directory; per-stage logs and `result.json`
remain there even if the monitoring connection is interrupted.

## Third-party notices

Dictionary source information and redistribution notices are included in
`data/language-packs/`. The layout model's data sources and license are in
`data/layout-model/NOTICE.md`. The settings UI uses Slint. Dependency versions are pinned
by `Cargo.lock`; upstream dependency licenses remain applicable. No project-wide
license file has been selected for this initial source snapshot.
