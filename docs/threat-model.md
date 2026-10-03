# Threat model

This page says what AutoKeyboardLayot protects, from whom, and what it does not
try to protect. It is short on purpose and describes the code as it is. The
details of each mechanism are in the pages linked below; the September 2026
audit findings (F-numbers) are in [`audit-remediation-2026-09.md`](audit-remediation-2026-09.md).

## What the program is

A Windows user-session agent. It installs low-level keyboard and mouse hooks,
watches the word being typed, and, when a word was typed in the wrong keyboard
layout, replaces it. It runs with the rights of the signed-in user, not as a
service and not elevated. Language data comes as signed, data-only packages.

## Assets

| Asset | Why it matters |
|---|---|
| What the user types | Every program receives it through the hooks, including passwords and private messages. |
| The clipboard | The protected-paste backend uses it for a moment; it holds whatever the user copied. |
| The user's settings, dictionary and exclusions | `config.ini` in `%LOCALAPPDATA%\AutoKeyboardLayot`; it names words and programs the user chose. |
| The installed language packages | They decide what the detector converts. |
| The release signing key and the trust list | Whoever holds a trusted key can make every installed copy accept a package or catalog. |

## Who the program defends against

| Threat | Defence | What is left |
|---|---|---|
| A network attacker, or a tampered release asset, supplies a catalog or package | Ed25519 signatures checked against keys compiled into the program; a catalog window of at most 31 days; a revision number that never goes down; size limits; downloads from the pinned repository over HTTPS with a reviewed redirect policy; packages are data only and are never executed | A stolen release key (see below); the first catalog of a fresh install has no earlier revision to compare with |
| Malformed or hostile package, catalog, configuration or installer input | Strict parsers with bounds and no panics; `tests/parser_mutation_smoke.rs` feeds seeded mutations to thirteen of them on every test run | Mutation testing is a smoke test, not a proof; real fuzzing is not done |
| Typed text leaks to disk or the network | The current word is kept in memory only; nothing typed is logged; diagnostics are off by default and never record characters, words or key codes (`diagnostics_never_log_key_identity`); no network access except the explicit package download | Another process of the same user can read the same keystrokes (see below) |
| Conversion of secrets | Password fields (found with UI Automation), excluded programs, elevated windows and unclear contexts fail closed; an unreadable privacy answer suppresses conversion for that word | UI Automation is the source of truth; a program that hides a password field from it is not recognised |
| Another user or a low-privilege process on the machine | Per-user profile directory, the instance guard and the installation fence are per user; named objects are local to the session | Not a boundary against administrators |
| A malicious or faulty update of the program itself | None built in: there is no auto-update. Releases are published on GitHub and the installer is not yet Authenticode-signed (F09) | SmartScreen and antivirus warnings are expected; a user must verify the download |
| Compromised dependencies | `Cargo.lock` pins every version; CI runs `cargo audit --deny warnings` and `cargo deny` (licenses, sources); Dependabot proposes updates weekly | Four accepted "unmaintained" notices, each justified in `.cargo/audit.toml` |

## Out of scope: code running as the same user

Anything that already runs as the same Windows user at the same integrity level
can install its own keyboard hook, read the clipboard, read and rewrite
`config.ini`, inject input and end this process. It gains nothing by attacking
the program, so the program does not try to resist it.

This is the accepted risk behind finding F13. The agent's hidden window accepts
its own `WM_APP` service messages and `WM_CLOSE` from any process of the same user,
and Settings finds the agent window by class name without checking which
process owns it. What such a process can do is end the agent, pause or release
its correction gate, make it reload the configuration, or take the place of the
agent window; none of the messages carries text, and the same process could end
the agent with `taskkill` anyway. Checking the sender would add code and cost
without a gain, so it is not done. A higher-integrity process is different: the
hooks and the privacy probes do not act on windows of a higher privilege level
(the hook watchdog also waits for such a window to leave the foreground).

Also out of scope: an unlocked or unencrypted machine in someone else's hands,
a compromised Windows installation, and the confidentiality of the Win32
clipboard against other local clipboard readers (the program opts its paste out
of clipboard history and cloud sync but cannot hide it from a process that
reads the clipboard at that moment).

## The release key

The release signing key is protected by DPAPI for one Windows profile. If it is
lost, no new catalog can be signed; if it is stolen, the thief can sign
packages and catalogs that installed copies accept. The trust list can carry a
second, offline recovery key (format 2 of `public-key.json`); the procedure for
creating it and for responding to a theft is in
[`package-signing-key.md`](package-signing-key.md). No recovery key has been
created yet.

## Related pages

- [`../README.md`](../README.md): privacy boundary, diagnostics and crash log.
- [`package-signing-key.md`](package-signing-key.md), [`package-release-catalog.md`](package-release-catalog.md): trust list, catalog window and renewal.
- [`package-store.md`](package-store.md), [`language-package-format.md`](language-package-format.md): what a package may contain and how it is stored.
- [`../SECURITY.md`](../SECURITY.md): how to report a vulnerability.
