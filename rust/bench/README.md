# Reproducible full-pipeline HTTP measurements

`run.ts` drives a native Rust `loadgen` against the original Bun library and a supplied native Rust server. It records raw NDJSON without declaring a winner. The Bun process is test tooling only. Neither the native server nor the native client embeds Bun or JavaScript.

## Server contract

The candidate argument is an executable path, not a shell command. The orchestrator invokes it as:

```text
/path/to/bench-server --port 0
```

Bind HTTP/1.1 on `127.0.0.1`, let the OS choose a free port, and write exactly one UTF-8 NDJSON readiness record to stdout after the listener is accepting connections:

```json
{"ready":true,"port":12345}
```

The port must be the actual bound port. Flush stdout. All diagnostics go to stderr. Emit no additional stdout records. Exit on SIGTERM or tolerate a forced SIGKILL after a one-second grace period. Each server must be a single process with no subprocesses. Readiness is bounded by `--startup-timeout-ms` (default 10000). Startup time runs from immediately before process spawn through receipt of readiness, not build time.

Register all five routes through the public route definition, handler definer and router pipeline. Resolve the user as the `Authorization` header, then authorize permission `bench` only for user `Bearer bench-token`. Missing or different tokens are unauthenticated (401). Do not replace handlers with cached responses or bypass parsing, auth, input/output validation or formatting. Rust uses the port's `RouteDef`, `HandlerDefiner`, `Router`, and HTTP hosting, plus Maud for the HTML formatter. Bun uses the original `define`, `RouteHandlerDefiner`, `Router`, and `Bun.serve`.

All bodies below are exact UTF-8 bytes with no trailing newline or spaces. JSON success content type is exactly `application/json`. HTML content type is exactly `text/html; charset=utf-8`.

| Workload | Request | Status | Exact response body |
| --- | --- | --- | --- |
| `static` | GET `/static` | 200 | `{"ok":true}` |
| `numeric-path` | GET `/items/42` | 200 | `{"id":42}` |
| `nested-query` | GET `/search?filter.active=true&values.0=1&values.1=2` | 200 | `{"filter":{"active":true},"values":[1,2]}` |
| `validated-post` | POST `/items` | 201 | `{"name":"sample","count":2}` |
| `html` | GET `/html/42` | 200 | `<main><h1>Item 42</h1><p>Switchboard</p></main>` |

POST sends content type `application/json` and exact body `{"name":"sample","tags":["a","b"]}`. All requests send `Authorization: Bearer bench-token` and an Accept header matching the expected response content type.

Equivalent runtime schemas and handlers are:

- `/static`: empty params object, output object `{ok: boolean}`, handler returns `{ok:true}`.
- `/items/:id`: params and output object `{id: number}`, handler returns its validated numeric id. Use an explicit numeric `id` schema, not string auto-derivation.
- `/search`: params and output object `{filter: {active: boolean}, values: number[]}`, handler returns validated params. Nested query coercion belongs to Switchboard.
- POST `/items`: empty params object, body object `{name: string, tags: string[]}`, output object `{name: string, count: number}`. Handler returns the validated name and tags length.
- `/html/:id`: params and output object `{id: number}`, handler returns its validated numeric id. The format callback renders `<main><h1>Item {id}</h1><p>Switchboard</p></main>` using the formatter pipeline. The Bun interpolation is safe because `id` is a validated number; Maud renders the equivalent Rust value.

Schemas use ordinary object unknown-key stripping, without additional coercion/refinement rules. The fixture table tests the success bytes; auth and malformed-input tests additionally check the Bun pipeline. Cross-language compatibility tests remain the authority for general error semantics.

## Build and run

Use the shared toolchain location and a dedicated build target. The integrating manifest must declare the native client binary with `required-features = ["client"]`:

```toml
[[bin]]
name = "loadgen"
path = "src/bin/loadgen.rs"
required-features = ["client"]
```

This tooling change does not edit the manifest. Its binary uses Tokio's multithreaded runtime, which the existing default `server` feature enables. Build with default features enabled, or explicitly enable both `server` and `client`.

Run from the repository root:

```sh
export RUSTUP_HOME=/home/dev/switchboard-worktrees/rust-implementation/.tmp/rustup
export CARGO_HOME=/home/dev/switchboard-worktrees/rust-implementation/.tmp/cargo
export PATH="$CARGO_HOME/bin:$PATH"
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_TARGET_DIR="$PWD/.tmp/bench-target"
cargo build --manifest-path rust/Cargo.toml --release --features maud --bin bench-server --bin loadgen
bun rust/bench/run.ts \
  --candidate "$CARGO_TARGET_DIR/release/bench-server" \
  --loadgen "$CARGO_TARGET_DIR/release/loadgen" \
  --samples 3 --duration-ms 1000 --warmup-ms 500 --concurrency 16 \
  --timeout-ms 5000 --startup-timeout-ms 10000 \
  --output .tmp/rust-bench.ndjson
```

The `bench-server` source and manifest registration are supplied by the integrating implementation. Do not substitute an echo server or router-only microbenchmark. Do not publish debug-build results as release measurements. The tool hashes executable bytes, but cannot infer compiler flags from an arbitrary binary; retain the exact build command with the artifact.

`--candidate` and `--loadgen` are required. All other flags use the defaults in the example. Samples are bounded to 3..100, concurrency to 1..1024, durations to 1..300000ms and timeouts to 1..60000ms. Existing output files are rejected to preserve previous evidence. Progress is emitted on stderr. Raw records are written incrementally, including failure records, and flushed on exit. A failed run is not a comparative result.

Each sample starts fresh Bun and Rust processes. Implementation order alternates Bun/Rust, Rust/Bun, Bun/Rust for three samples. Each process runs all five workloads in the documented order. Every workload gets a separate warmup loadgen invocation immediately before the measured invocation. Warmup records are tagged and retained, never folded into measurement statistics. All responses in both phases must pass exact status, content-type and byte checks. Redirect following, proxy use and request retries are disabled. Any mismatch, transport error, malformed result, changed invocation parameters, zero successful requests or nonzero client exit fails the run. Server/readiness/client deadlines and joined SIGTERM/SIGKILL cleanup also apply on errors.

## Native loadgen interface

```sh
.tmp/bench-target/release/loadgen \
  --base-url http://127.0.0.1:12345 --workload numeric-path \
  --duration-ms 1000 --concurrency 16 --timeout-ms 5000
```

The base URL must be a credential-free HTTP loopback origin with an explicit non-default port and no path/query/fragment. Workload names are exactly those in the table. Defaults are duration 1000ms, concurrency 16 and request timeout 5000ms. Duplicate and unknown flags fail.

Each worker owns at most one in-flight request. All workers share an HTTP/1.1 connection pool and a barrier start. Requests start until the monotonic duration expires; in-flight requests drain within the per-request timeout. Throughput is successful responses divided by actual elapsed time including drain. The first error stops new requests across workers; all in-flight workers are joined. Response bodies are bounded to 65536 bytes. The native process prints one JSON result and exits zero only for a nonempty, error-free run. Argument/startup errors emit stderr and exit nonzero.

The result includes workload, exact duration/concurrency/timeout, elapsedSeconds, completed, transportErrors, mismatches, requestsPerSecond, latency `{p50Ms,p95Ms,p99Ms,histogramRelativeError}`, and up to eight error details. Latency covers request construction, queueing, network, complete body read and verification. Successful latencies use nearest-rank percentiles from a bounded logarithmic nanosecond histogram. Reported bucket upper edges overestimate an observed latency by at most 1%. Failed runs can have zero percentiles and must be discarded.

## Raw evidence and interpretation limits

The NDJSON contains environment, startup, warmup/measure sample, completion or failure records. Environment includes OS/kernel, CPU model/count/affinity, Bun/rustc/Cargo versions, Linux clock tick rate, cgroup membership, v2 CPU quota (`cpuMax` plus mount-root `rootCpuMax`), legacy v1 quota/period where present, cpuset, exact invocation/commands/parameters, executable sizes/hashes, route count, and request/response payload byte counts. Nullable cgroup fields mean unavailable, not unlimited. Namespace mounts and inherited ancestor cgroups can impose limits beyond the files recorded; retain the deployment context.

Per-phase CPU is the server process's `/proc/<pid>/stat` user+system tick delta. RSS comes from `/proc/<pid>/status`, with before/after values and a 25ms sampled maximum. `processPeakRssBytes` is VmHWM since process startup: it includes previous workloads and warmups. It is not an isolated workload allocation measurement. CPU accounting is quantized by CLK_TCK and includes work between boundary reads. Sampling can miss transient RSS spikes. Process RSS includes runtime, allocator and shared mapped pages; it is neither unique physical memory nor total host/container memory. No child-process memory is included. Tool startup and HTTP connection setup affect short samples.

This is closed-loop, fixed-concurrency loopback traffic on the same host. It measures complete tiny-response handler pipelines, not WAN, TLS, browser, router-only dispatch or isolated template rendering. Client CPU, response validation, server/client scheduling, cgroup CPU quotas, connection reuse, ephemeral ports, OS buffers and loopback bandwidth can bottleneck the result. Fixed concurrency does not model offered-load queue growth and has coordinated-omission limitations. Run concurrency sweeps and longer samples, inspect client/server CPU separately, isolate cores/hosts when appropriate, and preserve negative or inconclusive results. Alternating process order reduces order bias without eliminating shared-host noise or the fixed within-process workload ordering.

Browser JS/WASM byte measurements remain an explicit pending artifact in the raw environment record. No browser byte advantage or Rust/Bun performance conclusion is claimed before real release-server measurements.

## Checks

```sh
bun run check
bunx tsc --project rust/bench/tsconfig.json --noEmit
bun test
cargo test --manifest-path rust/Cargo.toml --bin loadgen
```

The dedicated strict tsconfig includes benchmark tooling because root `bun run check` includes only `src/`. Bun tests cover real HTTP fixture bytes and schema/auth failures, malformed native results, accounting parsers, CLI bounds and readiness/termination cleanup. Rust tests cover CLI validation, histogram precision, actual HTTP response validation and no-retry connection failure. Tooling verification is separate from real candidate performance evidence.
