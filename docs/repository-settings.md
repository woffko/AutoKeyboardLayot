# Repository settings for the owner

These settings live on GitHub, not in the repository, so they are not changed by
any commit. At the time of the September 2026 audit secret scanning and push
protection were on, Dependabot security updates were off, and the repository had
no `SECURITY.md`. `SECURITY.md` and `.github/dependabot.yml` assume the settings
below.

They need an account with admin rights on the repository and the GitHub CLI
(`gh auth login`). Each command changes a setting; run them yourself.

```sh
REPOSITORY=woffko/AutoKeyboardLayot

# 1. Private vulnerability reporting: the "Report a vulnerability" button that
#    SECURITY.md points to.
gh api --method PUT "repos/$REPOSITORY/private-vulnerability-reporting"

# 2. Dependabot alerts. Security updates need them.
gh api --method PUT "repos/$REPOSITORY/vulnerability-alerts"

# 3. Dependabot security updates: pull requests that fix vulnerable dependencies.
gh api --method PUT "repos/$REPOSITORY/automated-security-fixes"
```

Check the result:

```sh
gh api "repos/$REPOSITORY/private-vulnerability-reporting" --jq .enabled
gh api "repos/$REPOSITORY" --jq '.security_and_analysis'
gh api "repos/$REPOSITORY/automated-security-fixes" --jq .enabled
```

To undo a setting, repeat the command with `--method DELETE`.

Optional, if they were ever switched off:

```sh
gh api --method PATCH "repos/$REPOSITORY" \
  -f 'security_and_analysis[secret_scanning][status]=enabled' \
  -f 'security_and_analysis[secret_scanning_push_protection][status]=enabled'
```

`.github/dependabot.yml` (weekly checks for Cargo and GitHub Actions) is read from
the default branch, so it takes effect once it has been merged there.
