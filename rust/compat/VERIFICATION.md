# Verification evidence

The baseline before edits passed `bun run check` and `bun test`: 256 tests, 0 failures, 463 assertions, 8 files.

The compatibility increment passed on Bun 1.4.0:

```text
bun run check
$ tsc --noEmit

bunx tsc -p rust/compat/tsconfig.json
(exit 0, no diagnostics)

bun test
268 pass
0 fail
666 expect() calls
Ran 268 tests across 9 files.

bun rust/compat/check.ts bun rust/compat/reference.ts
{"cases":90,"mismatches":0}
```

The separate compat tsconfig is required because the repository's root tsconfig includes only src. It extends the exact existing strict compiler options and adds compat files without modifying the root configuration.

`protocol.spec.ts` adds 12 tests. They validate all corpus records/results; exercise original methods, auth precedence, prefix form rewriting, deferred JSON and formatter inputs, exact warning issues/messages, stripping, definition errors and URL generation; reject invalid NDJSON; test exact comparison semantics; and execute the actual checker CLI against successful, semantically changed, malformed, crashing and missing-result candidates.

A real CLI candidate regenerated reference results and changed only `received number` to `received boolean` inside one warning message. The checker exited 1, reported `{"cases":90,"mismatches":1}`, and retained complete expected/actual validation text. Invalid output, exit 7 and no output each caused exit 1. Those checks are repeatable in the test suite.

The mutation proof temporarily replaced `compareResults`'s `return mismatches` with `return []`. The exact command `bun test rust/compat/protocol.spec.ts -t 'object key order'` failed at its validation-message comparison with expected length 1, received length 0. The file was restored from a backup in a separate command. Both typechecks, the full test suite and CLI self-comparison then passed again. Local raw command output is stored under the worktree's ignored `.tmp/compat-*.log` files; generated reference output is `.tmp/compat-reference.ndjson`.

The 90-case self-comparison validates the tooling only. A Rust candidate was not available in this component worktree, so Rust/Bun parity remains the parent's integration gate. No benchmark was implemented or run, and there are no measured performance claims. Network streaming timing, rejected futures, client transport errors, TLS, WebSockets and browser tests remain separate gates listed in PROTOCOL.md. The parent owns independent review and integration. No original source, package manifest, Rust implementation or root README was changed.
