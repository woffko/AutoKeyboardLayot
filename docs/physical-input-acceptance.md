# Physical input acceptance checklist (manual)

This checklist is for a human operator on a reviewed Windows test machine. It is
not a release approval. Record each result in the receipt template at the end and
keep the screenshots or text captures with it.

## Current capability boundary

- Supported input today: EN/RU/ET ordinary physical keys with Shift/Caps Lock and
  one Unicode scalar per key (`physical-key-v1`). Russian and Estonian are
  conservative physical subsets.
- Not implemented: AltGr, dead keys, composition/IME, grapheme composition.
  Do not report those as working. If a test depends on them, record `blocked`.
- Only the built-in EN/RU/ET data is present. Other languages cannot be enabled
  and must not be tested as if they worked.
- UI language is independent of input readiness.

## Preparation

1. Install the reviewed candidate build on the test machine and confirm the
   installed `AutoKeyboardLayot.exe` SHA-256 matches the build receipt.
2. Start the agent. In settings, enable exactly English + one target pack
   (run the whole checklist once for RU and once for ET).
3. Confirm the target keyboard profile is present and selected; the settings page
   must still show conservative scope and any missing capability identifiers.
4. Open Notepad, Windows Terminal and one browser text field as separate targets.
5. Do not delete or rewrite existing profile data during the checklist.

## Converted typing (per target application)

For Notepad, Windows Terminal and a browser text field, repeat all rows:

| # | Step | Expected |
| - | ---- | -------- |
| 1 | Type `ghbdtn` then a space, with automatic conversion enabled | The word converts to `привет` (RU) or the ET equivalent; no stray delimiter |
| 2 | Type the reverse wrong-layout word, e.g. `руддщ` then space | Converts to `hello` |
| 3 | Trigger the manual conversion hotkey (Pause) after each word | Conversion happens once; no double edit |
| 4 | Type fast continuous alternating words without pauses | Every boundary converts; no dropped or duplicated characters |
| 5 | Press Enter, then type the next word | The new word converts after Enter |
| 6 | Navigate to the search box and type there | Conversion behaves the same; focus change does not leak the previous word |
| 7 | Click in the middle of a word and continue typing | The earlier word is not converted retroactively |
| 8 | Alternate focus between two apps mid-word | No conversion of the abandoned partial word at the new focus |

## Undo, clipboard and protected input

| # | Step | Expected |
| - | ---- | -------- |
| 9 | After a conversion, press Ctrl+Z | Exactly the conversion is undone; the original typed text returns |
| 10 | Copy a known string, perform a conversion, then paste | Clipboard content is unchanged by the conversion |
| 11 | Paste into a password field, then type there | No conversion, no inspection, no diagnostic that exposes the content |
| 12 | Type in a field the app reports as protected | The agent does not mutate or read it |
| 13 | Disable the target pack, then repeat rows 1-2 | No conversion while the pack is disabled |
| 14 | Remove the target pack, then repeat rows 1-2 | Fails closed; English-only typing continues normally |

## After the run

1. Confirm the profile, store pointer and user dictionary/exclusions are
   unchanged except for data the test intentionally created.
2. Uninstall or restore the machine to its documented baseline.
3. Keep the notes with what is not covered: AltGr/dead-key/IME/composition,
   other languages, RTL input, and any crash or freeze.

## Receipt template

```json
{
  "state": "passed | failed | partial",
  "operator": "",
  "machine": "",
  "candidate_setup_sha256": "",
  "installed_app_sha256": "",
  "target_pack": "ru-RU | et-EE",
  "agent_scope": "physical-key-v1",
  "applications": ["notepad", "terminal", "browser"],
  "converted_typing": { "1": "", "2": "", "3": "", "4": "", "5": "", "6": "", "7": "", "8": "" },
  "undo_clipboard_protected": { "9": "", "10": "", "11": "", "12": "", "13": "", "14": "" },
  "blocked_cases": [],
  "observed_failures": [],
  "screenshots": [],
  "typing_acceptance": false,
  "note": "Manual human acceptance only; not a release approval."
}
```

Each field records `pass`, `fail` or `blocked` plus a short observation. A crash,
a wrong protected-input result or any lost user data stops the run and is
reported as a failure, not a partial pass.
