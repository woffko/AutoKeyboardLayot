# Localization and modular input-language packs

## Accepted scope

The application follows the current Windows user's display language by default,
with built-in English as the final fallback. All application-owned UI text must
use stable message identifiers. UI language and enabled input-language packs are
independent settings. English UI and the base English input data remain embedded.

The initial product language set is English, Chinese, Spanish, Hindi, Arabic,
French, Portuguese, Russian, Bengali, Indonesian, Urdu, German, Estonian, and
Japanese. Regional/script variants are explicit; this is a product coverage list,
not a claim about a demographic ranking.

## Milestones and acceptance gates

1. **English foundation and catalog runtime** — extract application strings,
   validate bounded external catalogs, resolve the Windows display language,
   and fall back per locale and per message. Keep keyboard hooks free of disk
   access, translation parsing, and UI calls. Add deterministic fallback,
   placeholder, malformed-file, and source-coverage tests.
2. **Complete UI localization** — translate every settings page, tray action,
   notification, dialog and application error; add system/manual language
   selection. Test standard-widget labels, long text, RTL, font coverage, and
   accessibility. Validate all 14 catalogs, not just the settings navigation.
3. **Runtime input-pack registry** — replace closed language enums/static FST
   wiring and per-language booleans with stable IDs and data-backed descriptors.
   Migrate EN/RU/ET first. Preserve dictionary overlays and exclusions for
   disabled/missing packs and preserve all existing configuration values. Before
   introducing incompatible configuration versions, replace the existing
   operational-default fallback on unreadable configuration with a fail-closed
   or last-known-good path that does not silently discard process exclusions.
4. **Independently versioned packs and manager** — maintain independently versioned
   data-only packs with manifests, dictionaries, short-word tiers, layout/input
   variants, confidence rules, licenses and checksums. Add explicit local import
   and selected-package downloads from GitHub, enable/disable, update, remove,
   compatibility checking and atomic rollback.
   Do not load arbitrary DLLs or scripts into the keyboard agent. Signed release
   metadata and bounded extraction protect installation and updates.
5. **Complete language/input coverage** — publish each language's dictionaries
   and rules as a separately downloadable pack, but install and enable only
   chosen compatible input methods. Distinguish
   language identity from exact keyboard layout/profile. Preserve conservative
   same-script ambiguity handling and current-word privacy boundaries.
6. **Composition-aware adapters** — implement and physically test required IME,
   dead-key, AltGr, complex-script, grapheme and composition handling. Japanese
   and Chinese UI translations do not imply working IME auto-correction. A pack
   cannot advertise correction readiness until its required adapter passes.
7. **End-to-end acceptance** — verify config migration, all catalog keys, clean
   English-only startup, pack lifecycle/rollback, and actual Windows typing in
   Notepad, Terminal and a browser. Test continuous typing, focus changes, undo,
   clipboard preservation and protected input. Unit/build success does not
   replace physical acceptance.
8. **Windows installer and upgrade lifecycle** — produce a versioned installer
   with a language-package selection page. The base installer includes English;
   other packages are downloaded individually from GitHub only after the user
   selects them and confirms installation. Do not download the full language
   collection or the source repository archive. Show each package's language,
   components (UI translation, dictionary, input rules), version, compatibility,
   download size, and the total selected size. UI translation and input-language
   participation remain independent even if distributed in one language archive.
   Prefer
   per-user installation without administrator rights or an SCM service. The
   setup UI follows the Windows display language with English fallback. Provide
   Start menu integration and explicit optional startup registration; never
   enable startup conversion as a side effect of installation. Support clean
   install, upgrade, repair and uninstall, including paths with spaces and
   non-ASCII characters. Request graceful agent/settings closure before replacing
   binaries rather than killing an active input session. Preserve existing
   configuration, user dictionaries and exclusions across updates, and retain
   user data by default on uninstall unless deletion is explicitly selected.
   Verify bundle compatibility and integrity, keep a recoverable upgrade path,
   and include dependency/dictionary notices. Test old-version upgrades and
   interrupted installation on an isolated Windows VM. Select the installer
   technology after checking these requirements; building an installer does not
   itself authorize publishing a release or modifying the user's installation.

### Selective package download requirements

- The main `woffko/AutoKeyboardLayot` repository provides a small versioned catalog and
  one release asset per language/variant. Fetch catalog metadata first, then only
  confirmed selections and their explicitly disclosed required dependencies.
  This replaces the earlier separate-repository proposal by explicit user choice.
  Packs remain independently versioned data assets, not a source-repository archive.
- Pin exact compatible package versions and verify signed release metadata,
  expected length and cryptographic checksums before extraction. Reject unsafe
  archive paths and incompatible packs. Do not resolve an unpinned `latest`
  asset independently for every package during the same installation.
- Display download progress and allow cancellation/retry. An interrupted or
  failed download must not install partial contents or remove a working pack.
  Reuse previously verified cached packages where possible.
- Without network access, allow installation of the English base and explicit
  import of local packages. Clearly report selected packages that could not be
  obtained; do not silently claim a complete multilingual installation.
- An English-only installation must still allow opening settings and adding
  packages. If fewer than two compatible input packs are ready, explain why
  automatic conversion is unavailable instead of rejecting all settings or
  silently downloading an unselected language.
- Upgrades retain installed selections and retrieve only selected packages that
  require an update. A full offline bundle may be a separate optional artifact,
  not the default download and not a prerequisite for installation.
- Test selecting one language, several languages and no optional languages;
  verify that unselected package assets are never requested.

## Implementation boundaries

- Programmatic identifiers, config keys and diagnostic event codes stay English
  and stable; user-facing prose is localized before parameter interpolation.
- No network translation or typed-text telemetry. Package downloads occur only
  for explicit installer/manager selections; no silent bulk downloads.
- User lexicons are not pack contents and are never removed during uninstall.
- A missing translation falls back to English; a missing/invalid input capability
  fails closed instead of borrowing unrelated layout rules.
- Existing privacy inspection timeouts remain a tracked issue; this refactor is
  not evidence that skipped-word behavior is fixed.
- Enter/Tab conversion behavior is unchanged by this work.
- Pack sources will be maintained separately from the application. No remote
  repositories, releases, or user installations are changed in this first phase.

## Current status

Milestone 1 now has an embedded English catalog with 133 messages, validated
external JSON catalogs, Windows display-language selection, and application-owned
Slint/tray/message-box bindings. Thirteen external catalog drafts are in a
separate local `AutoKeyboardLayot-language-packs` source project: AR, BN, DE, ES,
ET, FR, HI, ID, JA, PT, RU, UR and zh-Hans. Alongside embedded English, these
represent the planned 14-language set. Each covers all 133 keys and passes strict
structural/placeholder/completeness validation. This is not linguistic or visual
acceptance. The Chinese draft explicitly covers Simplified script; Traditional
Chinese has not been added.

The manual UI-language selector is implemented in source. Initially, an optional
`ui_language=system` or locale ID was stored in the existing schema-1 settings
section. Older schema-1 settings parsers ignore this UI-only key without losing their
operational settings. Missing valid locale preferences are retained for later
package installation; malformed UI values fall back to English without
invalidating process exclusions. Reads do not rewrite configuration.

Apply updates the settings window's localization revision and the tray's UI
revision independently of keyboard layout. Settings singleton lookup uses a
fixed native class rather than its translated title. The exact Slint/winit
backend version is pinned for that native-class integration.

The initial Windows build and strict Clippy passed. Advisory review led to
opened-handle validation for catalog files and separate bounds for directory
scanning and catalog count. English reservation and explicit-script fallback
remain intentional policies. File fixtures are now embedded into test binaries
so native tests do not depend on the build machine's source path.

The Windows build including stable settings RTL geometry, native dialog/menu RTL
flags and strict Clippy passed on 2026-09-08 (Longrun job
`165d16c83d704c9da79992e63c2139e6`). This includes
accessible labels/actions and keyboard activation to the custom navigation,
plus an opt-in `--require-complete` catalog validation mode. Completeness checks
count missing catalog keys before English fallback; they do not establish
translation quality.

The catalog runtime supports up to eight explicitly declared aliases
per catalog. The Chinese draft maps `zh-CN`/`zh-SG` to `zh-Hans`; `zh-Hant`,
`zh-TW`, and `zh-HK` remain English fallback with the installed draft set.
Alias language/script checks and atomic collision rejection are unit-tested.

The configuration-loading revision passed 110 Linux core tests (109 on Windows;
one symlink test is Unix-only). The refreshed Windows build and strict Clippy
passed in job `4cde2565de574e8fb82290d4625f9bdc` on 2026-09-08. Native tests
(expected 109 core and 91 adapter) and actual UI inspection are still pending.
The new `LocalizedTextEdit` settings component uses public Slint
TextInput/ScrollView/ContextMenuArea primitives to put Cut/Copy/Paste/Select All
under the JSON catalog without private translation hooks or embedded optional
language bundles. Input/selection/undo/clipboard operations remain with TextInput.
Pointer selection, Menu/Shift+F10, keyboard shortcuts, long-content scrolling,
IME composition and screen-reader editing require Windows regression checks.
The upstream TextEdit implementation has not been patched.

Initial settings RTL geometry is driven by the
selected catalog's direction, not the requested locale or keyboard layout.
The sidebar switches sides, text labels align with the reading direction,
action rows use explicit mirrored coordinates, and standard accessible checkboxes keep
their native behavior with a wrapping label placed on the appropriate side.
Configuration editors retain their text order and left-aligned editing geometry.
Card heights can grow with wrapped content. This has compiled for Windows but
still requires visual/keyboard/accessibility acceptance. The subsequent native
adapter change adds MB_RIGHT/MB_RTLREADING to message boxes and TPM_LAYOUTRTL to
the tray menu when the selected catalog is RTL, without changing button/default
or command-dispatch flags. Its Windows build passed; native test execution is pending. Shell
notification rendering and full font/mixed-script coverage remain open.

Slint 1.17.1 removes FlexboxLayout from its normal type registry despite shipping
its declarations. The first RTL build exposed that gate; the implementation now
uses stable Rectangle/standard-widget composition with mirrored coordinates and
stacking for two-button rows that do not fit. No experimental feature is enabled.

Managed catalog reload after package installation, RTL acceptance, font/accessibility
acceptance, translation review, and input-plugin milestones remain required. No
claim of accepted 14-language UI or plugin completion is made here.

The M3 prerequisite now fails startup before hooks are installed when configuration
loading fails. The unified file never falls back to legacy files on parse/read
errors, and errors reading any existing legacy component are propagated rather
than dropping exclusions or dictionaries. Only absent files receive defaults.
Reads are bounded and do not rewrite configuration; links/reparse points and
non-regular files are rejected. Worker startup receives the main thread's validated
snapshot instead of rereading disk. A failed worker reload discards pending input
and retains its last-known-good settings and policies; a failed main-thread reload
shows an error without announcing success. Runtime input-pack migration itself is
not yet implemented. The read-failure protection and startup snapshot passed Windows
build/Clippy in job `109c2fb0738f41d8948237ce4338a78f` on 2026-09-08.
The subsequent strict parser rejects malformed known boolean/hotkey values and
invalid legacy dictionary/exclusion rows without rewriting the source files.
Correct legacy boolean spellings, partial hotkeys and unknown forward-compatible
keys remain accepted. Unified settings errors retain original document line numbers;
malformed optional UI-language values keep their established English fallback.
The tolerant public parsing helpers remain available for compatibility, but disk
loading uses checked variants. The strict-parser Windows build and Clippy passed
in job `8a2ea33fc4f14373841067f912553875` on 2026-09-08.

The subsequent reload handoff uses a validated owned snapshot in the bounded
input queue, with configuration revisions independent of input-overflow epochs.
The worker no longer reads configuration files on reload. At most two snapshots
may await worker acknowledgement. Main-thread hook settings and the success
notification are published only after the latest accepted revision is acknowledged;
a failed newer enqueue retains the earlier pending request. Conversion gates and
new conversion hotkeys are disabled during handoff, while ordinary typing passes
through and transition input is discarded from the agent's buffer. An active gate
or retained recovery input rejects the handoff without altering it. These changes
have seven new native regression tests; Windows build and Clippy passed in job
`c534ea053a5e45c4b09802d9b217ec22` on 2026-09-08. Native execution remains pending.
The `configuration_handoff_unavailable` diagnostic code means the saved files were
not adopted by the running agent; retry when pending input or the queue has cleared.
The following source-tracking change records which files were successfully read
with each snapshot. A reload cannot lose any previously used legacy component or
fall back from a unified configuration to defaults/legacy files. Migration from
legacy files to a valid unified document is allowed. This guard checks both the
active configuration and the latest accepted pending snapshot. An empty first
launch still uses defaults without creating files. Three core tests cover the
source matrix, actual reads and file removal; one native test covers active and
pending snapshot protection. Windows build and strict Clippy passed in job
`4cde2565de574e8fb82290d4625f9bdc`; native execution remains pending.

M3 now has a first production integration slice: `DictionaryRegistry` replaces
the separate per-language dictionary caches in every detector lookup. Its bounded,
case-insensitive `PackId` keys are independent of installed data. Snapshots retain
missing selections, separate installation from enablement, and share immutable
FST storage. Owned dictionaries are compiled from bounded word lists outside
input processing; this is not a raw external-FST loader or a package installer.
Automatic and forced detection reject absent/disabled source and target data,
including when user overlays contain a matching word. Removing a pack from a new
snapshot leaves older snapshots and their overlays unchanged. Installing a Japanese
dictionary does not enable an IME adapter.

The Windows runtime prepares a dictionary snapshot from existing settings before
enqueueing configuration and replaces the worker's detector on accepted reload.
Failed reloads retain the existing detector. The current bootstrap still embeds
EN/RU/ET; the final English-only packaging step is not implemented. Language enums,
static scoring/layout models, per-language settings, and persistent overlay IDs
still need migration. In particular, in-memory preservation tests do not establish
schema migration or preservation of unknown IDs on disk. This first slice passed
118 Linux core tests and strict Clippy. Its Windows build and strict Clippy passed
in job `9f167142d6ac4e1fb4efa2f19ff0fbff` on 2026-09-09; native execution remains
pending.

The following schema migration reads unified versions 1 and 2, but explicit saves
write version 2. Settings now persist `enabled_input_packs` as a bounded set of
stable IDs, not three language booleans. Legacy booleans migrate to the equivalent
set; mixing old/new selection syntax fails closed. Empty, single-pack and absent
pack selections are retained. Detector readiness is separate from selection.
User dictionaries and word exclusions also retain syntactically valid unknown
pack IDs. EN/RU/ET UI controls currently edit only their own IDs and leave other
selections untouched; a dynamic package-management UI remains pending.

Reading does not migrate files. Before an explicit save replaces a unified
schema-1 file, its exact bytes are preserved as `config.schema-1.bak`. A different
existing backup, unreadable file, or link prevents migration instead of overwriting
the backup. Legacy standalone files are retained in place. Schema 2 is not suitable
for older binaries: rollback must restore the compatible configuration backup;
this source change cannot retrofit fail-closed behavior into a previously installed
binary. No user installation has been migrated. Seven new tests cover migration,
missing selections/overlays, conflicting input, read-only loading and backup
preservation. Save preparation uses exclusive temporary-file creation; a stale
`config.tmp` or link blocks saving without overwriting that file. Backup failures
stop save preparation before the destination can be replaced. All 125 Linux tests
and strict Clippy pass. The refreshed Windows release build and strict Clippy
passed in job `259f6c0257244ac18c8b7b256c6adb28` on 2026-09-09. Native execution
(expected 124 core and 92 adapter tests) remains pending. A native settings-window round-trip
with unknown selected IDs and overlays still requires Windows execution.

Automatic target enumeration now derives from direct-layout readiness in pack
descriptors instead of separate per-language target arrays. A regression test
checks both source and target capability gates, IME rejection, and independence
from dictionary embedding. All 126 Linux tests and strict Clippy pass. This is
still a transitional built-in descriptor catalog: stable runtime identities,
dynamic scoring models and installed input-profile routing remain unfinished.
That Windows build and strict Clippy passed in job
`74e47d74a5e148a3adb90b2c849251a3` on 2026-09-09; native execution remains pending.

The closed `Language` enum has now been replaced by a compatibility alias for
`PackId`. Built-in names are stable ID constants, not numeric registry handles.
The bounded 64-byte Copy value owns its normalized ASCII identity across runtime
snapshot replacement; it needs neither a heap allocation nor an interning epoch.
The raw keyboard FIFO still carries an OS layout handle, not this identity.
Dictionary and overlay lookup now use the same ID directly. Unknown IDs have no
built-in descriptor, character acceptance or automatic targets. Tests cover an
unknown installed/enabled dictionary with overlays as both source and target,
including forced candidates, and maximum ID length/case/order preservation.
All 128 Linux tests pass. Windows release and strict Clippy passed in job
`e1b3a1fb56514d1282c69f78e301e1c6` on 2026-09-09 (127 core and 92 adapter tests
compiled; native execution pending). This does not complete M3: scoring and character models,
input-profile routing and the descriptor catalog still require migration to
runtime data. No input adapter or package-manager capability is newly advertised.

### Runtime scoring data

EN/RU/ET scoring tables now live in `data/scoring/*.json`, parsed into bounded
immutable models during dictionary snapshot construction, not input handling.
Each dictionary pack owns or shares its model; owned replacement models are
supported without changing an existing snapshot. Scoring and visible-character
acceptance read this model rather than matching language IDs. The detector's
weights and thresholds remain application-controlled in this format.

Format 1 accepts at most 64 KiB of JSON, 64 non-overlapping normalized alphabetic
ranges with at most 65,536 covered code points, 256 bytes of vowels, 512 bigrams,
512 trigrams and 128 rare sequences. Counts apply before deduplication. Unknown
fields, versions, overlapping/reversed ranges, non-alphabetic or non-normalized
characters, duplicate sequences and wrong sequence lengths are rejected. This
API parses data only; it neither imports files nor enables an input adapter.

Tests exercise malformed/bounded data, owned model replacement, preserved
EN/RU/ET scores, a missing-model rejection, and an unknown ID that scores with supplied data but remains
ineligible for both automatic and forced conversion. The raw current-word buffer
also rejects that unsupported identity. All 132 Linux tests and strict Clippy
pass. Windows build and strict Clippy passed in job
`e08154a2b5154122ba314e3facb8a3d3` on 2026-09-09 (131 core and 92 adapter tests
compiled; native execution remains pending).
Exact input-profile descriptors and adapter routing remain the next M3 work;
composition-aware handling and full package lifecycle are still later gates.

The subsequent format-2 scoring revision makes bigram, trigram and vowel evidence
explicitly optional, removing both bonuses and penalties when disabled. A model
without n-gram evidence cannot request statistical automatic targets. Format-1
EN/RU/ET models keep their original scoring policy and operation order. See
[the scoring format specification](language-package-format.md#scoring-model-versions).
The Linux core suite passed 239 tests (one ignored), and strict all-target Clippy
passed on 2026-09-09 after this change. The new strict-parser cases also passed
individually. This does not resolve combining marks, grapheme/composition handling,
new-language model calibration or physical input acceptance.

### Exact Windows input-profile routing: design constraints

The current `find_layout` still chooses the first loaded HKL with a matching
primary language. This is not an exact physical-layout match and must be replaced
before profile-specific adapter readiness can be claimed. The tray's language
label must not serve as conversion authorization.

Microsoft documents HKL as an input-locale handle, whose high word is a device
handle, not a KLID. `GetKeyboardLayoutNameW` obtains the active KLID only for the
calling thread. An exact resolver therefore must not reinterpret HKL bits as a
KLID or query the worker's current KLID as if it belonged to the foreground app.
Any proposed isolated-thread activation probe needs separate review and native
verification of failure handling, restoration and effects on user input.
Unresolved profiles must fail closed; dictionary presence cannot supply missing
profile evidence. The provider and worker integration are described below;
their native acceptance remains unproven.

References: [GetKeyboardLayout](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getkeyboardlayout),
[GetKeyboardLayoutNameW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getkeyboardlayoutnamew),
[ActivateKeyboardLayout](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-activatekeyboardlayout).

### Owned input-profile requirements

`InputPackDescriptor` now supplies bounded data-backed profile requirements in
each dictionary snapshot. EN/RU/ET bootstrap descriptors are in `data/input/`.
Format 1 stores a pack ID and exact Windows keyboard profiles as
`LANGID:KLID` (four plus eight hexadecimal digits); neither a primary language nor
a bare keyboard ID is enough. Lookups never fall back to a sibling layout or
language. These are keyboard profiles, not TSF text-service identities.
The initial IDs follow Microsoft's [input-profile list](https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/default-input-locales-for-windows-language-packs?view=windows-11).

Parsing is limited to 16 KiB, 32 profile rows and eight required-capability IDs
per row, counted before deduplication. Capability IDs are lower-case ASCII tokens
of at most 63 bytes, beginning with a letter. Duplicate profiles/capabilities,
unknown fields/versions, empty requirements and malformed identities fail.
Unknown capability tokens remain requirements, never an implementation lookup or
readiness grant. ET conservatively requests physical-key, AltGr and dead-key
handling; this is not an assertion that those adapters are accepted or active.

Descriptors are immutable and shared by snapshots. Replacement is allowed only
for a matching pack ID and does not mutate an older snapshot or enabled selection.
Tests cover exact layout and language distinctions, malformed/bounded data,
snapshot replacement and mismatched identity. An unknown pack with a dictionary,
scoring model, overlay and a descriptor requesting the existing physical-key
capability still cannot obtain automatic or forced conversion.

All 138 Linux tests and strict Clippy passed for this data-layer slice. Windows
build and strict Clippy passed in job `aad9bfa0bbc84eea91cc5fd389bf2dc1` on
2026-09-09 (137 core and 92 adapter tests compiled, native execution pending).

### Runtime compatibility target selection

Historical intermediate stage: the embedded-descriptor equality gate described
here is superseded by the exact-profile binding integration at the end of this
document. The snapshot and source/target safety invariants remain in force.

The detector and Windows candidate mapper now enumerate eligible targets from
the active dictionary snapshot rather than `Language::automatic_targets`.
Snapshot construction requires an enabled dictionary, a scoring model and an
explicit input descriptor exactly matching the existing bootstrap requirements.
The compatibility set is cached outside input handling. Adding a requirement,
changing a keyboard variant or LANGID, or omitting the descriptor removes that
pack from this compatibility route as both source and target. An unchanged
normalized descriptor remains compatible, and older snapshots stay unchanged.

`DictionaryPack::from_words` no longer supplies an input descriptor merely from
a known pack ID. Only the embedded bootstrap attaches those descriptors
implicitly; callers constructing owned packs must supply input requirements
explicitly. This prevents dictionary replacement from silently restoring a
missing descriptor. Scoring data alone still conveys no input permission.

Automatic, forced, supplied-candidate and built-in transposition paths all pass
through the runtime target filter. The Windows mapper also applies settings
selection before invoking physical mapping. The old language catalog API remains
metadata-only, with no production candidate-routing callers.

This is transitional compatibility validation, not OS-profile attestation or
dynamic adapter registration: new IDs and changed requirements remain closed
until a runtime adapter-capability registry is implemented. The exact-profile
worker integration below replaces the earlier `find_layout` implementation;
no HKL-to-registry bit heuristic has been added. ET's complete composition
capabilities remain a later gate.

All 141 Linux tests and strict Clippy passed for that integration. Windows build
and strict Clippy passed in job `d5fb98da29e74db98ec4c63621b76eb2` on 2026-09-09
(140 core and 93 adapter tests compiled). New coverage checks missing/changed descriptors,
both conversion directions/modes, immutable reloads and filtering before the
Windows platform mapper; the Windows-specific test has not run natively yet.

### Isolated exact-profile provider

`profile_resolver` implements the all-or-nothing probe protocol and
`windows_input_profiles` implements its Win32 provider. The provider runs only
on one dedicated windowless thread using the existing bounded-request transport.
Spawning creates its message queue but does not activate a layout; a query is
explicit. The provider never attaches input queues, loads/unloads layouts, writes
registry data, changes process-wide activation, or uses reorder/reset flags.

For at most 64 already-loaded HKLs, the protocol skips handles identified by
`ImmIsIME`, activates each other handle on its own thread with flags zero,
verifies the previous/current handles, obtains the KLID with
`GetKeyboardLayoutNameW`, rechecks the current handle, and restores the original
layout with previous/current verification. A cleanup guard also attempts
restoration on error/unwinding. Failed restoration invalidates the complete
result even if the guard's final retry succeeds. A changed loaded-handle set,
bad name, unknown original handle or API failure returns no partial snapshot.
Duplicate handles and special activation constants are rejected before probing.

The immutable result binds opaque handles to exact `LANGID:KLID` values and
offers only unique target lookup, never a first-match variant. Its caller must
still enforce freshness, loaded-handle revalidation and generation changes.
Skipping legacy IMEs is not proof that arbitrary TSF composition is inactive.

Microsoft documents [thread-scoped activation](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-activatekeyboardlayout),
[calling-thread KLID lookup](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getkeyboardlayoutnamew)
and [IME detection](https://learn.microsoft.com/en-us/windows/win32/api/imm/nf-imm-immisime).
The selected design still needs native verification under both global and
per-window input-method settings, including transient language-change events,
manual switching, failure/restoration and live typing; API documentation and
mocked tests do not prove absence of user-visible effects.

`examples/resolve_keyboard_profiles.rs` is an explicit interactive Windows-VM
diagnostic. It uses a bounded query, prints profile identities (no typed text,
window titles or process paths), and rejects changes in the caller/foreground
endpoint snapshots. Those endpoint checks do not detect transient switches.
It has not been run on the user's host or accepted as a physical test.

All 147 Linux tests and all-target strict Clippy passed for this provider slice.
Windows build and strict Clippy passed in job `0e031ec0cb0b44f5bbb3d23f4a3a75ed`
on 2026-09-09 (146 core and 93 adapter tests compiled, plus the diagnostic
example). No native execution or physical acceptance was performed.

### Worker exact-profile integration

The input worker now owns one persistent `KeyboardProfileCache`. It polls the
provider with zero wait, refreshes after one second, and expires cached replies
after two seconds. Transport replies themselves have a two-second age limit;
the cache lifetime starts at receipt, not at the earliest per-layout observation.
Provider failures clear availability, retries are throttled to 250 ms, and a
disconnected provider is not replaced with new threads. Hooks do not enumerate,
activate or resolve layouts. No refresh request is started while an input gate
is active; freshness checks can still reject an expired snapshot.

An identical fresh snapshot preserves its generation and buffered word. Changed,
failed or expired bindings invalidate pending conversion and undo state. A
partially typed word is suppressed until its boundary instead of treating its
tail as a new word; an initial snapshot with no buffered word does not itself
suppress the first word. Config reload rebinds the detector to current evidence.

Production detector construction starts with no resolved profiles. Exact,
uniquely resolved profiles restrict eligible sources and targets, including
built-in transposition fallback and explicitly supplied candidate strings.
Foreground recognition now maps its opaque HKL through the confirmed profile
table; `LayoutIndicator` remains UI presentation only. The old first-matching
primary-LANGID resolver has been removed. Candidate mapping and final target
selection use the unique exact-profile binding.

Pending conversion and undo records carry the profile generation. Before a
conversion, the worker validates that generation and source/target requirements;
after the source barrier, it also rereads the loaded-handle set and requires it
to equal the cached inventory before editing. This final inventory check is
read-only on the worker; all activation remains on the dedicated provider thread.
Undo likewise checks generation, inventory and exact current/source profiles.
These checks cannot make Windows layout changes atomic across an entire edit;
existing input/focus guards and native race/rollback acceptance remain necessary.

Unit fixtures supply explicit mock profile snapshots; they do not activate
native layouts. Added coverage checks platform-mode missing/variant/ambiguous
profiles, exact worker lookup, pending generations and timer-driven word-state
invalidation. The existing scan-code mapping test is now read-only and runs only
when its own thread already has the exact US profile, rather than guessing from
an arbitrary loaded language match.

All 149 Linux tests and all-target strict Clippy pass. Windows compilation passed
in job `a611fd672c7e4fa8836857ba6286db91`, but strict Windows Clippy rejected the
now-unused `LayoutIndicator::language` method. That obsolete method has been
removed. Windows build and strict Clippy both passed on rerun in job
`03605c0e2eac46b0b72d526ef6ee88e8` (148 core and 96 adapter tests compiled).
Native verification of global/per-window
input settings, periodic probe effects, normal/forced conversion and undo remains
required before this test candidate is accepted or deployed. M3 still needs the
runtime adapter-capability registry/dynamic UI, and later milestones remain open.

### Code-owned implementation assessment (M3 foundation)

`input_capabilities` now assesses full profile requirements against a code-owned
exact LANGID/KLID inventory. Package IDs do not select implementations and package
data cannot register capabilities. The currently registered implementation is
`physical-key-v1` for the reviewed US, Russian and Estonian keyboard profiles;
unknown profile variants are unsupported, and unknown capability tokens remain
explicitly missing. Embedded ET therefore reports missing `altgr-v1` and
`dead-key-v1`, not full support.

`Detector::profile_implementation` exposes this assessment for active package
requirements. `Implemented` is not conversion readiness: installed OS evidence,
per-event safety, privacy and native acceptance remain independent requirements.
The initial assessment slice did not replace the compatibility gate. It passed
Windows compilation and strict Clippy in job
`0bcf8c7cc0ba4b398d0a7570e0fe82be` (152 core and 96 adapter tests compiled,
not natively executed).

Four new tests cover embedded requirements, exact variant/language matching,
unknown/complex capabilities and separation from platform eligibility. Current
Linux validation passed with 153 tests, all-target strict Clippy, formatting and
diff checks.

### Exact-profile binding integration

The detector now constructs code-owned `InputBinding` values from active package
requirements instead of comparing descriptors to embedded package identities.
An arbitrary stable pack ID can use an approved exact profile and its scoring
model. The offline US/Russian transposition table is selected through those exact
profile bindings, so custom IDs work as both source and target; ET still requires
native scan-code mapping. Neither a package ID nor a language match selects an
unrelated keyboard variant.

All current bindings explicitly have `ConservativePhysicalKeys` scope. The ET
binding preserves its physical subset only when the known full requirements are
present; AltGr and dead-key implementations remain missing. Unknown additional
requirements block the binding. No full requirement was removed or advertised
as implemented. Per-event modifier/composition guards and native acceptance
remain required, and the UI still needs to display the limited scope accurately.

Multiple active packages claiming one exact profile block every claimant,
including when one lacks a scoring model or a usable adapter. A package with
multiple profiles is not assigned the first supported profile; explicit profile
selection is still required. Disabled packages do not reserve a profile. Reloads
construct new immutable bindings and leave the old snapshot unchanged.

Four additional tests cover custom-ID direct/mapped/forced conversion, duplicate
claim rejection and recovery through selection, multi-profile refusal and ET
scope/unknown-capability refusal. All 157 Linux tests and all-target strict Clippy
pass. Windows build and strict Clippy passed in job
`06b895d47d6e4467816f4331ef8a3f20` (156 core and 96 adapter tests compiled,
not natively executed). Dynamic UI/profile selection, native input acceptance
and later milestones remain open.

### Dynamic input selection UI

The language page now derives rows from installed registry IDs plus retained
missing selections, replacing the three hard-coded language booleans and the
nonfunctional Japanese placeholder. Toggling edits only the pending selection;
Apply still uses the existing guarded configuration save and worker handoff.
A deselected missing row stays visible until the settings window closes so that
the user can reverse the choice. Duplicated/invalid row IDs, more than 128 rows,
or more than 64 selected IDs fail validation rather than truncating selections.

Rows distinguish missing data, disabled selection, unavailable requirements and
conservative input scope. ET shows its missing capability identifiers. A notice
separates these settings from live Windows profile availability and native
acceptance; the old About-page assertion of automatic EN/RU/ET readiness was
removed. The current UI and worker both still use the embedded bootstrap
inventory; pack-manager installation and explicit profile selection are pending.

Two portable projection tests cover missing selections, disabled installed data,
conservative ET requirements and installed data without an adapter. All 159 Linux
tests and strict Clippy pass. Updated Slint/Windows UI compilation and strict
Clippy passed in job `7436fac3be294b08bb97822fe0e5d2e1` (158 core and 96 adapter
tests compiled, not natively executed).
Six new English catalog messages bring the base to 139 keys. All 13 external
catalog drafts have now been updated with these six translations; strict
`validate_locales --require-complete` and placeholder validation pass for all
14 locales. This verifies coverage and structure, not linguistic acceptance.
Native checkbox, scrolling, keyboard/RTL layout and save-roundtrip tests remain
required; compilation alone will not establish those results.

The UI now uses the portable `parse_selection` validator. Three additional tests
cover missing-selection roundtrip and explicit deselection, malformed and
case-colliding unchecked IDs, and exact 128-row/64-selection limits. All 162
Linux tests and strict Clippy pass. Windows recompilation and strict Clippy for
this validator passed in job `3ad2d970207c4372a05ae185fc24bcb0` (161 core and 96
adapter tests compiled, not natively executed).

### Explicit profile preferences and schema 3

`InputProfileSelections` now preserves up to 64 exact per-pack choices, including
missing and disabled packages. It rejects malformed IDs/profiles and canonical
duplicate pack IDs. A pack without a preference uses its sole declared profile;
multiple profiles require an explicit choice. A missing or unsupported explicit
profile cannot fall back to another profile, even for a single-profile pack.
Selected declared profiles reserve only themselves. Unselected ambiguous packs
retain conservative collision blocking; multiple claimants remain blocked.
These preferences do not attest OS availability or grant input capabilities.

Configuration schema 3 adds `[input_profiles]` rows `pack-id=LANGID:KLID`. Schemas
1 and 2 remain readable with empty preferences; the new section is rejected in
older schemas. Explicit saves back up old documents byte-for-byte to separate
`config.schema-1.bak` or `config.schema-2.bak` files without overwriting different
existing backups. Older application versions reject schema 3 rather than silently
losing these choices. Startup and worker reload bind preferences into the same
immutable detector snapshot; read failures retain the last validated snapshot.
Settings status projection uses the same resolution, and unrelated GUI saves
preserve the profile map through `ConfigurationDocument`.

New core tests cover exact resolution, variant refusal,
collision scope, snapshot independence, OS-readiness separation, parsing limits,
config roundtrip and schema-2 backup protection. Windows compilation and strict
Clippy passed in job `abe597b987ce45d081a25b8a7e904fe4` (169 core and 97 adapter
tests compiled, not natively executed). M3 and later acceptance gates remain open.

### Settings profile picker

Each input-package row now offers exact descriptor profiles plus a default option
that permits only the sole declared profile. Stale saved profiles remain visible;
choosing them does not bypass descriptor/capability or OS checks. Disabled and
missing profile preferences are retained independently of enabled package IDs.
Rows now include the union of installed, enabled and profile-preference IDs:
at most 192 rows, with independent limits of 64 enabled and 64 explicit profile
choices. Descriptor options are bounded to 32 plus a retained stale choice and
the default option. Two portable tests cover retained stale choices, clearing
preferences and the full disjoint 192-row inventory.

Profile changes update only the UI draft and recompute status using the same
detector resolution. Apply validates the draft and uses the existing optimistic
save-conflict check and schema-upgrade backup path. The saved baseline is not
changed while editing. Closing without Apply leaves disk configuration intact.
The profile label and default option bring all 14 catalog drafts to 141 complete
message keys; strict completeness/placeholder checks pass. Native picker events,
Apply/reopen/cancel, scrolling, keyboard accessibility and RTL remain unverified.
Windows compilation and strict Clippy for this picker passed in job
`97820480f4cc47e6b27633a5b7786817` (171 core and 97 adapter tests compiled,
not natively executed); this does not complete M3 or the
remaining pack-manager, full-input-adapter, native and installer milestones.

### Authenticated package decoder (M4 foundation)

`language_package` now verifies a fixed-role data-only JSON envelope with strict
Ed25519 signatures over domain-separated exact manifest bytes and SHA-256/length
checks for every component. Input-only, UI-only and combined packages are parsed
without filesystem access, downloads or activation. Input data must identify the
same package and include all four word/short-word/scoring/profile components;
catalog identity is checked independently. Required authenticated license and
notice texts do not themselves establish legal permission to redistribute data.
See [the format and trust boundaries](language-package-format.md).

Eight new core tests cover valid components, trust/signature/domain failures,
hash/length/presence tampering, incompatibility and identity mismatches, fixed-role
path/executable rejection, weak/duplicate trust anchors, component independence,
and transport-hash versus semantic identity. All 180 Linux tests and strict Clippy
pass; Windows compilation and strict Clippy with the new cryptographic dependencies
passed in job `7615b11198aa41c7bb92294cd285452f` (179 core and 97 adapter tests
compiled, not natively executed).
No production trust anchors or release-signing keys are generated or enrolled.
Default trust rejects all external packages. This decoder is not an installed-pack
manager: authenticated release metadata, selected downloads, durable installation,
anti-downgrade policy, collision ownership, update/removal and rollback remain.

### Authenticated release selection

`package_catalog` adds signed release metadata, independent repository policy,
expiry checks, catalog-generation checkpoint validation and explicit selective
download planning. It pins exact artifact length/hash/ID/revision and advertised
components before accepting a signed package. Empty selections remain empty;
missing/incompatible selections fail without a partial plan. No downloads occur.
See [the catalog format and remaining manager boundaries](package-release-catalog.md).
Eight new tests cover selection, checkpoints, freshness, signatures/source policy,
URL/path rejection, bounds, malformed metadata and exact download verification.
Windows compilation and strict Clippy for this catalog layer passed in job
`5c3258cc5bbd4278996dd26a08006723` (187 core and 97 adapter tests compiled,
not natively executed). Durable receipt storage,
actual network/installer integration and package-level rollback remain unfinished.

### Inventory candidate transactions

`package_inventory` now stages authenticated imports, exact catalog plans and
removal without mutating the prior snapshot. It preserves per-package revision
high-water marks, rejects same-revision artifact changes, protects the English
base, preflights locale/alias ownership and retains source/catalog checkpoints.
The bounded state codec validates all metadata before requesting artifact reads;
restoration requires the exact authenticated blob set. Dictionary snapshots retain
explicit disabled/missing choices and share immutable dictionary data.
See [the inventory contract](package-inventory.md).
Eight new tests cover atomic/idempotent staging, deletion/reinstall downgrade
checks, locale ownership, exact restoration, malformed state, plan receipts and
freshness, immutable dictionary snapshots, limits and generation overflow.
All 196 Linux tests and strict Clippy passed. Windows build and strict Clippy
passed in job `0c91f6e9b2d54132b3a5f80284bafb68` (195 core and 97 adapter
tests compiled, not natively executed). This layer does not itself write files.

### Local package store

`package_store` adds explicit initialization, fail-closed loading, nonblocking
writer locking, exact-state compare-and-swap, current-trust reauthentication,
immutable artifact/state publication and final CURRENT pointer replacement.
The store retains previous states/blobs and high-water marks across removal;
it never alters user lexicons, exclusions or selections. Eight tests exercise
initialization, lifecycle/reload/cache reuse, stale writers and history loss,
locks, injected commit-boundary failures, invalid artifact sets/trust, corruption
and Unix no-follow/non-file protections. All 204 Linux tests and strict Clippy pass.
See [the store contract and limitations](package-store.md). Windows compilation
and strict Clippy passed in job `ad025a3134dc4191bb7daab24a916466` (202 core and
97 adapter tests compiled, not natively run). Explicit older-version
rollback policy, real process/power interruption and native Windows acceptance
remain unfinished; no user installation has been changed.

### Managed snapshots in startup, settings and agent reload

The Windows source now loads installed data at startup and settings entry.
Managed mode uses English-only base data plus authenticated packages and signed
UI catalogs. Its initial directory-presence discovery has since been replaced by
the explicit persisted mode described below.
Settings callbacks reuse their loaded immutable registry. Agent reload uses one
bounded background loader, coalesces newer explicit requests, and checks package
source continuity against both active and pending snapshots before enqueue.
Input-worker acknowledgement publishes the matching catalog/source without file
reads or package compilation in the hook/input-worker paths.

Two new core tests verify managed empty startup, retained missing selections and
source monotonicity. Store fixtures now exercise a combined signed input/UI pack,
projection, independent enabled selections and immutable snapshots after removal.
Three Windows adapter tests cover loader coalescing, active/pending source checks
through acknowledgement, and managed-base/policy retention on a read error.
All 206 Linux tests and strict Clippy passed. Windows compilation and strict
Clippy passed in job `a31ed6ff5c25415bad4474c60b2d9d3a` (204 core and 100 adapter
tests compiled, not natively executed). Release trust anchors remain unprovisioned.

### Persisted managed mode and schema 4

Schema 4 requires `[packages]` with exactly one `mode=legacy` or `mode=managed`.
Schemas 1–3 and standalone legacy files continue to load as legacy mode; ordinary
settings saves retain the document's mode and profile choices. Invalid, duplicate,
missing or old-schema package-mode fields fail parsing. Schema-3 upgrades now
receive an exact non-overwriting `config.schema-3.bak`, alongside the earlier
schema-specific backup rules. No configuration is rewritten merely by loading it.

Managed startup/settings loading requires the managed store even when its
directory is absent. Tests persist a managed config, load a valid empty store,
move the store aside, then freshly reread configuration and confirm failure rather
than bootstrap fallback. Legacy mode ignores staged/orphan package directories,
so preparing a store is not itself an activation operation. A separate fresh
managed-profile constructor selects only English and leaves startup conversion off;
existing-user migration must retain the user's original document instead.

Four new tests cover mode/old-schema preservation, strict parsing, exact schema-3
backup and restart behavior. All 210 Linux tests and strict Clippy pass. Windows
compilation and strict Clippy passed in job `0addfe262aad4a9f98a252816d57a3e3`
(208 core and 100 adapter tests compiled, not natively executed).
Native acceptance, explicit manager/installer migration, release trust anchors and
recovery when configuration itself is removed remain separate boundaries.

### Prepared local imports

`PreparedImport` now reads and authenticates one bounded local file without
writing to storage, stages the candidate and retains its exact previewed bytes.
Confirmation consumes the preparation and rechecks current trust and the original
inventory. Cancellation is a drop with no writes. Four new tests cover source
replacement/deletion, cancellation, changed trust, stale confirmation, invalid
and oversized files, and Unix symlinks. All 214 Linux core tests passed. The
subsequent settings integration and Windows compilation are described below;
native acceptance remains pending.

### Local import settings wiring (native validation pending)

Managed-mode settings now expose local file selection, authenticated metadata
and full license/notice preview, separate install confirmation and cancellation.
A single bounded background operation performs preparation/commit, with completion
handled on the settings event loop. Successful installation refreshes the shared
settings registry/catalog snapshot while retaining current selections/profiles
and notifies the agent; it does not enable input participation. Legacy mode is
explicitly gated pending migration. Trust anchors remain deliberately empty.
The eight new messages are now present in English and all 13 external drafts.
The strict catalog verifier passes for all 14 locales with 149 keys; this does
not certify linguistic or visual quality. All 214 Linux core tests pass, including
literal key coverage of the new module. Windows release/all-target compilation
passed in job `c8c83744492e49b6b070213dc970675e` after fixing a String borrow.
Strict Windows all-target Clippy subsequently passed in 19.17 seconds.
Import callbacks also reject reentry while the native file picker pumps messages.
External advisory review was not run because source-egress authorization remains
unconfirmed. Native interaction acceptance remains pending; this is not a
delivered test installer.

### Explicit package removal and settings lifecycle

Managed settings now list authenticated optional package IDs/revisions on an
explicit refresh and stage removal of one selected package. Removal has its own
component/impact preview and confirmation. The same single-operation background
path commits against the previewed inventory and refreshes live settings data.
English is absent from this optional list and inventory removal also rejects it.
User selections, explicit profiles, UI preference, lexicons and exclusions are
not erased; cached artifacts and anti-downgrade history remain intact.

Ordinary window close, Apply, OK and settings Cancel are blocked while a native
file picker or package worker is active. Cancelling a prepared preview still
performs no writes. Package controls share the page's scrollable area, and the
target selector cannot silently change while a confirmation is displayed.
Five additional messages have translations in all 13 external catalogs (154
English keys in total). Linux core tests pass (214). Windows compilation and
strict Clippy passed in job `24e285e47cb64f978b08f710216f9d33`; native validation
of removal/close guards remains pending.

### Migration preparation and exact mode-transition backup

`PreparedStoreInitialization` authenticates a bounded selected file set without
creating storage, checks required input IDs against the resulting English-base
registry, and retains the exact raw artifacts. Confirmation rechecks current
trust before initialization. Only an empty initialized store or the exact same
previously confirmed candidate can be reused after an unsuccessful configuration
save. Unrelated inventory/history and partial initialization are not adopted or
repaired. The API does not write configuration; managed mode must be saved last
by the caller, so a staged store alone cannot activate migration.

Configuration save preparation now creates an exact non-overwriting
`config.packages-legacy.bak` before a legacy-to-managed switch, even within schema
4. A conflicting backup prevents the save. Standalone legacy files remain in
place when there is no unified configuration to back up. New tests cover required
inputs, exact-byte confirmation/retry, revoked trust, unrelated/partial stores,
English-only initialization and exact current-schema backup. All 219 Linux tests
passed, and Windows build/strict Clippy passed in job
`91e53c75a2f84bb6b3f7f3f217c852c2`.

### Explicit migration dialog (Windows validation pending)

Legacy settings now expose a separate multi-file migration action. The dialog
requires the draft to match the saved document and preserves every currently
installed legacy input ID, not just the enabled subset. Verified package names,
total size and the migration effects are previewed; individual metadata and legal
text are viewed one package at a time. Confirmation rechecks the draft and saved
document, initializes the store, projects its data, then saves the same document
with only package mode changed through the existing concurrency-checked writer.
Before that final save, `with_current_snapshot` reauthenticates and pins the exact
store under its writer lock; the config writer lock is acquired inside it.
A competing store update prevents activation against a stale preview. That last
save is the activation point. Failure leaves legacy configuration in
place (unless independently changed) and may leave a reusable prepared store.

Successful activation updates the settings process's saved-document baseline and
registry/catalog snapshots and notifies the agent. Edits made after confirmation
remain unsaved in the UI. Native multi-selection decoding bounds the UTF-16
buffer/list, preserves Unicode paths, and rejects non-child names and truncated
multi-selection data; three new Windows-only tests await native execution.
All 14 catalogs now cover 159 English keys. Windows compilation and strict Clippy
passed in job `6bbd01820b0a46108682b8d5dd37523c`. Native migration acceptance
remains pending; production trust is still
unprovisioned and no actual user configuration was migrated during development.
A new core test verifies stale activation rejection, exclusion of a competing
writer during the callback, and lock release/config-save error behavior.

### Selected artifact network transport (validation pending)

The new `package_download` API accepts one pinned selection, restricts HTTPS
redirects to the explicit GitHub release hosts, bounds headers/body, uses
cooperative cancellation/timeouts and authenticates exact returned bytes before
exposing a package. It performs no writes or installation. Portable URL/body
tests and the pinned-artifact wrapper checks are in place; the full Linux suite
has 223 tests. See [transport scope and limitations](package-download-transport.md).
Windows compilation and strict Clippy passed in job
`fd40d9b0a78b4496b76152c60868b340`; actual network/TLS/proxy tests remain pending.
No production key or release endpoint was
invented or provisioned, and no live package download was performed.

### Selected online workflow and catalog acceptance (Windows validation pending)

`package_install` connects an independently approved source, immutable selection,
accepted catalog checkpoint, exact cached/network artifacts and a separate
whole-plan confirmation. The settings page now fetches signed catalog metadata,
starts with no selected packages, shows size/components/compatibility, supports
cooperative cancellation and bounded progress, and reviews each package before
installation. Accepted catalog history survives cancellation without installing
package data. The manager handles uncertain writes by rereading/notifying with
an explicit durability warning, never automatically repeating a commit.

Five new portable tests cover selected-only/cache/receipt/cancellation/staleness
and the bounded catalog body. All 228 Linux tests and all 14 catalogs (176 keys)
pass. Full behavior and remaining gates are documented in the
[online package workflow](online-package-manager.md). Windows compilation,
actual networking/UI interactions, signing authority, release publication,
online migration/installer and explicit rollback remain unaccepted or incomplete.

Windows release/all-target compilation and strict Clippy of the selected online
workflow subsequently passed in job `0c416f3ce34a41e2b18468a055220434` (71-second
build and 20.99-second Clippy). Native runtime/network acceptance is not implied.

### Explicit cached rollback backend

`stage_rollback` and `PreparedRollback` now prepare a deliberate one-step rollback
to the exact immediately preceding installed artifact, with current-trust checks
and separate confirmation. Updates record bounded target receipts; ordinary
imports/plans still reject lower revisions. Confirmation uses a fresh generation
and existing CAS publication, preserving high-water and catalog checkpoints.
Removal clears rollback eligibility, not downgrade history. No cached-directory
scan, download or CURRENT rewind is involved. See the updated
[inventory format and rollback contract](package-inventory.md).

Five additional tests cover exact target identity, no repeated/implicit rollback,
restoration, forged target history, ownership collisions, cancellation/staleness,
missing/corrupt cache, revoked trust and injected commit-boundary failures.
All 233 Linux tests and strict all-target Clippy passed. The rollback UI,
Windows compilation of this backend, independent policy review and native
acceptance remain pending. This is not a ready test installer.

The abstract independent policy review completed in job
`33c2c8e1a7dd4176a28095a38e8ab227`; no source attachments were sent. Intermediate
revision lockout remains intentional and now has an explicit test and UI warning.
Managed settings now provide distinct previous-version inspection and rollback
confirmation, show complete target licensing, preserve current choices, and
disable repeated inspection after a failed cache/authentication check until refresh.
Five new messages are translated in all 14 UI catalogs (181 keys). Linux core
tests remain 233 passing. Windows compilation and native UI acceptance of this
integration are pending, along with the broader unfinished plan gates above.

Windows release/all-target compilation and strict Clippy of rollback integration
subsequently passed in job `e197b70037a34b858c5397277e6f306f` (70-second build,
21.01-second Clippy). Native rollback interaction is still not verified.
