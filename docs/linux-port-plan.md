# Linux port plan

Status: in progress on branch `linux-port` (from `2408f57`). Owner decisions of 2026-10-13: an
IBus engine, the whole port in one plan, the Windows agent kept with a shared core.

## Target

The owner's desktop: Ubuntu 26.10, GNOME 51 on Wayland, IBus 1.5.34, input sources `us`, `ru` and
`ee` switched with Alt+Shift (`grp:alt_shift_toggle`), per-window input sources. The design also
works wherever IBus delivers keys (GNOME and KDE on Wayland and X11), but only GNOME on Wayland is
accepted on a real desktop.

## Why an IBus engine

On Wayland an ordinary program cannot see the keyboard of other programs. The alternatives were a
keyboard proxy on `/dev/input` and `/dev/uinput`, which needs the `input` group (every program of
the user could then read every keystroke) or a root service plus a GNOME Shell extension to read and
switch the layout, or an input method engine. An IBus engine receives exactly the keys of the
focused text field of programs that take input methods, is told when that field is a password field,
and edits text through the input method protocol. It needs neither root nor extra groups, and games,
virtual machines and other programs that do not use an input method never reach it.

### One engine with its own layouts

An engine cannot switch the desktop's input source, and the Windows agent switches the layout after
each conversion. So the engine is one input source with a `us` base keymap and keeps the layouts of
the enabled languages itself: in the `us` layout keys pass through untouched; in another layout the
engine commits the character that layout gives for the key (libxkbcommon computes it from the
keycode). Alt+Shift switches the engine's layout, and the GNOME indicator shows it through the
`InputMode` property (`EN`, `RU`, `ET`). After a conversion the engine simply continues in the target
layout. Programs without input method support only get the base layout; the system `xkb` sources
can stay configured as a fallback (Super+Space).

### How an edit is made

The engine handles keys one at a time and synchronously, so no correction gate is needed: the keys
after a Space wait in the compositor while the engine decides. A word is replaced by deleting it and
committing the replacement plus the delimiter:

- when the field reports its surrounding text (GTK, Qt, browsers), the engine first checks that the
  text before the caret ends with exactly the tracked word, then sends `DeleteSurroundingText`;
- otherwise (terminals), it forwards one Backspace per character, as the Windows terminal route.

A password or PIN field (content purpose) and a field with the sensitive-data hint are passed
through and nothing is tracked.

## What is shared and what is new

Shared, unchanged: detector, layout model, dictionaries and user lexicons, input session,
conversion transactions, settings and configuration document, package catalog, store and signature
checks, localization catalogs, crash log. The Windows agent stays as it is.

New, under `src/linux_agent/` (compiled on Linux only):

| Part | Role |
|---|---|
| `paths` | XDG directories: configuration `~/.config/autokeyboardlayot`, packages `~/.local/share/autokeyboardlayot`, diagnostics `~/.local/state/autokeyboardlayot` |
| `runtime` | configuration and installed packages to a detector, reload, diagnostics log, crash log |
| `keymap` | libxkbcommon keymaps per enabled language, keycode and modifiers to text, replay of a word's keys in another layout |
| `engine` | the per-field state machine (pure, no D-Bus): layout, word, boundary conversion, Pause with cycle and undo, Alt+Shift, privacy, diagnostics |
| `ibus` | zbus connection to the IBus bus, component, factory and engine objects, IBus text and property serialization |
| `indicator` | `InputMode` property and the menu: automatic conversion, undo, settings |
| `settings` | the Slint settings window for Linux (Windows-only sections hidden) and the reload notice to the engine |
| `packages` | catalog and package download over HTTPS with `curl`, verified by the shared pipeline |

zbus, xkbcommon and the Linux side of Slint and winit are already in `Cargo.lock` (the lock file is
resolved for every target), so no new crate version enters it; they become Linux-only direct
dependencies. The Windows build compiles the same crates as before, but `Cargo.toml` changes, and
the reviewed 0.1.1 notices are bound to its hash: the Windows 0.1.1 release is built from
`audit-fixes-20260930` (or `main` after PR #1), not from this branch, or the notice pipeline runs
again.

## Phases

| Phase | Content | Done when |
|---|---|---|
| L0 | branch, Linux entry point and module skeleton, Linux gate `tools/verify-linux.sh` | the Linux binary builds, the gate passes |
| L1 | XDG paths, configuration and package loading, diagnostics and crash log | unit tests on a temporary directory |
| L2 | keymaps with libxkbcommon | tests: `us`, `ru`, `ee` keys, Shift, Caps Lock, AltGr, replay of `ghbdtn` as `привет` |
| L3 | engine state machine | tests with simulated keys: typing in each layout, conversion on Space with and without surrounding text, Pause cycle and undo, password fields, shortcuts, navigation, focus changes, Alt+Shift |
| L4 | IBus D-Bus layer | serialization tests; an end-to-end test against a private `ibus-daemon` that creates an input context, selects the engine, sends keys and checks the commits, forwarded keys and deletions |
| L5 | indicator and menu | end-to-end: properties registered and updated on a layout switch |
| L6 | settings window on Linux | the window opens, saves atomically, the engine reloads |
| L7 | language packages on Linux | catalog and package download verified against fixtures; the real catalog read-only |
| L8 | packaging: user install script, `.deb`, IBus component file, desktop entry, uninstall | package built and inspected; installs into a temporary root |
| L9 | CI job for Linux, README, `docs/linux.md`, threat model | CI green |
| L10 | acceptance on the owner's desktop | needs the owner at the moment: install, add the input source, test in Terminal, Text Editor, Firefox, a password field |

The Windows code cannot be built here (no `cargo-xwin`); shared changes are type-checked for the
Windows target when the standard library for it is installed, and CI builds and tests Windows.

## Not ported

UI Automation routes and per-program edit rules (one IBus path for every program), the correction
gate and the hook watchdog (not needed), the Inno Setup installer and its lifecycle (a `.deb`
instead), DPAPI package signing (signing stays in the owner's Windows profile; Linux only verifies),
per-program exclusions (IBus does not tell the engine which program a field belongs to).
