# UI translation catalog format 1

English is embedded in `data/locales/en.json`. External catalogs are data files
with these fields: `format` (1), `locale`, `direction` (`ltr` or `rtl`), and a
`messages` object mapping stable English-catalog IDs to translated strings.
Optional `aliases` lists at most eight compatible locale IDs. This field is part
of the current unreleased format-1 draft; older draft readers reject catalogs
containing it and fall back to English rather than misreading the data.
Catalog data and text direction metadata alone do not implement RTL layout or
input conversion; those capabilities have separate acceptance gates.

## Validation and fallback

- Unknown/duplicate fields or message IDs, malformed JSON, invalid IDs, invalid
  control characters and incompatible placeholders reject the catalog.
- Every English placeholder (for example `{count}` or `{error}`) must also be
  present in its translated message. Values are interpolated once, never
  interpreted as another template.
- Missing messages use embedded English. All English locale variants use the
  embedded English baseline; external catalogs cannot replace that baseline.
- Locale matching is case-insensitive. Region fallback preserves an explicitly
  requested script: an unavailable Traditional-script translation must not
  silently select a generic catalog that may contain Simplified-script text.
  Explicit aliases can select a compatible catalog, for example `zh-CN` and
  `zh-SG` selecting `zh-Hans`. Aliases must share the catalog's primary language
  and cannot declare a different explicit script. Canonical names and aliases
  share one collision namespace; duplicates reject the whole incoming catalog
  without partially registering its names. Aliases do not create extra packs or
  chooser entries, except that a saved alias is retained instead of rewritten.
  Alias declarations establish routing, not linguistic truth or publisher trust.
- Unknown programmatic message IDs remain detectable errors for formatting;
  the UI adapter converts such errors to the safe English `Text unavailable`
  fallback. This is not a process crash. Source-coverage tests check literal UI
  references against the embedded catalog.

## File handling limits

Catalogs are loaded outside keyboard hooks into immutable snapshots. The loader
does not recurse. It examines at most 1,024 directory entries and accepts at most
128 JSON candidates. Ignored files consume scan budget, not catalog budget.
Catalogs are limited to 512 KiB each, 8 MiB read per directory, 512 messages and
8,192 bytes per message. Interpolated output is bounded to 64 KiB so repeated
placeholders cannot amplify an error argument into an unbounded allocation.
Filesystem I/O itself is not a hard-real-time operation.

Validation uses the opened file handle, not a pre-open pathname metadata check.
Unix opens use `O_NOFOLLOW | O_NONBLOCK`, then reject non-regular files before
reading. Windows opens the final reparse point itself, rejects reparse metadata,
and allows atomic replacement but not concurrent in-place writes. An opened
snapshot remains bound to its original file after a pathname replacement.
Managed package installation and signed release verification are separate
requirements; translation parsing does not establish publisher authenticity.

For offline validation:

```sh
cargo run --locked --example validate_locales -- /path/to/catalog-directory --require-complete
```

Without `--require-complete`, incomplete catalogs can pass structural validation
and use English fallback. The strict option rejects missing keys before fallback.
Release acceptance additionally requires checking every expected message key,
language quality, actual UI layout, fonts and accessibility.

## English message IDs are a compatibility contract

A package embeds its UI catalog. Parsing an installed or downloaded catalog
rejects any message ID that the running executable's embedded English catalog
does not know (`UnknownMessage`). Signed packages already published keep their
embedded catalogs, so removing or renaming an English message ID makes every
older package unloadable; because the store is verified while the configuration
is read, the application then fails to start with
`package_store_Package(InvalidComponents)`.

Never delete or rename an English message ID. Retire text by leaving the ID in
place or by hiding it in the UI. Preserve placeholder sets too: old translations
are checked against them. Adding new IDs is safe for a new consumer loading an
old package, but not for an old consumer loading a new package. New package
producers now declare API 2; release catalogs must retain that requirement so
API-1 applications refuse the upgrade before changing their store.

CI verifies the previously published API-1 packages with the new consumer. The
signed-package tests retain `import.failed` as a compatibility regression. Do
not infer compatibility merely from source-locale completeness tests.
