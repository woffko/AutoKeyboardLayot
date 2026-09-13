# Composition/IME adapter plan

Status: design. No composition-aware adapter is implemented yet. This document
separates verified facts from unknowns so the first implementation slice does not
guess Windows composition behavior.

## Goal and non-goals

Goal: make conversion safe and useful for keyboards that use AltGr, dead keys or
an IME, without corrupting input or reading composed text incorrectly.

Non-goals for the first slice: implementing IME candidate correction, changing
the EN/RU/ET physical behavior, or enabling any pack that requires a capability
that is not implemented and physically accepted.

## Verified facts (repository)

- The conservative binding registers only `physical-key-v1` for the exact
  US/RU/ET profiles and explicitly withholds `altgr-v1` and `dead-key-v1`
  (`src/input_capabilities.rs:56`, `:95`).
- The input scope enum has a single variant, `ConservativePhysicalKeys`
  (`src/input_capabilities.rs:13`).
- The foreground context already captures the top-level window, focus window,
  input thread id, process id and current `HKL` (`src/windows_agent.rs:3075`).
- `ImmIsIME` is already imported and used by the isolated keyboard-profile
  provider to skip legacy IME layouts (`src/windows_input_profiles.rs:16`,
  `:47`).
- The edit-unit guard rejects composite graphemes and supplementary-plane
  scalars rather than assuming a Backspace count (`src/conversion.rs:54`,
  `docs/composition-adapter-boundaries.md`).
- Documentation states the provider is run on one dedicated windowless thread
  and that no layout activation happens outside it
  (`docs/localization-and-language-packs.md`, "Isolated exact-profile
  provider").

## Unknowns that must not be guessed (`UNKNOWN`)

- Whether `GetKeyboardLayout(input_thread_id)` plus `ImmIsIME` is sufficient to
  detect an active composition for the focused control, or whether per-window
  `ImmGetContext`/`ImmGetCompositionString` state or TSF is required.
- The thread affinity and cleanup contract of `ImmGetContext`/`ImmReleaseContext`
  when the focused window belongs to another process.
- Which Win32 messages or UIA properties reliably indicate "composing" for
  Notepad, Windows Terminal and browser text fields on the target build.
- Whether reading composition flags can block or be re-entered inside the
  low-level keyboard hook, and the resulting latency bound.
- How each target application reports committed vs pending text after both
  Backspace edits and IME commits.

Each unknown is resolved by documentation plus a native probe on the reviewed
test machine before the code depends on it. Until then the adapter must fail
closed rather than assume composition is inactive.

## Failure and security model

- Treat any composition/IME query as a trust boundary: it can fail, return stale
  data, or describe a different thread than the one that will receive edits.
- Never mutate text while a composition is active or while the composition state
  is unknown. Suppress conversion instead.
- Bound the query (timeout, no allocation in the hook path, no blocking of the
  input queue).
- Keep the existing fail-closed checks: profile, exact profile binding, privacy
  inspection, per-event modifier guards and the edit-unit guard remain in force.
- Do not read or log composed content; do not bypass protected-input handling.

## First bounded slice: composition guard (fail-closed)

1. Add a narrow, code-owned `composition-guard-v1` implementation that answers
   one question for the exact foreground input thread: is a composition active or
   indeterminate?
2. Wire it into the conversion path before any edit is produced: if composing or
   indeterminate, suppress conversion and do not mutate or inspect text.
3. Keep `physical-key-v1` behavior unchanged; a pack that requires
   `composition-guard-v1` must still satisfy every other requirement.
4. Unit-test the guard through an injected state source so the portable logic is
   covered without native IME state; keep the native query behind the existing
   bounded provider pattern.
5. Verify natively on Windows: EN/RU/ET typing still converts; typing while an
   IME composition is active does not mutate or capture composition text.

## Later slices

- Dead-key state: track the pending dead-key state for the exact profile and
  refuse or handle the two-event sequence explicitly.
- AltGr: implement `altgr-v1` only after per-event modifier guards and native
  acceptance for the exact profile.
- Composition-aware correction: only after the guard is accepted; never claim
  IME correction readiness from UI translation coverage.

## Capability and readiness contract

- New capability ids are code-owned and exact-profile based, never granted from
  package data.
- A pack may require a new capability, but automatic conversion stays disabled
  until the implementation exists and passes native acceptance.
- The settings projection must show the conservative scope and any missing
  capability identifiers, as it does today.

## Verification gates

- Gate A: Linux `cargo test`, strict Clippy; Windows cross build and Clippy.
- Gate B: guard unit tests plus the existing detector/session/conversion tests.
- Gate D: manual review of every new `unsafe` block and its invariants.
- Gate F: final diff review for new raw pointers, blocking calls in the hook
  path, validation bypasses and any change to protected-input behavior.
- Native: the operator checklist in `physical-input-acceptance.md` plus an
  IME-specific pass on the reviewed test machine; results stay conservative until
  that pass exists.

## Open questions

- Is `composition-guard-v1` the right name, or should it be
  `ime-state-v1`?
- Should an indeterminate query suppress conversion for the current word only, or
  until the next trusted boundary?
- Which target application is the reference oracle for composition state?
