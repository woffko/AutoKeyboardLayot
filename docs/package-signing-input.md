# Signing input boundary

`package_signing` is gated by the developer-only `signing-tools` feature. It is
not part of the ordinary application's feature set and does not read keys, files
or network resources.

Input is an unsigned format-1 envelope: packages contain `manifest` and typed
`components`; catalogs contain `catalog`. A signer/signature field is not accepted
on input. Duplicate and unknown fields are rejected by typed deserialization.
The manifest/catalog string is preserved as the exact signing document. The
signer ID comes separately from trusted caller policy and is bound at preparation;
this also makes preflight envelope sizing match the actual signer ID.

Before a real private key can be used, `prepare` applies the existing runtime
package/catalog validators using an in-memory synthetic validation signature.
The public, deterministic preflight key (using the configured identifier only
inside the temporary validation context) is never a release trust anchor and its
temporary envelope is not returned or written. Invalid content fails before any
private-key handling. The protected English base is not an external candidate.

`signing_message` supplies the same domain prefix used by the runtime verifier.
`finalize` inserts the supplied signature, rechecks it against the supplied trust
policy, and validates the full envelope again, including catalog expiry. Only
then are output bytes returned. It does not grant filesystem installation,
linguistic/IME readiness, licensing approval or permission to publish. Catalog
asset availability and release-history discipline still belong to the release
workflow.

On 2026-09-12 the native utility signed 13 UI-only revision-1 candidates using
`pkg-20260912-01`; a separate Linux runtime verifier accepted every signature.
Artifacts and exact hashes are recorded in
`target/ui-package-candidates-20260912-01/verification.json` (317541 bytes total).
A local revision-1 candidate catalog was subsequently signed and checked against
all 13 local artifacts: `catalog.aklc` in that same directory, 3706 bytes,
SHA-256 `98e438256b33b92bb2f4946f5ea13823933e80808a3e3b581d40e32a51b805f9`.
It pins the planned `ui-candidates-20260912-r1` tag in the main repository and
expires seven days after issuance. Those remote assets are not published or
verified reachable. Changing expiry or any catalog data requires a new catalog
generation; do not overwrite an accepted generation with a different document.

The real signed candidates also passed the isolated Linux store rehearsal in
`examples/rehearse_ui_packages.rs`: all 13 optional catalogs loaded with English
as the fourteenth UI locale, input stayed English-only, removal/reimport and
reload succeeded, stale confirmation and corrupted input were refused. Receipt:
`target/ui-package-candidates-20260912-01/store-rehearsal-linux.json`. The fresh
temporary store was removed; no live profile was used. The same scenario also
passed on native Windows, with receipt `store-rehearsal-windows.json` in that
artifact directory. Actual installer UI/typing acceptance remains separate.

## Native developer utility

`sign-language-package` is a separate binary requiring `signing-tools`; ordinary
agent builds exclude it. It accepts `package|catalog ABS_INPUT ABS_OUTPUT`, loads
only the embedded release identity, validates input before DPAPI access, and
revalidates the signed output before an exclusive, flushed write. It never
publishes, replaces existing outputs, or exports plaintext key material.

Run only in the original Windows key owner's profile, with reviewed input and a
trusted local output directory. Reparse paths are refused, but ancestor metadata
checks are not a race-proof sandbox against hostile same-user processes. Output
contains public signed data, not key material; restricting its ACL to SYSTEM and
the signer is not a substitute for reviewing publication. A write failure may
leave a partial new output: inspect it before any manual retry. Portable recovery,
paging/crash-dump protection, runtime signing tests and release acceptance are
separate gates. Do not distribute this developer binary with the agent.

### Native fixture verification (2026-09-12)

Three Windows tests passed: strict fixed DER and public-key identity, CurrentUser
DPAPI roundtrip with wrong-entropy/corrupt-ciphertext refusal, and bounded regular
file reads. All used public synthetic test material; none accessed the production
key. Receipt: `target/signing-native-tests-20260912-01.json`. This establishes the
tested native primitives. The later real UI-package signature verification above
is additional evidence; neither establishes release acceptance.

## Preparing real package inputs

`cargo run --locked --offline --features signing-tools --example
prepare_language_package -- RECIPE NEW_UNSIGNED_OUTPUT` builds an unsigned
envelope from explicit UTF-8 component files. Paths resolve relative to the recipe.
The recipe is trusted developer input, not a downloaded installation instruction.
Example shape (the referenced files must actually exist):

```json
{"package_id":"ru-RU","revision":1,"ui_locale":"ru","components":{
  "ui":"ru.json","license":"LICENSE.txt","notice":"NOTICE.txt"}}
```

Combined/input packages additionally supply `input_pack` and all four input
components. The tool computes byte-exact lengths/hashes, invokes the full runtime
preflight and exclusively writes a new output. It does not generate license
grants, sign anything, install or publish. License/notice sources are mandatory;
an arbitrary nonempty text satisfying schema is not evidence of distribution
rights. The current translation drafts are in `data/package-locales`; the owner
selected MIT for the program and these translations on 2026-09-12. Their recipes
must include the root `LICENSE` and translation provenance/acceptance notice.
Third-party dictionary and dependency license gates remain separate.
