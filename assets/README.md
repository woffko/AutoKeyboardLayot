# Application icon assets

- `icon-source.png` — reviewed master image (1254x1254).
- `app-256.png` — 256x256 PNG used by the Slint settings window (`Window.icon`).
- `app.ico` — multi-size icon (16/24/32/48/64/128/256, 32-bit DIB entries).
- `app.rc` — resource script: `1 ICON "app.ico"`.
- `app.res` — compiled Windows resource linked into every executable target by
  `build.rs` (`cargo:rustc-link-arg-bins`). The tray icon is drawn separately and
  does not use this file.

`build.rs` only links `app.res` for `target_os = "windows"`; when the file is
missing the build warns and produces executables without an icon.

## Regenerating

`app.ico` and `app-256.png` are generated from `icon-source.png` with .NET
(`System.Drawing`): each size is drawn with high-quality bicubic resampling and
written as a 32-bit BGRA DIB entry with an empty AND mask. Regenerate `app.res`
after changing the icon:

```sh
cd assets && llvm-rc-18 /fo app.res app.rc
```

The installer references `..\assets\app.ico` through `{#SourcePath}` so the
setup executable, Start-menu shortcuts and the uninstall entry all use the same
icon.
