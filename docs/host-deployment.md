# Deploying a build to a Windows host

`tools/deploy-host.ps1` replaces the per-user installation
(`%LOCALAPPDATA%\Programs\AutoKeyboardLayot\AutoKeyboardLayot.exe`) with a build whose SHA-256 you
have recorded. It asks the running agent to close gracefully and stops without changes when it
declines, keeps a backup, verifies the result and restores the backup when anything fails. It is meant
for development installs from a checkout; the installer remains the way to install for other users.

## Build

From WSL, cross-build the modular base, which matches what the installer ships:

```sh
CARGO_ENCODED_RUSTFLAGS="$(python3 tools/release_rustflags.py)" \
  cargo xwin build --release --target x86_64-pc-windows-msvc --no-default-features \
  --bin AutoKeyboardLayot --target-dir target/xwin-hotkeys
sha256sum target/xwin-hotkeys/x86_64-pc-windows-msvc/release/AutoKeyboardLayot.exe
```

`tools/release_rustflags.py` adds path remapping to the flags in `.cargo/config.toml` (the static C
runtime), so the executable records no path of the build machine: no account name, checkout or
Cargo cache. The installer build (`tools/build_experimental_installer.py`) uses the same flags and
refuses executables that still contain such a path.

Commit first: the version shown in Settings contains the commit id, and a build from uncommitted
changes shows `-dirty` after it.

## Dry run

`-WhatIf` performs only read-only checks, prints the plan and changes nothing. The candidate is not
started. Paths are Windows paths; from WSL convert them with `wslpath -w`.

```powershell
powershell.exe -NoProfile -File tools\deploy-host.ps1 `
  -Source <candidate.exe> -ExpectedSha256 <64 hex digits> -BackupSuffix <name> -WhatIf
```

## Deploy

Run the same command without `-WhatIf` (`-Confirm` asks once, before the first change; nothing after
that asks again, so no answer can skip the backup or a step of the rollback). `-BackupSuffix` names the backup
`AutoKeyboardLayot.exe.before-<BackupSuffix>` and must not exist yet. `-KeepBackups` (default 3) is how
many `AutoKeyboardLayot.exe.before-*` files remain afterwards, counting the new one; older ones are
deleted, newest first by creation time, and only regular files with that exact name pattern.

The script stops before changing anything when:

- the candidate's SHA-256 differs from `-ExpectedSha256`;
- there is no installed executable, or the backup name is already taken;
- more than one agent process exists (an open Settings window counts), the process does not run from
  the installed path, an application window is open, or the running agent does not advertise graceful
  close;
- the candidate's `--verify-profile` preflight does not exit with 0 within 15 seconds.

After the preflight it closes the agent with `WM_CLOSE` and waits at most 10 seconds. An agent that
does not close in time keeps running and nothing is changed: the agent declines to exit while it holds
typed input or is busy (with diagnostics on, its log says `event=gate phase=shutdown result=declined
blockers=...`), and stopping it by force would lose that input. Run the script again later; held
keys that nobody recovers are deleted after one minute. An agent that accepted the close but needs
longer (it waits for its worker) is watched for `-CloseGraceSeconds` more (default 60): when it exits
then, the script starts it again from the unchanged executable, replaces nothing and says so.

Then it copies the backup, replaces the executable, verifies the installed hash, starts the new agent,
waits five seconds and checks that exactly one agent process runs and its observer window exists. On
any failure from the backup on, it stops the new agent (if it is still running) and waits for it to
exit (the executable stays locked until then), restores the backup when the executable was already
replaced, and restarts the previous agent when one was running. A restore that fails (a scanner holding
the file, for example) is retried for about ten seconds and checked against the backup's hash; if it
still fails, no agent is started and the error names the original failure and the backup to copy back
by hand (see below).

The last lines are a receipt: `DEPLOYED_PID`, `EXE_SHA256`, `BACKUP`, and the number of pruned and kept
backups.

## Acceptance on a test machine

`tools/test-deploy-host.ps1 -Script <deploy-host.ps1> -OutputDirectory <new directory>` runs the script
against stand-ins compiled on the spot (an agent with the real observer window and graceful-close
property), each scenario in a LOCALAPPDATA of its own, so the machine's real installation is never
touched: a successful deployment; the installed file held for six seconds (copying the candidate
fails, the restore waits, the previous agent runs again); held for forty seconds (the error names the
original failure and the backup, no agent is started); an agent that closes twenty seconds after
`WM_CLOSE` (it runs again from the unchanged executable); `-Confirm` answered Yes and then No to
anything else, and answered No. Run it on a test machine where no AutoKeyboardLayot process runs.

## Rolling back by hand

Close the agent from its tray menu, copy the wanted `AutoKeyboardLayot.exe.before-*` file over
`AutoKeyboardLayot.exe`, and start the executable again.
