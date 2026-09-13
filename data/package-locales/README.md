# Optional UI catalogs

These 13 source catalogs were imported from the local
`AutoKeyboardLayot-language-packs/ui` working tree on 2026-09-12, following the
decision to use the main AutoKeyboardLayot repository. The former source tree
was left unchanged. Three lifecycle messages were added to each imported catalog.
Each catalog now contains all 184 current English message IDs and passes the
runtime's strict coverage and placeholder validation.

These remain translation drafts: linguistic review, native layout/RTL/fonts,
accessibility and user acceptance are not established by structural validation.
The project owner selected MIT on 2026-09-12; these project-authored translations
are covered by the root `LICENSE`. Third-party data retains its own terms.
These files are not embedded into the English base by their presence
here. UI-only revision-1 package candidates were signed and independently
verified locally on 2026-09-12; they have not been installed or published.
See `target/ui-package-candidates-20260912-01/verification.json` for exact hashes.

Validate from the repository root:

```sh
cargo run --locked --offline --example validate_locales -- data/package-locales --require-complete
```

Do not validate the stale `target/locale-drafts` snapshot as release source. It
also contains a non-catalog `pack-index.json` which the catalog-only loader
correctly rejects.
