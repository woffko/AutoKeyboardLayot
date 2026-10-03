# Security policy

## Reporting a vulnerability

Please report a suspected vulnerability privately. Do not open a public issue or
pull request for it.

- Use GitHub's private vulnerability reporting: open the **Security** tab of the
  repository and choose **Report a vulnerability**.
- If that choice is not offered, write to the repository owner through the
  contact on their GitHub profile and ask for a private channel before you send
  any details.

Useful details: the version shown in Settings and About (for example
`0.1.0 (abc1234)`), the Windows build, the steps that reproduce the problem and
what an attacker would gain. Please do not send real typed text, passwords or
other personal data; a description of the text is enough.

This is a small project maintained by one person. The aim is to acknowledge a
report within 7 days and to give a first assessment within 30 days. A fix is
released as soon as it has been verified, and the report is then published as a
GitHub security advisory. Credit is given if you want it.

## Supported versions

The latest release and the `main` branch receive fixes. The program is
pre-1.0 and has no update mechanism yet, so users need to install a newer
release themselves.

## What is in scope

- The agent: keyboard and mouse hooks, text replacement, clipboard handling and
  the privacy checks (password fields, excluded programs).
- The installer and the installation lifecycle.
- Verification of signed language packages and of the release catalog, the
  package store and the download path.
- Handling of the release signing key and of the trust metadata
  (`docs/package-signing-key.md`).
- Build, CI and release configuration.

## What is out of scope

- Code that already runs as the same Windows user at the same integrity level.
  Such code can read the user's keystrokes and files without help from this
  program. See [`docs/threat-model.md`](docs/threat-model.md) for the reasoning
  and the accepted risks.
- Physical access to an unlocked or unencrypted machine.
- Vulnerabilities in third-party components themselves. Please report them
  upstream. Dependency advisories are tracked with `cargo audit` and Dependabot.
- Wrong conversions of typed text. Those are bugs, not vulnerabilities; please
  use a normal issue.

## If you think the signing key is compromised

Report it privately in the same way and say so in the first line. The response
plan is in the "Recovery key" section of
[`docs/package-signing-key.md`](docs/package-signing-key.md).
