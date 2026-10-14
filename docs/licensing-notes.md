# Licensing notes

This file records what the repository says about the licenses of its contents and lists questions that
need a decision by the project owner or a lawyer. It states facts and open questions only; it is not
legal advice and draws no legal conclusions.

## What is recorded today

| Material | Where | License as recorded |
|---|---|---|
| Project code, tools, documentation, UI translation drafts | repository root | MIT: `LICENSE`, and `license = "MIT"` in `Cargo.toml` |
| English dictionary | `data/language-packs/en-US/LICENSE.words.md` | public-domain dedication (Unlicense text) |
| Russian dictionary | `data/language-packs/ru-RU/LICENSE.words.txt` | copyright of Alexander I. Lebedev under BSD-style conditions; modified versions must be marked |
| Estonian dictionary | `data/language-packs/et-EE/LICENSE.words.txt` | Jaak Pruulmann's work under GNU LGPL, plus the Institute of the Estonian Language (EKI) software licence; the pack notice says EKI was informed of the use |
| Layout model | `data/layout-model/NOTICE.md` | CC BY-SA 4.0 (training data from wordfreq and from FrequencyWords, which derives from OpenSubtitles) |
| Slint (settings UI) | crate metadata of `slint`, `slint-build`, `i-slint-core`, `i-slint-backend-winit` | `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` |
| Application icon | `assets/` | authorship and license are not recorded (`assets/README.md` only describes how the files are generated) |
| Other Rust dependencies | `Cargo.lock` | per crate; their license texts are collected into the installer's `THIRD-PARTY-NOTICES.txt` |

Where each material ships:

- The English dictionary and the layout model are compiled into every build of the executable.
  `tools/collect_dependency_notices.py` puts both notices into `THIRD-PARTY-NOTICES.txt`
  (sections "Bundled English dictionary" and "Bundled layout model").
- The Russian and Estonian dictionaries ship only inside signed language packages
  (`.aklp`), each with its own license and notice files.

## Open questions

1. **Estonian dictionary (LGPL) in signed packages.** The data ships in signed packages that the
   application only accepts when they carry the release signature, so a user cannot substitute a modified
   dictionary package without a new signing key. Does this satisfy the LGPL terms that apply to the
   Pruulmann work, and does the EKI licence allow this form of redistribution?
2. **CC BY-SA 4.0 layout model inside an MIT executable.** The model file is distributed under the
   license of its data, and `data/layout-model/NOTICE.md` says the code that reads it keeps the project
   license. The model is embedded in the executable and is always present. Does the ShareAlike
   condition reach only the model data (adapted material) or also the executable? Are the attribution
   and notice requirements met by `THIRD-PARTY-NOTICES.txt`, the repository notice and the About page?
3. **Slint license option.** The crates offer three options and the repository does not record which one
   applies to the distributed executable. The README only says the settings UI uses Slint. If the
   Royalty-free license is the choice, which attribution does it require, and where must it appear
   (README, About page, installer)?
4. **Application icon.** Who created `assets/icon-source.png` and under which license may it be
   distributed? Until the owner supplies a statement, the icon stays an open item and the README makes no
   claim about it.

## Decisions to record here

When an open question is answered, record the answer and its source in this file and update the README
"License and third-party notices" section and, where needed, the installer notices.
