# Deploying a build to a Windows host

`tools/deploy-host.ps1` replaces the per-user installation
(`%LOCALAPPDATA%\Programs\AutoKeyboardLayot\AutoKeyboardLayot.exe`) with a build whose SHA-256 you
have recorded. It keeps a backup, asks the running agent to close gracefully, verifies the result and
restores the backup when anything fails. It is meant for development installs from a checkout; the
installer remains the way to install for other users.

## Build

From WSL, cross-build the modular base, which matches what the installer ships:

```sh
cargo xwin build --release --target x86_64-pc-windows-msvc --no-default-features \
  --bin AutoKeyboardLayot --target-dir target/xwin-hotkeys
sha256sum target/xwin-hotkeys/x86_64-pc-windows-msvc/release/AutoKeyboardLayot.exe
```

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

Run the same command without `-WhatIf` (`-Confirm` asks first). `-BackupSuffix` names the backup
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

After the preflight it copies the backup, closes the agent with `WM_CLOSE` (waiting at most 10 seconds),
replaces the executable, verifies the installed hash, starts the new agent, waits five seconds and
checks that exactly one agent process runs and its observer window exists. On any failure from the
closing step on, it stops the new agent, restores the backup and restarts the previous agent when one
was running.

The last lines are a receipt: `DEPLOYED_PID`, `EXE_SHA256`, `BACKUP`, and the number of pruned and kept
backups.

## Rolling back by hand

Close the agent from its tray menu, copy the wanted `AutoKeyboardLayot.exe.before-*` file over
`AutoKeyboardLayot.exe`, and start the executable again.
