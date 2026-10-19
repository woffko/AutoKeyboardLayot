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

On a new profile, automatic conversion is on from the start. The alpha provides:

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
  exclusions, backend routing, diagnostics, and About. It renders with OpenGL 2.0
  or later: where the display adapter offers only Windows' built-in OpenGL 1.1 (a
  virtual machine with the basic display adapter, for example) it does not open
  until the Microsoft OpenGL Compatibility Pack is installed;
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
- an optional soft, quiet sound after each conversion and a lower one after an
  undo (`conversion_sound`, off by default), played on a thread of its own so a
  conversion never waits for audio;
- no typed-text logging.

Automatic replacement is on at process start by default; it can be turned off in
settings (`automatic_conversion_on_startup`). A conversion failure turns it off
for the session and exposes a technical failure counter in the tray status.

## Validated so far

- Linux formatting, 210 deterministic core/state-machine/configuration/localization tests,
  and strict Clippy checks;
- Windows target check, strict Clippy, and a native release build for the
  Fluent candidate;
- latest native Windows verification: 68 core tests and 75 adapter/UI-helper tests;
- Fluent window launch and responsiveness on the physical Windows host;
- UI Automation checks of Apply, hotkey modifier persistence, and stale-draft
  rejection in an isolated temporary profile;
- native protected-clipboard checks on the real clipboard, run on the test VM
  with `--include-ignored`: format and text restoration with the opt-outs, content
  rendered on request by its owner, an unconfirmed paste that gives the
  clipboard back only after its target read the replacement, and a whole
  protected paste (its UI Automation and input steps scripted) into a field that
  reads the clipboard late;
- on the desktop of the test VM's interactive session: the agent itself (hooks,
  gate, worker) converting a word typed into a field of another process that is
  busy when the paste arrives, and the same paste driven directly (real UI
  Automation, input and clipboard): the field ends with the replacement and the
  user's clipboard comes back. The test's typing is injected with a marker that
  only test builds take as the user's. These tests need that desktop, so they run
  alone by name there; other runs pass `--skip on_the_desktop`;
- on the same desktop, the tray icon in the real notification area: hovering over it
  shows the standard tooltip with the status, and the shell does not ask the program
  for a popup of its own (`NIN_POPUPOPEN`) as it does without `NIF_SHOWTIP` (the XAML
  taskbar of Windows 11 26H2 then still shows the tooltip, a classic notification area
  shows none); Explorer restarted while an old conversion failure is on record gives the
  icon back without announcing it again; and the agent started while Explorer is
  stopped (with `AutoRestartShell` set to 0 for the test) keeps running and adds its
  icon once the taskbar is there;
- on the same desktop, the settings window, driven through UI Automation and input:
  the installer's graceful-close property appears once the window exists and the
  window closes on the installer's request; long messages — the close guard's and that
  of a save refused by a change made outside the window — are shown in full; one
  interface language stays marked after several clicks; the online review shows each
  chosen package's license through its package picker, which stays clear of the
  preview; the Add language dialog shows its failures inside; in Arabic the language
  rows are mirrored with their marks visible; Space starts a shortcut recording; the
  migration review shows in a legacy profile without opening the tools; the installed
  packages are listed at opening; file dialogs belong to the settings window and
  disable it; a maximized window stays maximized when Settings is opened again; while
  a recording runs, Shift with a letter and Ctrl+Alt with a letter are refused with
  their reasons and the recording goes on. The shortcut capture ignores injected input,
  so that test hands the chords to it the way its hook hands on keys that are not
  injected;
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
mismatch never falls back to replay. Without UIA nothing is converted: no field
can be checked for a password, so privacy fails closed. The explicit fallback
setting allows physical replay, without opening the clipboard, only when UIA works
but the probe cannot resolve the editable control for one of the safe reasons
listed below. Physical keys arriving during a
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
corrections until those modifiers have been released. Recording a shortcut in the
settings refuses chords that are typing, which the agent would otherwise swallow
everywhere: Shift with a letter or digit (a capital or a symbol) and Ctrl+Alt with a
letter or digit (AltGr on many layouts, without Win). It warns before Alt+F4 and
Ctrl+C, V, X, Z, A and S, which Windows and programs use. Selected-text conversion
is planned separately and is not implemented.

Enter and Tab end the current word without converting it, then re-check privacy
without suppressing the first word in the next context. A partially edited word
remains suppressed after Backspace, while a fully erased tracked word starts a
fresh detectable word immediately. After an observed Enter, Ctrl+Backspace can
also re-arm the first word when its setting is enabled. An additional Backspace
on an already empty line still invalidates the context because it joins the
line with preceding text. Pause/Break is reserved when its setting is enabled:
it triggers undo when available, otherwise it forces the current buffered word
through the same replacement pipeline even when automatic mode is off. Repeated
presses walk the word through every enabled layout: the first goes to the
language in which the word reads best (a dictionary word first), the next ones to
the remaining languages, and the last one back to the original; without a clear
reading the configured order is used. A layout that reads the keys exactly as the
original text (Estonian for Latin letters typed in English) counts as the
original, so the presses go back to the original layout instead of stopping in a
lookalike one. Repeats
and key-up are swallowed so the hotkey cannot leak into the target editor. A
manual layout switch (Alt+Shift, Ctrl+Shift or Win+Space) ends automatic
conversion of the word but not Pause: the word typed so far, or else the last
completed word, can still be converted; it is read in the layout it was typed in
and replaced from the layout the field shows now. Text typed right after a manual
switch is kept for Pause like text typed after a shortcut. Digits typed on the numeric
keypad can be part of such a word; they are typed back as keypad keys, because a
keypad scan code sent alone arrives as the navigation key that shares it (keypad
2 is Down). Keys held after a Space are replayed the same way. When Pause cannot act, the diagnostics log
names the reason (`event=hotkey result=ignored reason=` `no-word-before-caret`,
`key-without-character`, `stale-word`, `privacy`, `unsupported-layout`,
`single-layout` or `identical`).

In applications on the protected-paste backend, whose edit checks the exact text
before the caret before it replaces anything, Pause does more. It waits up to 100
ms for the field to confirm the selection of the word and selects a second time
when the field did not show it (automatic conversions keep 50 ms, which the
correction gate's budget accounts for). After you switch to another window and
back, or click, Pause can still convert the last word of that window for two
minutes: the word is used only if it is still exactly before the caret, and
nothing is typed otherwise (`event=hotkey source=remembered`). Applying settings or
switching the automatic mode in between keeps it; changed keyboard layouts drop it
(`source=remembered result=stale`). A paste the field
did not confirm ends with `event=conversion result=cancelled`, and its selection
line says what the field showed (`confirm=` and `attempts=`).
Changing automatic mode clears stale queued state without suppressing the
first new word; only a real queue overflow keeps suppression until a boundary.

Session identity uses the stable top-level window and process, while the
focused input thread is tracked separately for HKL and focused-child routing.
Modifier keys held across a focus change or a new input epoch keep counting: the
worker takes their physical state from the next key instead of assuming them
released.
Ordinary focus changes and mouse clicks re-check privacy on the first printable
key without suppressing that first word. Esc, the function keys and the menu key,
which can move the focus inside a window, do the same. A field check that was still
running when such a key, Tab, Enter, a click or a shortcut arrived never decides
the field after it. A new foreground window always has its program read again, so
a program that reuses the process ID of a closed one does not inherit its verdict.

The focused field of the foreground window is also checked periodically: every
half second while checks are fast, and when a check is slow, after twenty times
its length (at most five seconds), so that periodic checks fill at most a
twentieth of the worker's time. A full-screen program whose check is slow, which
is how most games answer, is checked once per input target and then left alone;
a new input target, and a click or a change of field on the next typed key, are
still checked. The log records `event=foreground full_screen=true|false` when
that state changes.

## Privacy boundary

The process retains at most the current word in volatile memory, plus the
physical keys of a word that is being converted by hand (at most 64 key scan
codes, kept only until the next word boundary or reset), plus, for Pause, the
physical keys of the last word of at most eight windows for at most two minutes
after a window switch, a click or a shortcut ended tracking there. While the correction
gate is armed after a Space, up to 256 key events are held in memory for at
most 1.2 seconds and then replayed unchanged. If the gate gives up on its time
limit, a full buffer, a full queue or a hook reinstall, the held keys are handed
back to the same field unless the privacy check finds a password field or an
excluded or elevated program; a field that merely cannot be inspected at that
moment (a hung or busy window) does not keep them back. Whenever a gate gives up,
the key-ups of keys that were pressed before it armed are delivered at once: they
type nothing, and holding them back would leave Shift, Ctrl or Space pressed; a gate
that held nothing else keeps nothing back and does not pause automatic conversion.
After an internal fault the held keys are replayed unchanged to the same field.
Held keys that are not handed back (the gate ended at a click, physical or injected
by another program, a focus change, injected input, a failed submission or an
internal fault after the field changed, or the hand-back was refused) wait in memory
for an explicit recovery from the tray menu for at most one minute, or one
minute from preparing that recovery, and are then deleted. None of this is
written to logs, configuration, diagnostics, or network services. Unsupported
input, editing, navigation, focus changes, layout changes, and queue overflow
clear or suppress the current context.

For the protected-paste backend, the replacement is placed on the global
clipboard only for the bounded paste transaction. The item opts out of Windows
clipboard monitoring, local history, and cloud upload. The prior clipboard is
materialized by a short-lived STA broker into local clipboard-compatible
`STGMEDIUM` values before replacement. The broker owns its hidden clipboard
window, pumps OLE messages, and restores those handles only while the temporary
sequence and owner are unchanged. This preserves delayed, private, GDI, and
enterprise-protection formats instead of discarding them. The clipboard is
replaced only if it did not change since it was copied (checked with the
clipboard open, before it is emptied), so content another program put there
meanwhile is never lost; an emptied clipboard whose replacement could not be
completed is put back at once, and only then. The restored item carries the
history, cloud and monitoring opt-outs it lacked, so it does not come back as a
new copy in clipboard history. Password, excluded,
elevated, non-materializable clipboard, and ambiguous UIA contexts fail closed.
A clipboard that cannot be taken in time (it belongs to a window that does not
answer) leaves the word alone: an automatic conversion waits 0.2 seconds for it,
Pause 0.5 seconds, and neither counts it as a failed edit; a broker that took too
long does not put the replacement there any more, no new paste starts while it
still runs, and how it ended is logged (`phase=broker_end`).

The replacement's text is rendered only when a program asks for it, so the broker
knows which process read it. The user's clipboard is put back once the field
shows the replacement. When the Ctrl+V was sent but the field did not show the
replacement in time (a busy program), the replacement stays on the clipboard
until the paste's target process has read it, and the user's clipboard comes back
half a second after that (the read may be the program's own look at the new
clipboard, with its paste right behind it); a target that has not read it within
5 seconds leaves the replacement there. Only the first program to read the
replacement is seen, so when another program reads it first, the replacement
stays as well. A paste handled late therefore inserts the replacement rather than
the user's previous clipboard (a password, say), unless the program read the
clipboard on its own and handles the paste more than half a second later. Every
wait of a paste is fitted
into the time the correction gate has left: the selection, the clipboard and the
Ctrl+V are skipped when what follows them would not fit (`phase=budget`), and a
Pause press may then wait less than 0.5 seconds for the clipboard.
Third-party local clipboard readers remain outside the confidentiality
guarantees of the Win32 clipboard.

Settings and explicitly managed lists are stored atomically in the versioned,
human-readable `%LOCALAPPDATA%\AutoKeyboardLayot\config.ini`. On a profile that
does not yet have this file, the previous `settings.ini`, `user_dictionary.txt`,
`word_exclusions.txt`, and `exclusions.txt` files are imported in memory and
migrated on the first explicit Save/Apply. The legacy files are not deleted, and
an error in one of them names that file. The pre-migration backup is written to a
temporary file and appears whole or not at all.
GUI and tray writes share a cross-process named Windows mutex; a word added from
the tray is saved on a thread of its own, so the keyboard hook never waits for the
lock or the disk. When the configuration has changed since the window was opened
(a word added from the tray, a hand edit), Apply merges the draft into it: what the
window changed wins, everything else is kept, and list entries added on either
side stay. A value changed differently in both places is reported; the window then
shows the current settings with the user's values for those, and Apply saves them.
The settings application itself is excluded from conversion.

Dictionary and word-exclusion entries take the form `en-US: word`, `ru-RU: слово`,
or `et-EE: tere`; the colon is required, as a first word would otherwise be taken
for a language. The GUI validates, normalizes, deduplicates, and rejects a word
present in both lists, naming the page and the editor line of an entry it cannot
read; after saving it says when an entry's language is not installed. Words are
limited to 64 characters after lowercasing. Program exclusions are normalized to
executable names; a name that starts with `#` or is in square brackets is refused,
as it would read back as a comment or a section. Every save checks that the file
reads back unchanged. A setting given twice is refused with its line, and a byte
order mark at the start of a file is accepted.
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

A field check that fails because the focused window answers late (Windows
Terminal while you type quickly) does not by itself make the word being typed
manual-only. When the same window, focus and input epoch passed a check less than
two seconds earlier, and nothing that can move the focus has happened since (Tab,
Enter, a click, a shortcut, injected input or a focus change), one failed field
check keeps that earlier verdict. The keyboard layout is not part of the target:
the answer does not depend on it, and a conversion switches it. A failure never
renews the good check, so a window that keeps failing counts as failing after two
seconds. A password field,
an excluded or unreadable process, a changed context and every transport failure
stand as before, and recovery of held keys and manual conversion always take a
failure at face value. A word that did meet a standing failure stays manual-only
even when the next check passes.

Capability diagnostics expose only backend states such as `uia-paste`,
`physical-replay`, `capability-probe`, `unsupported`, or `text-mismatch`; they
never include the typed source or replacement.

`diagnostics_enabled` is off by default. When explicitly enabled, a dedicated
bounded queue writes `%LOCALAPPDATA%\AutoKeyboardLayot\diagnostics.log`. The
log contains timestamps, process basename and PID, backend stages, language
IDs, word lengths, gate results, edit outcomes, reset and discard categories,
the reasons that blocked a shutdown, and one result for every typed word that was
not converted (`event=word result=` `no-candidate`, `manual-only`,
`suppressed`, `no-language`, or `not-attempted` for a candidate with `reason=`
`boundary` when Tab or Enter ended it, `automatic-off`, `shift`, `caps-lock` or
`keys`), never the word. While privacy blocks the field (a
password field, or an excluded, elevated or uninspectable program or field),
nothing about the typing there is written at all: no key, word, candidate, Pause,
queue or field-check line, and switching to a blocked program leaves no line
either, because even a counter or a timestamp per key would tell a secret's length
and rhythm. A suppressed word also names what
cancelled it (`reset=` `unsupported`, `layout`, `shortcut`, `mouse`, `focus`,
`backspace`, `delete`, `navigation` or `external-input`), and an unsupported key is
logged by class only (`class=no-character` for a key that prints nothing,
`class=other-character` for a digit or symbol a word cannot contain). A shortcut
that ends a word is logged by its editing command only (`reason=shortcut kind=`
`erase-word`, `paste`, `copy`, `cut`, `undo`, `ctrl`, `alt`, `ctrl-alt` or `win`).
Privacy
check lines name what asked for them (`trigger=`) and say when a late answer kept
an earlier verdict (`source=recent-ok age_ms=`). It never records key codes, scan
codes, characters, words, clipboard contents, window titles, URLs, file names,
or message text; the test `diagnostics_never_log_key_identity` checks every
diagnostic format string and the functions that build line details, and rejects a
text value passed other than as a count. The file rotates at 1 MiB and has no
network transport. When the full log cannot be moved aside (a viewer holds it open
without allowing that, say), records still go into it up to 2 MiB, and then it
starts again. Records that could not be written, records dropped from a full queue
and failed rotations are counted; while that count is not zero the tray tooltip
shows `log errors: N`. Earlier builds logged the virtual-key code of unsupported keys
(including digits) as `vk=`; delete `diagnostics*.log` files written by them.

`crash.log` in the same folder is always written, independent of
`diagnostics_enabled`: one line per panic with the time in Unix milliseconds,
the thread name, the source location and the application version, for example
`1700000000123 panic thread=unnamed at src/windows_agent.rs:123:9 version=0.1.0 (abc1234)`.
It never contains the panic message, typed text or key codes. The file is
capped at 256 KiB and rotates to `crash.1.log`.

A panic in a keyboard or mouse hook callback, the window procedure or the input
worker is contained. Typing is not affected: the event is passed on untouched,
keys held by the correction gate are released unchanged (or kept for recovery when
the input field changed meanwhile), automatic conversion is paused and a tray notice
points to `crash.log`. A configuration reload whose application panicked is
rejected with a notice, and the previous settings stay in force. A reload that the
worker cannot take at that moment (a correction gate or retained keys, two reloads it
has not acknowledged yet, a full queue) is kept and handed over on a later timer tick,
and a newer save replaces it; earlier builds reported it as an unreadable configuration
and dropped it. Only a configuration that cannot be read is reported as a read error;
one the agent can never apply (its worker stopped, or it would take back a
configuration file or a package generation) is reported as such. If the worker
panics three times within a minute it stops, which is reported once and turned into
a prompt when Exit is chosen; retained input can then only be deleted, not
recovered. A tray Exit that is declined (an active correction gate,
retained input, a busy worker, a pending configuration, a hotkey in progress)
now asks whether to exit anyway and names those reasons; it warns that
keystrokes held back for recovery are lost. Retained input deletes itself after
its minute, so it does not block Exit for longer. A worker thread that has
already finished never blocks Exit, not even with a configuration reload it can no
longer acknowledge. Installer-driven closes never force an exit.

A Space that arms the correction gate is always released once the worker has
handled it, whichever path it took (stale input epoch, pending configuration,
automatic conversion switched off, a pending conversion, a shortcut modifier),
so held keys no longer wait for the 1.2 second deadline that aborts the gate.
When a gate timeout does happen and its held keys are handed back in full,
automatic conversion resumes by itself; so it does when held keys that nobody
recovered are deleted after their minute, which a tray notice reports. Either
way it resumes at most three times an hour, and never when an automatic edit
failed because of the same abort. A failed or partial recovery waits for that
minute; an explicit recovery or any other pause stays paused. Keys that are being
replayed when their minute ends are not deleted under the replay. The replayed keys
bypass the hooks, so the word at the caret is not converted until the next word
boundary. The optional diagnostics log records `phase=armed`, `phase=abort` (with
counts of `returned` key-ups and `replayed` keys), `phase=returned`,
`phase=release_forced`, `phase=recovery_discarded` (`reason=expired` after the
minute), `phase=auto_resumed`, `phase=auto_resume_declined`, `event=ui_stall`
(timer ticks more than 700 ms apart) and `event=hook_slow` (a hook callback over
50 ms).

The protected-paste backend selects the word it is about to replace. If the
selection cannot be confirmed to hold exactly that word, or the paste does not
complete, the selection is collapsed to its end instead of being left behind to
be typed over; a selection the user made is never touched. A selection call that
reports failure is collapsed too, as the field may have applied it, and a second
Pause attempt collapses the first attempt's selection when it shows up late. The
collapse is read back: when the field does not show the selection gone within
200 ms (a busy or hung field), the keys typed after the Space are kept for
recovery instead of being replayed over the word, automatic conversion pauses
(`selection-left`), and the log records `phase=cleanup result=Unverified` and
`cleanup=` in the selection report. A paste abandoned after the user acted (new
input, a click, another field) leaves what is selected alone, as it may be the
user's own selection now, and keeps the held keys the same way. Keys kept like
this never come back automatically; only an explicit recovery brings them back.
When the text edit is
confirmed but the previous clipboard contents cannot be put back (restoring
waits up to 60 ms for the clipboard), the conversion still counts as done:
automatic conversion stays on and the diagnostics log records
`clipboard_restored=false`. A field that could not be inspected or selected is
no failed edit either. Failures after the paste name their own reason in the
tray: `text-commit` (the replacement did not show), `layout-switch` (the layout
did not switch after it) or `clipboard-edit` (the paste chord was not sent
whole). A physical replay whose keys were all sent but whose guard was lost
while waiting for the field logs `result=applied-uncertain`.

The privacy check asks Windows UI Automation about the focused field from a
dedicated provider thread. If that provider stops answering (a call that never
returns), every later check used to report "queue busy" and privacy stayed
unavailable for good. A provider that keeps the queue busy for 5 seconds is now
abandoned and replaced by a fresh one on a new thread, at most three times; the
diagnostics log records `event=probe respawned=N`, and the hung thread is never
joined. The text barrier's reads (whether the text before the caret is the word an
edit is about to replace) also run on a provider thread of their own: the worker waits
for an answer no longer than the barrier's deadline, at most 150 ms per read, a read left
behind changes nothing in the field, the barrier report then names `stage=Provider`, and a
provider that keeps the queue busy for 2 seconds is replaced, at most three times.
Selecting the word and collapsing that selection again run on the worker itself: they
change the field, and a call abandoned at a deadline could still take effect later, under
keys typed meanwhile. When the
worker is still busy two seconds after a gate broke for a cause that hands its keys
back without being asked (its deadline, a full gate, a full input queue or a hook
reinstall), the UI thread hands the keys back itself, provided the input target is
the same, nothing was typed or clicked since and no modifier key is held, no
password field or excluded or elevated program was found, and the worker is not
inside an edit that may have selected or typed text (`phase=recovery_handed_back`).

The tray icon's tooltip shows the mode (or the safety pause and its reason), the
layout code and the counters. The icon uses version 4 of the notification-area protocol,
under which the shell shows the standard tooltip only when every call asks for it; earlier
builds did not ask, so hovering over the icon showed nothing.

The keyboard and mouse hooks are kept alive. When Explorer restarts (`TaskbarCreated`)
the tray icon is added again, showing what the old icon showed, so a conversion failure
it already announced is not announced again; if Explorer is not ready yet the addition is
retried with a growing delay. The same holds at start-up: an agent that starts with the
session before the taskbar exists works without the icon and adds it once the taskbar is
there (`event=tray` in the diagnostics log), where earlier builds stopped with a start-up
error. Unlocking the session, reconnecting and resuming
from sleep reinstall both hooks (held keys are handed back first, as after a gate
timeout). A
watchdog on the 250 ms timer also reinstalls them when a hook misses input it must see
for more than 3 seconds while the user gives input (within the last 400 ms): the mouse
hook while the pointer keeps moving (on two ticks in a row, so a program placing the
pointer does not count), the keyboard hook when input comes while the pointer stands
still and the mouse hook saw none of it either (a click or a scroll it saw is no
typing). Each hook has its own record of callbacks, so typing no longer hides a mouse
hook that Windows removed. It does
not act on the lock screen or the UAC desktop, when the foreground window has no
known or a higher integrity level (its input may legitimately never reach the
hooks), or when nothing is in the foreground; it acts at most once per 15 seconds
and four times an hour, after which a tray notice says the hooks are unstable. It
also leaves the hooks alone while a full-screen program (most games), a VMware,
VirtualBox or Hyper-V console or a Remote Desktop window is in the foreground:
these take the keyboard with a hook of their own, so silence proves nothing there,
and reinstalling only fought them. The hooks are judged again once another window
is in the foreground.
A failed installation is retried with backoff (1 s up to 30 s) and reported after
the third failure. Tray updates are limited to two per second and wait while the
correction gate holds keys, because the shell call can block on an unresponsive
Explorer while the hooks need the UI thread; a skipped update is tried again on a
later tick. The diagnostics log records `event=hooks` with the reason
(`silent-keyboard` or `silent-mouse` for the watchdog).

## Language packs

Which packs take part in conversion is decided at run time from the enabled
packages and the keyboard layouts Windows has loaded; there is no static
capability table. Dictionary sources, checksums, and exact redistribution
notices live under `data/language-packs/`.

- `en-US`: direct layout, automatic correction; its dictionary is built in;
- `ru-RU`: direct layout, automatic correction;
- `et-EE`: direct layout, automatic correction, mapped from the buffered
  physical scan codes through the installed Windows Estonian HKL;
- `ja-JP`: no automatic correction; it awaits an adapter that can observe and
  preserve Windows IME composition.

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
user profile. An entry without an extension (`Signal`, `KeePass`) matches the
program of that name whatever its extension.

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
planned language set with every current message key, including the input-package
status messages. Strict completeness and placeholder validation
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

From WSL, `tools/verify-wsl.sh` runs the whole local gate with one command:
formatting, Clippy (default features, `installer-tools` without default
features, the Windows target, the signing tools on both systems, and the Windows
installer helper), tests in two feature configurations plus the signing tools,
strict locale validation, the Python unit tests, and every Windows test executable
(the agent's, the library's, the integration tests and the signing utility's, in
the default and in the modular configuration), which are cross-built with
`cargo-xwin` and executed on the Windows host with a temporary `LOCALAPPDATA`. Each step logs to
`target/verify/<timestamp>/`, the run ends with a `summary.txt`, and the test
counts may not fall below recorded floors. `--keep-going` runs every step after a
failure; `--list` and `--only` show and select steps.

The CI workflow (`.github/workflows/checks.yml`) runs the same checks on Linux
and Windows. On Linux it also checks dependency advisories with
`cargo audit --deny warnings` (the four accepted notices are listed with their
reasons in `.cargo/audit.toml`) and dependency licenses and sources with
`cargo deny check licenses bans sources` (policy in `deny.toml`). A new license
type or a git dependency fails the check until someone decides on it.

The settings window and About page show the version as `0.1.0 (abc1234)`, where
`abc1234` is the source commit. A build whose tracked source files (`src`,
`data`, `ui`, `assets`, `Cargo.toml`, `Cargo.lock`, `build.rs`) differ from that
commit shows `abc1234-dirty`; builds outside a Git checkout can set
`AKL_BUILD_COMMIT` and otherwise show `unknown`.

`tools/deploy-host.ps1` installs a verified build over the per-user installation
with a backup, a graceful close and a rollback on failure; `-WhatIf` only prints
the plan. See [`docs/host-deployment.md`](docs/host-deployment.md).

`tools/verify-windows.ps1` runs native format/tests/Clippy/release checks, using
an isolated temporary profile for adapter tests. `tools/start-windows-build.ps1`
launches that script independently of SSH through WMI and monitors its result.
Each run requires a fresh log directory; per-stage logs and `result.json`
remain there even if the monitoring connection is interrupted. The build retries
replacing `result.json` while the monitor holds it open, and the monitor reads a
receipt it could not read again on its next check. `tools/test-build-scripts.ps1`
checks these scripts and `tools/run-installer-boundary-tests.ps1` (which reads a
test's output while it runs) on a Windows machine with stand-ins for cargo and
for a test executable.

## Security

Report a suspected vulnerability privately; [`SECURITY.md`](SECURITY.md) says how.
[`docs/threat-model.md`](docs/threat-model.md) lists what the program defends against and what it
does not (code running as the same Windows user is out of scope), and
[`docs/repository-settings.md`](docs/repository-settings.md) lists the GitHub settings that the
owner switches on.

## License and third-party notices

The project's own code, tools, documentation and translation drafts are released
under the MIT License ([`LICENSE`](LICENSE)). Third-party material keeps its own
terms:

- Dictionary sources and their redistribution notices are in
  `data/language-packs/*/LICENSE*`.
- The layout model's data sources and its CC BY-SA 4.0 license are in
  `data/layout-model/NOTICE.md`.
- Dependency versions are pinned by `Cargo.lock`; upstream dependency licenses
  remain applicable. `tools/collect_dependency_notices.py` gathers their texts,
  together with the notices of the bundled English dictionary and the layout
  model, into the installer's `THIRD-PARTY-NOTICES.txt`.
- The settings UI uses Slint, declared by its crates as
  `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0`.
  Which option applies to the distributed executable is an open item.

Open licensing questions (the Estonian LGPL dictionary in signed packages, the
ShareAlike model, Slint, the application icon) are listed in
[`docs/licensing-notes.md`](docs/licensing-notes.md), which records facts and
questions, not legal advice.
