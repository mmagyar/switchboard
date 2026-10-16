# Strict schema and parameter differential gate

`check.ts` compares the unchanged Bun/Zod implementation with the real native `schema-runner` binary. The reference imports `src/routeParser.ts`, `src/urlUtils.ts` and `src/util.ts` directly. The Rust binary calls the library's public schema and parameter APIs, including the checked `coerce_form` result. No implementation is copied into either runner.

## Reproduction

Run from the repository root with the existing Bun dependencies installed:

```sh
export RUSTUP_HOME=/home/dev/switchboard-worktrees/rust-implementation/.tmp/rustup
export CARGO_HOME=/home/dev/switchboard-worktrees/rust-implementation/.tmp/cargo
export PATH="$CARGO_HOME/bin:$PATH"
export CARGO_PROFILE_DEV_DEBUG=0

cargo build --manifest-path rust/Cargo.toml --bin schema-runner
bun x tsc --project rust/compat/schema/tsconfig.json
bun rust/compat/schema/check.ts .tmp/schema-gate rust/target/debug/schema-runner
bun rust/compat/schema/bounds.ts .tmp/schema-gate rust/target/debug/schema-runner
cargo test --manifest-path rust/Cargo.toml --all-features --lib --bin schema-runner
cargo check --manifest-path rust/Cargo.toml --all-features --bin schema-runner
cargo clippy --manifest-path rust/Cargo.toml --all-features --bin schema-runner -- -D warnings
bun run check
bun test
```

The strict gate exits zero only when every result matches. It writes the exact generated inputs, reference output, Rust output, stderr logs, full mismatch records and summary into the specified evidence directory. Process failures, malformed output, wrong result counts and output-schema failures exit nonzero. Child processes have a 30-second bound, and the boundary audit bounds each native invocation to five seconds. Successful output comparison ignores JSON object member order and equivalent decimal notation only. Array order, absent versus null values, every issue field, the full error-message string and URL strings remain exact. Numeric comparison preserves decimal values beyond JavaScript's integer precision; it does not round candidate output through `JSON.parse` before comparison.

## Corpus and DSL

`source-cases.ndjson` is the unchanged 3103-case scratch corpus, retained as the deterministic seed. Its SHA-256 is `12cb704c1b48abfe5cfae096df8395b00601454e46478833fea0f949ab07e732`, matching both the parent scratch corpus and the original schema worker corpus. The original operations, ordering, missing `input` fields and numeric JSON lexemes are retained. `generate.ts` validates this seed with Zod and appends 564 deterministic cases. It can also emit the complete 3667-case corpus to stdout:

```sh
bun rust/compat/schema/generate.ts > .tmp/schema-cases.ndjson
```

Every case uses `op`, `schema`, and the operation's input fields:

| Operation | Additional fields | Result |
| --- | --- | --- |
| `parse` | optional `input` | success with explicit presence and data, or full issues and message |
| `form` | object `input` | original numeric/boolean form passes followed by validation |
| `coerce` | object `input` | original numeric/boolean form passes without validation |
| `url` | `path`, absolute `url` | original path/query extraction, coercion and validation |
| `def` | `path`, `input` | generated URL or exact original error message |

Schema nodes retain the original scratch `{ "type": ..., "value": ... }` encoding. Unit types are `unknown`, `string`, `number`, `boolean`, and `file`. `literal` carries a JSON primitive; `enum` carries strings; `object` carries a field-to-schema map. `array`, `record`, `optional`, `nullable`, and `readonly` carry one schema. `tuple` and `union` carry arrays of schemas. `default` and `catch` carry `[schema, fallback]` pairs.

The added `derived` fixture names are `map`, `tuple`, `open`, `tree`, `mutual`, and `containers`. The Rust runner builds these from actual Rust types through `Schema::from_type`, including `BTreeMap`, heterogeneous tuples, flattened catchall fields and self/mutually recursive structs. The reference constructs equivalent original Zod schemas. Recursive Zod shape getters represent references without changing the original form traversal. Schemars `anyOf(ref, null)` is represented by `z.union([object, z.literal(null)])`, preserving its union issues rather than substituting `.nullable()` errors. The derived numeric fields use `f64`, whose finite JSON domain is representable by `z.number()`.

Extensions cover nested records, empty and heterogeneous tuples, missing and invalid tuple elements, oversized tuples with combined element issues, trailing optional/default elements, open objects, unknown-field stripping, recursive valid and invalid data, nested form coercion, URL generation and query parsing. The 76 missing-input seed cases are preserved. Native tests separately pin the difference between missing input and explicit JSON null.

The corpus uses finite JSON values. Successful File validation and unresolved Promise identities cannot be represented by this transport. File rejection cases are covered; the library's identity-specific tests remain responsible for File/Promise success behavior. This gate does not claim to represent every Zod construct or every Rust-derived numeric constraint.

## Explicit bounds

`bounds.ts` runs 12 separate assertions and persists both implementations' complete results in `boundaries.ndjson`. These cases are outside the strict parity corpus when an approved resource or finite-JSON boundary is crossed. They are never rewritten into matching results.

| Boundary | Accepted side | Rejected side and exact Rust message |
| --- | --- | --- |
| Sparse query index | 4095 | 4096: `Query array index exceeds 4095` |
| Query nesting | 64 key components | 65: `Query key nesting exceeds 64 levels` |
| Query allocation budget | 65536 counted nodes | 65537: `Query structure exceeds 65536 nodes` |
| Schema nesting | recursive fixture within 128 schema levels | beyond 128: `Schema nesting exceeds 128 levels` |
| Schema visits | root array plus 65535 numbers | root plus 65536: `Schema validation exceeds 65536 nodes` |
| Finite JSON numbers | regular finite corpus | raw `1e400`: native JSON decoder rejects `number out of range`; original Zod rejects the resulting Infinity |
| Unicode scalar strings | regular Unicode corpus | lone UTF-16 `\ud800`: native JSON decoder rejects `unexpected end of hex escape`; original Zod string accepts it |

The depth fixture uses 31 versus 32 recursive `next` edges. Optional, union and lazy-reference visits count toward the schema limit, independently of JSON structural depth. Query budget fixtures count sparse allocations rather than just query-pair count. The Rust JSON decoder also has serde_json's default nesting limit; these fixtures stay below it and exercise the actual library validation bounds.

## Verification evidence

The checked run used Bun 1.4.0, rustc 1.99.1 and cargo 1.99.1. It produced 3667 parity cases with zero mismatches and 12 successful boundary assertions. The six tooling tests cover corpus stability, complete issue/message/URL comparison, lossless numbers, Zod boundary rejection, artifact persistence and failing subprocesses. The native binary has two tests for null/absence and checked coercion.

The extended corpus caught three oversized-tuple mismatches that the original 3103 cases missed. The core-owner fix is commit `f2b6a9e`, cherry-picked locally as `0cc4c91`. Its regression tests and the extended optional/default tuple matrix are green. Reference nullable-versus-union errors were resolved by representing the generated schema's actual `anyOf` semantics in Zod, with no output rewriting.

A real-output mutation changed the first Rust issue message from `Invalid input: expected string, received null` to `MUTATED schema diagnostic`. Running the same strict checker through that wrapper exited 1 with exactly one mismatch across all 3667 cases. The mutation evidence lives in `.tmp/schema-mutation/` and `.tmp/schema-mutation.log`; the original binary and product source were unchanged. The normal gate subsequently returned zero mismatches.

Failure-path testing initially exposed an unhandled stdin `EPIPE` when a candidate exited before consuming the corpus. The checker now awaits input completion while draining stdout/stderr concurrently, preserves output artifacts and reports the real nonzero exit. Tests also assert the distinct malformed-JSON, wrong-count and missing-data errors after a candidate consumes its input.
