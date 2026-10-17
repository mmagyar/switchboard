# Integrated verification and measurements

The Rust implementation runs natively without Bun, Node or an embedded JavaScript runtime. The measured server uses substantially less resident memory and starts sooner on this host. Throughput gains depend on concurrency and workload. The linked Rust/WASM browser application is larger than the equivalent original-client JavaScript bundle.

The Bun/TypeScript reference remains unchanged. Acceptance remains blocked. The latest question batch timed out; its automatic selections granted no policy approval.

## Current remediation status

The integrated remediation passed all-features check, Clippy with warnings denied, formatting, 179 library tests, four loadgen tests and two schema-runner tests. Bun passed 283 tests with 775 assertions in 6.34 seconds. The serialized feature/compiler/browser matrix completed in 43.13 seconds with no failures. It exercised the linked release application and all 19 public browser decoder probes.

| Acceptance evidence | Current result |
| --- | --- |
| Schema/parameter corpus | 3,741 cases, zero mismatches: unchanged 3,103-case seed plus 638 extensions |
| Explicit safety boundaries | 12 assertions passed |
| Strict HTTP corpus | 103 cases, two mismatches: BREW and string-backed BOM constructor behavior |
| Portable HTTP corpus | 102 cases, one BOM mismatch; only BREW is excluded explicitly |
| Unfiltered radix audit | 124 mismatches in 768 integrated-parser cases |
| Separate exact-radix checkpoint | 75 mismatches in 768 cases; commit `66cc24f` remains unintegrated |
| Generated Unicode lengths | Existing UTF-16 expectation remains unchanged pending explicit approval |
| Independent remediation review | Pending a resolved, green acceptance check |

Bun 1.4 rejects BOM-prefixed JSON in an in-memory string-backed Request, but accepts the same byte-backed Request and both string/byte bodies over actual HTTP. Rust's byte decoding accepts it. The constructor mismatch remains in the corpus, with complete expected/actual output; no new exclusion or normalization was added. The expanded corpus exposed this distinction after the component BOM correction.

All four policy decisions remain asked: BREW handling, generated Unicode lengths, large-radix conversion and the BOM constructor difference. The radix mismatches are within the finite input domain, not approved resource-bound exceptions. The currently configured CI portable gate remains red on BOM; hosted Actions has not run. Server performance tables below describe the pre-remediation build and require refresh before claims about the current binary.

## Feature evidence

| Surface | Verified behavior |
| --- | --- |
| Routing | Seven methods, numeric/string and optional params, static-first backtracking, aliases/prefixes, custom defaults, duplicate/conflicting paths and bodyless HEAD |
| Validation and URLs | Stripping/strict objects, arrays, records, heterogeneous tuples, open objects, sticky work exhaustion, unions/wrappers/defaults/catches, recursive generated schemas, checked coercion, numeric-key ordering and complete issues |
| Typed contracts | serde/schemars shared types, `route!`, compile-fail fixtures, actual handler binding and renamed dependency compilation |
| Authorization/errors | Authorization before parsing, 401/403 distinction, request/not-found/internal errors, negotiation, custom parsers/HTML formatters and error logging |
| Output/streaming | Validated JSON, stripping, warning-and-retain invalid output, nested deferred values, real streaming bodies, custom status/headers/redirects and owned background cleanup |
| Multipart | File identity and retained metadata/bytes through success and error context; lazy 128 MiB default server cap maps parser limit errors to 413; application file/concurrency policy remains necessary |
| HTTP hosting | Live endpoints, access logs, text/binary transport MIME inference, failed-start cleanup, pending-request cancellation, socket shutdown and listener reuse |
| TLS/WebSockets | Trusted self-signed TLS/WSS, provided PEMs, arbitrary upgrade paths, multiline log delivery, exact RELOAD broadcasts and owned shutdown |
| Logging | Owned file logger, argument encoding, file tailing, notifications, safe callback-triggered closure and explicit cleanup |
| Native client | Typed and unchecked calls against real peers, settings/credentials, auth callbacks, absent/empty/text/JSON bodies and distinct transport/HTTP/validation failures |
| Maud | Optional escaped server templates, real HTML endpoint and escaping mutation proof; no required browser runtime or hydration |
| Rust browser | Shared typed route calls, actual fetch validation, unchecked JSON, exact original-client fetch/validation messages, counter/listener disposal, socket close and RELOAD navigation |

The integrated component gates passed with Rust 1.99.1 and Bun 1.4.0:

- Core all-features tests: 179 library tests, four loadgen tests and two schema-runner tests passed.
- Macro suite: eight unit tests plus one trybuild group containing four compile/compile-fail fixtures passed.
- Browser suite: 15 native tests passed. Core and browser WASM checks passed, and the linked release application ran in Chromium 148.0.7778.96.
- Original and tooling Bun suite: 283 passed, zero failed, 775 assertions, 6.34 seconds.
- TypeScript checks passed for the original source, HTTP compatibility, schema compatibility, benchmark and browser tooling.
- Rust format, all-target Clippy, no-default/client-only/server-only checks and renamed-consumer checks passed.
- Rust 1.85.1 checked all core features/targets, the browser WASM crate and the public decoder WASM consumer successfully.

The browser journey checked real responses and visible output, exact error text against the original client, log frames, navigation type `reload`, closed sockets and a counter that remains unchanged after disposal. Desktop (1100px) and mobile (360px) screenshots were inspected; text and controls remain legible without horizontal overflow. Browser CORS, cross-origin cookies, HTTPS/WSS and non-Chromium behavior remain outside this particular browser journey. Separate native tests exercise TLS/WSS.

Focused mutation proofs cover routing, escaping, coercion, authorization, credentials, macro inference/order, record keys, client output validation, WebSocket behavior, strict comparison and benchmark accounting. The MIME regression failed against the missing live header before its fix. Saved live evidence reproduced the invalid-header error-message difference before fetch-owned validation fixed it.

## Strict comparisons and explicit boundaries

The schema/parameter gate passed **3,741 cases**, comprising the preserved 3,103-case seed and 638 extensions. These include strict and generated deny-unknown objects, nested strict issues and tuple-union continuation. The independent boundary audit passed **12 cases**. Complete issues, messages, URLs, missing values and array order remain observable.

The HTTP/definition/URL checker produced:

```json
{"cases":103,"mismatches":2}
```

`method-unsupported` retains Bun's BREW-to-GET constructor behavior versus Rust's 405. `request-leading-bom` retains the string-backed constructor distinction described above. Both strict and portable commands exit 1. The opt-in portable result is:

```json
{"cases":102,"mismatches":1,"excludedRuntimeCases":["method-unsupported"]}
```

The exclusion remains limited to BREW; BOM and radix evidence are not normalized away. The separate unfiltered radix audit remains red. Universal parity and final acceptance remain unproven.

[README.md](README.md#compatibility-evidence-and-limits) documents the approved finite-JSON, Unicode, query-expansion and validation-work boundaries. Generated unsupported schema constraints fail explicitly. Native logging is explicit rather than a global console patch. Native code reload requires rebuilding/restarting; the browser reload signal is supported. Native clients have no browser CORS/SameSite context.

## Historical pre-remediation measurement protocol

Measurements ran on 2026-10-16, Linux 6.8.0-146-generic x86_64, AMD EPYC-Rome Processor, four visible logical CPUs with allowed CPU list 0-3. Rust was 1.99.1, Cargo 1.99.1, LLVM 23.1.1 and Bun 1.4.0. CPU quota fields were unavailable; that does not establish an unlimited or dedicated host. No CPU affinity isolation was applied.

The release build used thin LTO, one codegen unit and stripped debug information. Five routes used the real router, authorization, input/output validation and formatting; HTML used Maud. Response sizes were 9 to 47 bytes and the POST request was 34 bytes. The harness used fresh processes, three alternating samples per implementation, 500ms warmup and 1000ms measurement per workload, at concurrency 1, 16 and 64. Every warmup and measured response passed exact status, Content-Type and body-byte checks. All runs completed without transport errors or mismatches.

The tables show per-metric medians across three samples. CPU is server user+system microseconds per completed request, quantized at 100 ticks/second. RSS is the median of each phase's 25ms sampled maximum, in MiB. Latency covers request construction through full response validation; logarithmic histogram percentiles have at most 1% bucket overestimate. Startup is spawn-to-readiness, excluding builds.

### Concurrency 1

Median startup was Bun **45.63ms**, Rust **1.52ms**.

| Workload | Bun req/s | Rust req/s | Ratio | Bun p50/p95/p99 ms | Rust p50/p95/p99 ms | Bun/Rust CPU us/request | Bun/Rust RSS MiB |
| --- | ---: | ---: | ---: | --- | --- | --- | --- |
| Static | 8267 | 8493 | 1.03x | 0.12/0.17/0.21 | 0.11/0.17/0.21 | 93.13/52.21 | 70.89/4.38 |
| Numeric path | 7673 | 8171 | 1.06x | 0.12/0.18/0.23 | 0.12/0.17/0.21 | 88.61/55.78 | 63.02/4.39 |
| Nested query | 6323 | 7440 | 1.18x | 0.15/0.22/0.28 | 0.13/0.19/0.24 | 118.60/66.09 | 62.71/4.44 |
| Validated POST | 7285 | 7815 | 1.07x | 0.13/0.19/0.24 | 0.12/0.18/0.22 | 86.47/62.35 | 62.28/4.44 |
| HTML | 7390 | 7861 | 1.06x | 0.13/0.19/0.24 | 0.12/0.18/0.23 | 91.34/59.78 | 64.08/4.44 |

### Concurrency 16

Median startup was Bun **43.75ms**, Rust **1.50ms**.

| Workload | Bun req/s | Rust req/s | Ratio | Bun p50/p95/p99 ms | Rust p50/p95/p99 ms | Bun/Rust CPU us/request | Bun/Rust RSS MiB |
| --- | ---: | ---: | ---: | --- | --- | --- | --- |
| Static | 34098 | 42112 | 1.24x | 0.42/0.90/1.30 | 0.34/0.72/0.99 | 31.07/22.31 | 64.27/4.75 |
| Numeric path | 28191 | 38047 | 1.35x | 0.50/1.03/1.48 | 0.39/0.75/0.98 | 41.12/24.96 | 70.44/4.82 |
| Nested query | 16806 | 28007 | 1.67x | 0.84/1.72/2.46 | 0.53/0.85/1.01 | 64.80/35.32 | 68.50/4.94 |
| Validated POST | 23011 | 31023 | 1.35x | 0.62/1.24/1.79 | 0.48/0.82/1.08 | 46.08/31.57 | 69.31/4.95 |
| HTML | 27414 | 35884 | 1.31x | 0.52/1.07/1.57 | 0.41/0.80/0.97 | 40.08/27.30 | 74.00/4.99 |

Across the three samples, throughput min/max was Bun/Rust: static 30478..38584 / 38494..45074; numeric 24893..30813 / 37245..40066; query 15244..17135 / 27722..29538; POST 22541..26564 / 30264..32128; HTML 23554..27595 / 32329..37306 req/s.

### Concurrency 64

Median startup was Bun **42.62ms**, Rust **1.34ms**.

| Workload | Bun req/s | Rust req/s | Ratio | Bun p50/p95/p99 ms | Rust p50/p95/p99 ms | Bun/Rust CPU us/request | Bun/Rust RSS MiB |
| --- | ---: | ---: | ---: | --- | --- | --- | --- |
| Static | 38572 | 47722 | 1.24x | 1.51/3.03/4.01 | 1.18/2.46/3.16 | 26.14/20.29 | 64.77/6.13 |
| Numeric path | 28478 | 41320 | 1.45x | 2.02/3.85/4.99 | 1.37/2.64/3.45 | 37.82/23.17 | 70.98/6.33 |
| Nested query | 17102 | 31358 | 1.83x | 3.39/5.40/7.58 | 2.00/2.89/3.85 | 62.37/31.81 | 68.44/6.52 |
| Validated POST | 24718 | 33990 | 1.38x | 2.39/4.01/5.40 | 1.79/3.22/4.13 | 40.77/28.77 | 69.61/6.67 |
| HTML | 28017 | 39351 | 1.40x | 2.14/3.70/4.94 | 1.56/2.56/3.32 | 38.09/25.06 | 74.22/6.74 |

Process-lifetime VmHWM maxima across the sample processes were Bun/Rust **72.87/4.48 MiB** at concurrency 1, **74.09/5.00 MiB** at 16 and **75.07/7.00 MiB** at 64. These include previous workloads and warmups and do not isolate an individual endpoint's allocation.

Single-connection throughput is close for several workloads. An earlier pre-final transport build measured approximately equal static throughput and worse Rust tail latency at concurrency 1. The final run's three-sample ranges still show substantial shared-host variability. Tiny closed-loop HTTP responses, client scheduling, connection pooling, fixed workload order and coordinated omission limit interpretation. These results do not predict WAN, TLS, database-heavy workloads or a universally faster runtime. No isolated router/template-only result is claimed.

## Browser artifact cost

The actual release Rust application links the shared typed contracts, schema validation, fetch, DOM ownership and WebSocket helper. Both applications passed the same live journey. Gzip level 9 was applied independently to each HTTP resource; shared server-rendered HTML/CSS, declaration files and source maps were excluded.

| Artifact | Raw bytes | Gzip bytes |
| --- | ---: | ---: |
| Linked WASM | 598367 | 172875 |
| Generated wasm-bindgen JS | 26440 | 5798 |
| Bootstrap | 268 | 188 |
| Rust total | **625075** | **178861** |
| Comparable minified original-client JS | **291595** | **66684** |

These are refreshed post-remediation artifacts. Startup API failure/recovery and back/forward interactions passed. Explicit persisted events exercised the reload branch; Chromium policy prevented a real cached-document restoration. Current desktop/mobile success and failure screenshots were inspected. Rust is about **2.68x** the gzip download in this representative application. Optional server-rendered Maud pages require **zero framework browser bytes** when they use no browser behavior. WASM compilation/startup and browser memory were not benchmarked, so byte counts establish no browser latency or CPU advantage.

## Reproduction and retained evidence

[bench/README.md](bench/README.md) contains the complete protocol and CLI. [browser/README.md](browser/README.md) contains linked-application build and browser commands. The release server is 4726848 bytes, SHA-256 `717ee573ffbf109608e2b8d149975305ea2ff17cfdb7c6c75ed07dacd8b3d143`. The loadgen SHA-256 is `4c239a2aee424fd53d3e9fbdfbf255cdce026e8aab54f1b6009402b1e9fbb343`.

The local `.tmp/` artifacts remain untracked:

| Artifact | SHA-256 or purpose |
| --- | --- |
| `final-rust-bench-c1.ndjson` | `635db387dbcf124f90958ec311af386a7aa2fa8850fd3b33536d9a50d1028224` |
| `final-rust-bench-c16.ndjson` | `6c6fa07522cd9ae653ff7006ecd4c1462bcb1145f20e34739a11638a88227512` |
| `final-rust-bench-c64.ndjson` | `0abaaba1b1292dc4aead6050ce30209f237a925bf6e00742d9dc88ea7a04e9f0` |
| `final-bench-summary.json`, `bench-table.md` | Validated per-workload medians, ranges and resource summaries |
| `final-performance.log`, `final-performance.status` | Build/run sequence and successful completion |
| `final-compat-strict.log`, `final-compat-portable.log` | Explicit strict mismatch and portable result |
| `final-schema-gate/`, `final-schema.log`, `final-bounds.log` | Schema/reference outputs and explicit-bound audit |
| `browser/` | Real browser evidence JSON, desktop/mobile screenshots and artifact sizes |
| `browser-final-live.log`, `browser-final-measure.log` | Exact matching UI messages and linked-application byte measurements |
| `rust-test.log`, `clippy.log`, `final-bun-test.log`, `extra-checks.log` | Component/integration checks and feature/compiler matrix |

## Independent review

Six independent reviews of `57b9e4f` completed and found actionable defects. Their reproduced schema, ownership, transport, hosting, client and browser findings drove the integrated remediation. Fresh independent re-review remains pending a resolved, green acceptance check. Policy-dependent findings and current radix/BOM mismatches remain visible; component completion grants no final review approval.

Current evidence is under `.tmp/final-reviews/`: `remediation-tools-{rust,bun,types}.log`, `integrated-matrix.{log,status}`, `matrix-*.log`, `integrated-{http-strict,http-portable,schema,bounds}.log`, `integrated-schema/`, `radix-integrated-{audit.log,results.ndjson,mismatches.ndjson}`, `bom-source-policy.{ts,log}` and `integrated-runner-hashes.txt`. Cargo validation was serialized with the shared external lock, local crates were forced to recompile, and binaries/WASM were preserved before external gates. An earlier apparent green run that omitted new tests was rejected as invalid shared-target evidence.
