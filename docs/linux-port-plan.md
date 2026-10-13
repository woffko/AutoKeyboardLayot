# Linux port plan

Status: L0 to L9 done on branch `linux-port` (from `2408f57`), not pushed; L10 in progress on the
owner's desktop (user installation since 2026-10-13).
Owner decisions of 2026-10-13: an IBus engine, the whole port in one plan, the Windows agent kept
with a shared core. How to install and use it: [`linux.md`](linux.md).

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
can stay configured as a fallback (Super+Space), but the XKB option `grp:alt_shift_toggle` has to go:
GNOME turns its key into "next input source", which would leave the engine.

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
| `profile` | the configuration writers' lock and the English-only profile the modular base creates on its first start (on Windows the installer creates it) |
| `runtime` | configuration and installed packages to a detector, reload, diagnostics log, crash log |
| `keymap` | libxkbcommon keymaps per enabled language, keycode and modifiers to text, replay of a word's keys in another layout |
| `engine` | the per-field state machine (pure, no D-Bus): layout, word, boundary conversion, Pause with cycle and undo, Alt+Shift, privacy, diagnostics |
| `ibus` | zbus connection to the IBus bus, component, factory and engine objects, IBus text and property serialization, the `InputMode` indicator and its menu (layouts, automatic conversion, settings) |
| `localization` | the display language of the menu and the settings window from the shared catalogs |
| `settings` | the Slint settings window for Linux (Windows-only sections hidden) with the package pages shared with Windows; the engine reloads when `config.ini` changes |
| `diagnostics` | the optional diagnostics log on a writer thread |

Package downloads use `curl` (`src/package_download/curl.rs`) with the policy of the Windows
transport and are verified by the shared pipeline. `tools/linux_package.py` builds the `.deb` and
installs for one user.

### Packaging

The `.deb` and the user installation are the modular base, as the Windows installer is: English and
the layout model are compiled in, Russian and Estonian come as signed packages, because the
Estonian dictionary ships only that way ([`licensing-notes.md`](licensing-notes.md)). No installer
runs per user on Linux, so the engine and the settings window create the English-only profile when
they first start, with the rules of the Windows installer (never rewrite, never reset a partial
store). IBus has no per-user component directory: the user installation adds a drop-in for GNOME's
IBus service that sets `IBUS_COMPONENT_PATH`, and rewrites the user's IBus engine cache, because an
IBus with an existing cache does not look in a directory that cache does not list (checked against a
private `ibus-daemon`). A file added to a directory the cache lists is found at the next start, so
the `.deb` needs no maintainer scripts.

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

Progress: L0 `d4f7335`; L1 to L5 `d4f7335` and `e3b24f7`; L6 `e3b24f7`; L7 `99a492e`; L8 `d687220`
and `a94601f`; L9 `ed0e8be`. `bafd0ef` fixed the start by IBus itself (the engine lost the
factory's first call), which the tests had not covered; an end-to-end test covers it now. The
package `target/linux-package-inspection-20261013-02` (program `bafd0ef`) was built, read back and
extracted, and a private `ibus-daemon` started the extracted program from its component file: it
created the English-only profile in an empty home and served the engine. The settings window has
not been opened on a display yet (L10), and CI runs when the branch is pushed. The Linux notices of
that package (`target/linux-notices-complete-20261013-01`, 421 packages, none missing) are not yet
reviewed by the owner.

L10 started on 2026-10-13 with the user installation of `dfa67c7`. The journal of the desktop's IBus
showed two GLib warnings when IBus created the engine: the engine answered the flags `FocusId` and
`ActiveSurroundingText` as `(b)`, the type the IBus introspection declares, but `ibus-daemon` reads
both as plain booleans and took them as false. The engine now answers plain booleans, and the
end-to-end tests fail when the private daemon logs a warning.

The owner's first typing tests (Ptyxis with Claude Code, Firefox) showed the replacement overtaking
the forwarded Backspaces: `ghbdtn` and Space gave `ghbdtnп`, the six Backspaces erasing six of the
seven characters of `привет ` instead of the word. GNOME delivers forwarded keys with its next frame
and GTK takes key events with its next frame, while a commit reaches the application at once. The
engine now waits 80 ms after Backspaces before it commits; a deletion through the surrounding text
needs no wait. All conversions in Firefox used Backspaces too, so the conversion record now names
what the field reported (`surrounding=`). A reload of the settings recreated the engine of every
field and lost what the application had reported about it, such as a password purpose; the reload
now keeps that.

The Windows code cannot be built here (no `cargo-xwin`); shared changes are type-checked for the
Windows target when the standard library for it is installed, and CI builds and tests Windows.

## Not ported

UI Automation routes and per-program edit rules (one IBus path for every program), the correction
gate and the hook watchdog (not needed), the Inno Setup installer and its lifecycle (a `.deb`
instead), DPAPI package signing (signing stays in the owner's Windows profile; Linux only verifies),
per-program exclusions (IBus does not tell the engine which program a field belongs to).
