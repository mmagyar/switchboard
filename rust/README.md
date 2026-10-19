# Switchboard in Rust

This directory contains a native Rust implementation beside the unchanged Bun/TypeScript reference in `../src`. The server and native client run without Bun, Node, an embedded JavaScript engine or hydration. Optional browser behavior is Rust compiled to WASM. Maud is an optional server-template renderer.

The implementation uses safe authored Rust (`#![forbid(unsafe_code)]`). Established dependencies include native cryptography and may use unsafe internals. This is a language-safety guarantee, not a claim that every dependency or application is free of vulnerabilities.

Acceptance remains blocked on explicit decisions for BREW handling, generated Unicode lengths, large-radix rounding and Bun's string-backed BOM constructor behavior. Automatic question-timeout selections granted no exceptions. The integrated remediation passes compilation, unit, feature, MSRV and browser checks; strict HTTP and the unfiltered radix audit remain red. [VERIFICATION.md](VERIFICATION.md) records the current results and separates historical measurements.

## Build and test

From the repository root, with Rust and Bun installed:

```sh
cargo check --manifest-path rust/Cargo.toml --all-features
cargo test --manifest-path rust/Cargo.toml --all-features
cargo clippy --manifest-path rust/Cargo.toml --all-features --all-targets -- -D warnings
cargo fmt --manifest-path rust/Cargo.toml --check
cargo check --manifest-path rust/typecheck/Cargo.toml --locked
cargo test --manifest-path rust/macros/Cargo.toml --locked
cargo test --manifest-path rust/browser/Cargo.toml --locked
rustup target add wasm32-unknown-unknown
cargo check --manifest-path rust/Cargo.toml --no-default-features --target wasm32-unknown-unknown --lib
cargo check --manifest-path rust/browser/Cargo.toml --target wasm32-unknown-unknown --locked
bun run check
bunx tsc -p rust/compat/tsconfig.json
bunx tsc -p rust/compat/schema/tsconfig.json
bunx tsc -p rust/bench/tsconfig.json
bun test
```

Default features are `server` and `client`. `maud` adds rendering without changing routing or validation. `default-features = false` selects the HTTP contracts, route definitions, schema/parser, typed contracts and macros without hosting or reqwest. The browser crate uses the core with server/client features disabled. The package uses edition 2024 and declares Rust 1.85 as its minimum; verification records identify the compiler actually exercised.

## Typed routes and authorization

Use serde and schemars derives as the shared contract. There is no separate hand-maintained output schema for a typed route:

```rust
use serde::{Deserialize, Serialize};
use schemars::JsonSchema;
use switchboard::{Authorization, HandlerDefiner, Method, Router, TypedRoute};

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Params { item_id: f64 }

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Item { item_id: f64, title: String }

fn router() -> Result<Router, switchboard::DefinitionError> {
    let route = TypedRoute::<Params, (), Item, _>::new(
        Method::Get, "/items/:itemId", "public", vec![],
    )?;
    let define = HandlerDefiner::new(
        |_| async { Ok(()) },
        |(), permission: &str, _| async move {
            Ok(if permission == "public" {
                Authorization::Ok
            } else {
                Authorization::Forbidden
            })
        },
    );
    let mut router = Router::new();
    router.add_route(define.bind_typed(route, |params, _body, (), _prefix| async move {
        Ok(Item { item_id: params.item_id, title: "Switchboard".into() })
    }))?;
    Ok(router)
}
```

`get_user` receives `RequestContext`; authorization runs before parameter/body parsing. Return `Authorization::Unauthenticated` for 401 and `Forbidden` for 403. The permission type is application-defined. Typed handlers receive `(P, Option<B>, User, Option<String>)`. Body routes validate their body before invoking the handler. No-body routes supply `None`. Prefixes are server-only; the route retains its canonical path for clients.

`route!` generates an inferred path-parameter type:

```rust
let route = switchboard::route!(get, "/items/:itemId/:slug?", "public", String)?;
// bind_typed infers params.itemId: f64 and params.slug: Option<String>.
# Ok::<(), switchboard::DefinitionError>(())
```

The macro checks literal paths, optional ordering, parameter names and prefixes at compile time. `:itemId` and `:item_id` infer numbers. See [macros/README.md](macros/README.md) for body methods, keyword fields and renamed dependencies. Named shared structs are preferable when clients need to construct parameter values or contracts span modules.

`RouteDef` and `HandlerDefiner::bind` expose the dynamic `Schema`/`serde_json::Value` API. Runtime schemas support stripping and strict objects, arrays, unions, literals, enums, defaults, catches, nullability, optionality, records, heterogeneous tuples, open objects, recursive generated references, and Rust refinements/transforms/preprocessors. `Schema::from_type` returns an error for unsupported JSON-schema keywords, patterns or formats. An explicit Rust refinement can express application validators. Unsupported rules are never silently dropped.

## Hosting and ownership

```rust
let mut server = switchboard::server::serve(
    switchboard::server::ServerConfig {
        hostname: "127.0.0.1".into(), port: Some(8080),
        ..Default::default()
    },
    std::sync::Arc::new(router()?),
    Default::default(),
).await?;
// Await application shutdown here.
server.stop().await?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

`serve` supports HTTP, PEM TLS or cached self-signed development certificates, optional HTTP-to-HTTPS redirects, access-log callbacks, log watching and WebSocket upgrades at arbitrary paths. For unprivileged development TLS, set an explicit HTTPS port and `redirect: None`, or choose an unprivileged redirect port. `Https::Generate` defaults to localhost and `.genCert`; generated certificates are development credentials. Supply trusted PEM material for production.

Use `server::serve_http_only(config, router, options).await?` with the same arguments and `ServerHandle` lifecycle when an application does not need hosting-level WebSockets. Valid upgrade requests then dispatch through the router and access-log callback, including normal 404 responses for missing paths. `serve` retains its automatic upgrades and existing defaults. Neither public configuration struct requires a new field.

`ServerHandle::reload()` broadcasts exact `RELOAD` text. Any client text message requests logs when log delivery is configured. Native code changes require rebuilding/restarting the executable; Bun's in-process module HMR is runtime-specific. A file watcher or development process supervisor can trigger the exposed reload operation. The browser helper does not impose a reconnection policy. Hosting uses a separate 120-second WebSocket idle deadline and 60-second pings; the HTTP timeout does not terminate quiet responsive sockets.

Dropping a server initiates shutdown. Await `stop()` to join connections and guarantee listener reuse. The server cancels open/pending requests on stop. Formatter validation is owned by the definer: stop accepting requests, then `drain_background().await` to complete it, or `cleanup_background().await` to abort and join pending work. Dropping the last definer owner aborts pending validation; dropping one clone does not abort another clone's work. Late streaming failures are logged and returned by draining, after already-sent headers remain fixed.

`RequestContext.uploads` retains uploaded field names, filenames, content types and bytes. Use `bind_with_context` when a handler needs files or request metadata. Filenames and MIME declarations remain untrusted application input. Multipart bodies are collected in memory. `ServerConfig.max_request_body_size` defaults to 128 MiB; zero permits only empty bodies. The cap counts bytes lazily as handlers consume them, without pre-auth buffering, and parser limit errors return 413. Applications should choose suitable total-body, file and concurrency policies.

## Rendering and streaming

With `features = ["maud"]`:

```rust
use switchboard::render::{html, html_response};
let response = html_response(html! { main { h1 { ("<untrusted title>") } } });
```

Ordinary interpolations are escaped. `PreEscaped` deliberately crosses the HTML trust boundary. Maud has no hydration payload or mandatory client runtime. Other renderers can return the common HTTP response/body types. `response` retains text/binary identity without changing in-memory headers. HTTP hosting infers `text/plain;charset=utf-8` for nonempty text and `application/octet-stream` for binary data when no Content-Type was supplied. Explicit headers win; empty bodies receive no inferred type. `Formatted::with_body` retains the source identity before body erasure; an explicitly constructed `Formatted` defaults to `BodyKind::Text`, with `Binary` available for byte streams.

`bind_formatted` passes `DeferredValue` to the formatter before nested futures resolve. Construct deferred objects/arrays with `DeferredValue::{object,array,future,ready}`. A formatter can resolve individual fields and return `Formatted.data = axum::body::Body::from_stream(...)`; the body stays a real stream. Calling `resolve()` on the whole value deliberately waits for all fields. Output warnings preserve Zod-style issues, messages, URLs and unresolved Promise identities. Ordinary JSON responses await and validate the full result; valid schemas strip unknown fields, while invalid output warns and retains the original data.

## Native and browser clients

`ApiClient::call(&TypedRoute, &P, Option<&B>, CallSettings)` returns `Result<Option<O>, ClientError>`. It uses canonical URL generation and validates output before deserializing `O`. `call_unchecked` returns JSON `Value`, never a falsely typed `O`. Headers, auth overrides, base URL/method overrides and per-client credential state follow the reference's precedence. Native clients require an absolute base URL; browser CORS/SameSite rules have no equivalent native context.

HTTP failures retain their upstream status. Network failures use status 0 and malformed response data uses 502, with distinct error kinds so a genuine upstream 502 stays distinguishable. Serialization, parameter, schema-validation, deserialization and callback failures remain separate typed errors. Successful HEAD/DELETE and 204/205 return `None`. See the client tests for exact empty/text/JSON handling and credentials.

[The browser crate](browser/README.md) owns Rust-authored fetch, DOM listeners and WebSocket reload/log behavior. Event listeners and sockets are disposed through owned handles. Generated wasm-bindgen bindings and a minimal module bootstrap load the application; browser logic stays in Rust. The application artifact, including used helpers and contracts, is the meaningful byte measurement.

`logger::init_file_logger` returns a cloneable owned append-only `FileLogger`. `LogLevel` and JSON argument encoding match the file format; `read_logfile` and `log_file_change_watcher` provide tailing and subscriptions. Rust code calls this logger explicitly. The library does not monkey-patch global `println!` or `eprintln!` as the Bun reference monkey-patches `console`. Access logging uses configurable callbacks and `tracing`.

## Compatibility evidence and limits

Run the strict shared HTTP/definition/URL differential gate:

```sh
cargo build --manifest-path rust/Cargo.toml --release --all-features --bin compat-runner
bun rust/compat/check.ts ./rust/target/release/compat-runner
```

The current 103-case corpus calls the actual routing/handler APIs in memory and compares response statuses, application headers, bodies, registration errors, URLs, complete validation messages/issues, output warnings and logged errors. Live HTTP/TLS/WebSocket tests and the benchmark exercise transport separately. Four integration cases cover multipart File identity and a nested unresolved-Promise warning. Only documented transport-generated headers are excluded. The checker leaves mismatches visible and exits nonzero for any difference.

The integrated result matches 101 of 103 cases. `method-unsupported` exposes Bun 1.4's Request-constructor quirk: custom `BREW` becomes GET before reaching the reference router. Rust preserves the method and returns 405. `request-leading-bom` exposes another constructor difference: Bun rejects BOM-prefixed JSON from a string-backed Request but accepts the same byte-backed and real HTTP request; Rust accepts its byte body. TRACE agrees at 405. Both compatibility policies remain unresolved. Default strict checking exits nonzero; `--portable-only` excludes only BREW and still fails on the retained BOM case:

```sh
bun rust/compat/check.ts --portable-only ./rust/target/release/compat-runner
# {"cases":102,"mismatches":1,"excludedRuntimeCases":["method-unsupported"]}
```

The separate schema/parameter gate preserves the original 3,103 cases and adds 638 portable cases. All **3,741** match with complete issue arrays/messages and missing-value distinctions. A separate **12-case** audit exercises the explicit safety bounds and reports those intentional boundaries independently:

```sh
cargo build --manifest-path rust/Cargo.toml --bin schema-runner
bun rust/compat/schema/check.ts .tmp/schema-gate ./rust/target/debug/schema-runner
bun rust/compat/schema/bounds.ts .tmp/schema-gate ./rust/target/debug/schema-runner
```

The [schema protocol](compat/schema/README.md) documents the finite model and stored artifacts. Numeric JSON decoding at HTTP/client boundaries and serialization follow finite IEEE-754/ECMAScript semantics before application decisions; use strings for opaque or full-width integer identifiers. A separate unfiltered large-radix audit reports 124 mismatches in 768 cases for the integrated parser. The isolated exact-rounding checkpoint reports 75 and remains unintegrated pending policy approval. These findings are outside the stored schema corpus and block acceptance; they are not safety-bound exceptions.

Safety and representation boundaries are explicit:

| Boundary | Rust behavior |
| --- | --- |
| Query indices | Maximum 4095 |
| Query expansion | Maximum nesting 64 and 65,536 expanded nodes |
| Validation work | Default nesting 128 and 65,536 schema visits, including wrappers and failed union attempts |
| Recursive coercion | Checked finite work/depth bounds; exhaustion returns a validation error |
| Deliberately larger finite validation | `ValidationLimits`, `parse_with_limits`, `parse_value_with_limits` |
| Non-finite JSON numbers, including `1e400` | Rejected; JSON transport uses finite numbers |
| Unpaired UTF-16 surrogates | Rejected; Rust strings contain valid Unicode scalar values |
| Prototype-like keys/query collisions | Safe data structures; JavaScript prototype side effects are not reproduced |
| Unsupported generated schema constraints | Definition-time error; explicit Rust validators remain available |

`params::coerce_form` returns `Result<Value, ValidationError>`. HTTP parsers use checked input-preserving paths so overflow and File/Promise diagnostics survive coercion. A validation-work error remains an error through catch/optional/union wrappers. Limits do not turn unfinished validation into success.

The shared corpus and component tests provide bounded evidence. They do not establish universal 100% equivalence for arbitrary JavaScript values, callbacks, runtime behavior or hostile inputs.

## Performance

[bench/README.md](bench/README.md) defines five complete handler workloads and the alternating multi-sample measurement protocol. `bench-server` uses public definitions, authorization, input/output validation, routing, HTTP hosting and Maud. `loadgen` validates every status/content-type/body. Build both in release mode:

```sh
cargo build --manifest-path rust/Cargo.toml --release --features maud --bin bench-server --bin loadgen
bun rust/bench/run.ts --candidate ./rust/target/release/bench-server \
  --loadgen ./rust/target/release/loadgen --samples 3 --duration-ms 1000 \
  --warmup-ms 500 --concurrency 16 --timeout-ms 5000 \
  --output .tmp/rust-bench.ndjson
```

Raw records include throughput, latency percentiles, process CPU, sampled/peak RSS, startup, environment, parameters and executable hashes. Shared-host loopback measurements cannot establish production WAN/TLS behavior or universal speedups. [VERIFICATION.md](VERIFICATION.md) records feature evidence, actual measurements and limitations.
