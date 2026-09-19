# Embedded language packs

Language packs keep detector data and its redistribution terms separate from
the platform adapter. `build.rs` converts the word sources into compact,
read-only FST sets embedded in the executable. Typed text is never added to
these files.

Each direct-layout pack also includes a small, project-maintained
`common-short-words.txt` containing curated one/two/three-letter words. These lists
are language data, not source-to-target replacement rules. Their separate FST
tier allows unambiguous short corrections (`yt` → `не`, `рш` → `hi`) without
treating every abbreviation in the large spelling dictionaries as an equally
likely word. Known common source words, user dictionary entries, explicit word
exclusions and ambiguous target languages continue to block automatic changes.
The short tier may hold one to three characters. One-character tokens are
evaluated only when the `single_letter_words` setting is on, only against the
short tier or the user dictionary, and short statistical-only guesses stay
excluded; embedded base tiers drop one-character rows while runtime packages
keep them.
A specific unwanted short conversion can be suppressed with a user-dictionary
entry, for example `en-US: vs`.

The optional Russian single-letter tier contains `а, и, в, к, о, с, у, я`.
The interjection `э` is intentionally not included: its US-keyboard source is
an apostrophe. With single-letter conversion enabled, the shipped pairs are
`f→а`, `b→и`, `d→в`, `r→к`, `j→о`, `c→с`, `e→у`, `z→я`; case is preserved.
Correctly typed listed Russian words remain unchanged. A user who intentionally
types English letter labels (for example `plan b` or `vitamin c`) can protect
them with `en-US: b` and `en-US: c` in the user dictionary. English `a` and `i`
remain protected by their own short tier. The setting is still off by default.

Managed installations obtain Russian data from the signed package, not from a
source-file edit: install the updated Russian package to use the expanded tier.

## en-US

- Source data: `dwyl/english-words`, processed and compressed by PolterType.
- PolterType snapshot: `0143f21e81ffac31231508ef3e77d9db0f7e921f`.
- Data license: Public Domain / Unlicense; the exact notice is stored beside
  the word list as `en-US/LICENSE.words.md`.
- Source archive SHA-256:
  `719766520cf7e076ac52f3d50549eed6a97c8006afeca06e99661d79f3d46a24`.

## ru-RU

- Source data: LibreOffice `ru_RU` Hunspell dictionary, expanded into surface
  forms and compressed by PolterType.
- PolterType snapshot: `0143f21e81ffac31231508ef3e77d9db0f7e921f`.
- Data license: BSD-style license by Alexander I. Lebedev; the exact notice is
  stored as `ru-RU/LICENSE.words.txt`.
- Source archive SHA-256:
  `079cf053e813f0a1c5537be6c9f0fe64ffb068f42c0a560dbdd4200f2c8e06e1`.

## et-EE

- Source data: LibreOffice `et_EE` Hunspell dictionary at snapshot
  `32b006a2c22a4ac7e8ed3f03346f7b3d85a970a4`.
- Data license: EKI Software Licence plus LGPL terms for the MySpell
  adaptation; the complete upstream notice is stored as
  `et-EE/LICENSE.words.txt`.
- The original ISO-8859-15 `.dic` and `.aff` files are retained unchanged.
- Dictionary SHA-256:
  `cd1378434aefeaa8a31f49369dbf71caf4e6340badb5c2cf7a55820933ed4f13`.
- The pack uses buffered physical scan codes plus the installed Windows
  Estonian HKL. Dead-key or ambiguous multi-layout candidates fail closed.

## ja-JP

Japanese is registered as an `ImeRomaji` pack and has no embedded word list in
this milestone. Automatic correction remains fail-closed until the Windows
adapter can detect and preserve active IME composition. Treating Japanese as a
direct one-key-to-one-character layout would corrupt composition state.
