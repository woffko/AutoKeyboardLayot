# AutoKeyboardLayot on Linux

On Linux, AutoKeyboardLayot is an IBus input method. While its input source is active, it types
English, Russian and Estonian itself and replaces a word typed in the wrong layout when the word
ends. It shares the detector, the dictionaries, the settings and the language packages with the
Windows agent; [`linux-port-plan.md`](linux-port-plan.md) explains the design.

The port targets GNOME (Wayland or X11) with IBus, the Ubuntu default. Other desktops work when
IBus is their input method framework and the engine is installed from the `.deb`.

## Install

Both ways build the modular base, as the Windows installer does. The program contains English and
the layout model. Russian and Estonian come as signed language packages that you download in the
settings window (see [First start](#first-start)). The Estonian dictionary is under the LGPL, and
the project ships it only in signed packages ([`licensing-notes.md`](licensing-notes.md)).

### For one user, from the source tree

```sh
python3 tools/linux_package.py install-user
```

This builds the release program and installs:

| File | Purpose |
|---|---|
| `~/.local/bin/autokeyboardlayot` | the program |
| `~/.local/share/ibus/component/autokeyboardlayot.xml` | tells IBus how to start the engine |
| `~/.config/systemd/user/org.freedesktop.IBus.session.GNOME.service.d/autokeyboardlayot.conf` | sets `IBUS_COMPONENT_PATH` for GNOME's IBus, which has no per-user component directory |
| `~/.local/share/applications/autokeyboardlayot.desktop` and the icon | "AutoKeyboardLayot" in the application list opens the settings |

The installer then reloads the user's systemd units and rewrites the user's IBus engine cache
(`ibus write-cache`). Without that step, an IBus with an existing cache does not look in the new
directory. The installer does not restart IBus or change input sources. It prints those steps, and
they are described below. `--binary PATH` installs an already built program. The user installation
needs GNOME's systemd-managed IBus; on other desktops, use the `.deb`.

### As a Debian package

The package is built from reviewed third-party notices, like the Windows installer:

```sh
python3 tools/linux_package.py receipt target/linux-receipt-DATE.json
python3 tools/collect_dependency_notices.py target/linux-notices-DATE \
  --platform x86_64-unknown-linux-gnu --build-receipt target/linux-receipt-DATE.json
# When texts are missing, fetch them from the recorded upstream revisions and collect again:
python3 tools/fetch_dependency_notices.py target/linux-notices-DATE/notices-report.json target/linux-upstream-DATE
python3 tools/collect_dependency_notices.py target/linux-notices-complete-DATE \
  --platform x86_64-unknown-linux-gnu --build-receipt target/linux-receipt-DATE.json \
  --upstream target/linux-upstream-DATE/upstream-notices.json
# Review THIRD-PARTY-NOTICES.txt, then build with its SHA-256:
python3 tools/linux_package.py build --output target/linux-package-DATE \
  --notice-directory target/linux-notices-complete-DATE \
  --build-receipt target/linux-receipt-DATE.json --notice-sha256 REVIEWED_SHA256
sudo apt install ./target/linux-package-DATE/autokeyboardlayot_0.1.1-1_amd64.deb
```

`build` refuses notices that are incomplete, stale, unreviewed or collected from another receipt.
It checks that every package Cargo compiled is in the receipt. It then reads the package back
(fields, exact file list, extracted files, the extracted program's version) and records everything
in `build.json`. The package installs:

- `/usr/bin/autokeyboardlayot`;
- `/usr/share/ibus/component/autokeyboardlayot.xml`;
- the desktop entry and the icon;
- `copyright` and `THIRD-PARTY-NOTICES.txt` in `/usr/share/doc/autokeyboardlayot`.

It has no maintainer scripts: IBus sees the new file in its component directory the next time it
starts. The package depends on the C library of the system it was built on, so build it on the
oldest distribution release it has to support.

## Turn it on

1. Restart IBus so it reads the engine: `systemctl --user restart
   org.freedesktop.IBus.session.GNOME.service` on GNOME, `ibus restart` elsewhere, or log out and
   back in.
2. Add the input source: Settings → Keyboard → Input Sources → Add Input Source → Other →
   AutoKeyboardLayot. The same from a terminal, replacing the whole list:
   `gsettings set org.gnome.desktop.input-sources sources "[('ibus', 'autokeyboardlayot')]"`.
3. Remove the Alt+Shift option `grp:alt_shift_toggle` from Settings → Keyboard (or from
   `org.gnome.desktop.input-sources xkb-options`). The engine switches with Alt+Shift itself, but
   GNOME turns the key of that option into "next input source", which would leave the engine. The
   engine types Russian and Estonian itself (with the XKB layouts `ru` and `ee`), so the other input
   sources are not needed. They can stay as a fallback for programs that do not take input methods
   (Super+Space switches to them).

The panel indicator shows `EN`, `RU` or `ET`. Its menu has the keyboard layouts, an "Automatic
conversion" switch and "Settings…".

## First start

The first time the engine or the settings window starts for a user, it creates an English-only
profile. On Windows the installer does this. An existing profile is never rewritten. Open the
settings (the indicator menu, the application list, or `autokeyboardlayot --settings`) and download
Russian and Estonian on the Input languages page. The packages come from the project's GitHub
releases and are installed only when their release signature checks out. A downloaded `.aklp` file
can also be imported from disk.

A profile written by a development build with the bundled languages (`cargo run`, default features)
selects Russian and Estonian without packages. The modular base refuses such a profile rather than
rewriting it, and the engine exits with "migrate selected legacy input packs before installing the
modular base". Move `~/.config/autokeyboardlayot/config.ini` aside to start fresh.

## Use

- Type. When a word typed in the wrong layout ends with Space, the engine replaces it and switches
  to the layout of the replacement.
- Alt+Shift switches EN → RU → ET.
- Pause (the hotkey is configurable in Settings) undoes the last conversion. Without one, it
  converts the word at the caret, or the last word, to the most plausible other layout. Pressing it
  again tries the next one.
- A setting changed in the settings window takes effect when the next text field takes the focus.

How a word is replaced: if the application reports the text around the caret and that text ends
with the typed word, the engine deletes exactly that word through IBus and commits the replacement.
Otherwise, in terminals and in applications that do not report the text, it sends one BackSpace per
typed character and then commits the replacement. Inside a terminal program that gives BackSpace
another meaning, switch automatic conversion off.

Password and PIN fields, as the application reports them to IBus, and fields marked private are
typed normally but never tracked.

The settings window on Linux leaves out the Windows-only pages and options: excluded applications,
replacement methods, the physical-replay fallback and the terminal UI Automation fallback. IBus
does not tell the engine which program a field belongs to, and one IBus path serves every program.

## Privacy

The same boundary as on Windows (see the README's privacy section):

- The current word is kept in memory only. For Pause, the last word and the last conversion are
  kept in memory until the field loses the focus or the application resets its input (a click, a
  caret move).
- Nothing typed is written to disk. Diagnostics are off by default. When switched on, they record
  stages and counters, never characters, words or key codes; `tests/diagnostics_privacy.rs` checks
  the Linux sources for this too.
- The network is used only when you download packages in the settings window. Downloads go through
  `curl`: HTTPS only, TLS 1.2 or newer, only `github.com` and `release-assets.githubusercontent.com`,
  redirects followed by the program (at most three), size limits and a deadline.

| What | Where |
|---|---|
| Settings, user dictionary, word exclusions | `~/.config/autokeyboardlayot/config.ini` |
| Language packages | `~/.local/share/autokeyboardlayot/packages/` |
| Diagnostics log (when on), crash log | `~/.local/state/autokeyboardlayot/` |

[`threat-model.md`](threat-model.md) has a Linux section.

## Troubleshooting

- `ibus list-engine | grep autokeyboardlayot` shows whether IBus knows the engine. After a user
  installation, IBus must be restarted first.
- The engine writes its errors to the log of IBus:
  `journalctl --user -u org.freedesktop.IBus.session.GNOME.service`.
- Settings → Diagnostics switches on the diagnostics log
  (`~/.local/state/autokeyboardlayot/diagnostics.log`, rotated at 1 MiB). A crash leaves
  `crash.log` next to it.
- `autokeyboardlayot --version` prints the version and the source commit.

## Remove

- Package: remove the input source in Settings → Keyboard, then `sudo apt remove autokeyboardlayot`.
- User installation: remove the input source, then `python3 tools/linux_package.py uninstall-user`.
  It removes the files it installed and rewrites the IBus engine cache without the user's component
  directory. Restart IBus or log out to finish.

Both keep the profile. To delete it: `rm -r ~/.config/autokeyboardlayot
~/.local/share/autokeyboardlayot ~/.local/state/autokeyboardlayot`.

## Development

- `tools/verify-linux.sh` runs the local gate in eleven logged steps: formatting, Clippy in four
  configurations, tests in three, the locale catalogs, the Python tests, and Clippy for the Windows
  target when its standard library is installed.
- The end-to-end tests start a private `ibus-daemon` with an empty environment, its own address and
  its own directories. They never reach the desktop's IBus.
- `cargo run -- --register` registers a development engine with the running IBus of the desktop,
  for development only.
