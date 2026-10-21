# Rust implementation contract

The Bun library remains the reference. The Rust product has no Bun, Node, embedded JavaScript, or hydration dependency. Optional browser logic is Rust compiled to WASM with generated JavaScript bindings. Established crates and native cryptography are authorized. Maud is optional. Behavioral compatibility is verified with shared cases, not assumed from source similarity.

## Delivery and ownership

This is multi-seam work. Each isolated writer owns its listed files and hands off a commit or patch. The parent owns integration, final acceptance, and publication. No push or merge is authorized.

| Lane | Files | Decision | Gate |
| --- | --- | --- | --- |
| Schema/parser | rust/src/schema.rs, rust/src/params.rs | Schema-directed parsing, validation and URL generation | Unit tests and reference cases |
| Routing foundation (parent) | rust/Cargo.toml, rust/src/lib.rs, rust/src/router.rs | Standalone trie and common HTTP contracts | Unit tests |
| Handler | rust/src/handler.rs | Auth, typed handlers, deferred results, formatting, errors | Handler unit tests |
| Hosting | rust/src/server.rs, rust/src/logger.rs | HTTP/TLS, WebSockets, lifecycle and file logging | Live HTTP/TLS/WS and shutdown tests |
| Clients/browser | rust/src/client.rs, rust/browser/** | Native typed client and WASM browser helpers | Client tests and wasm target check |
| A/B and performance | rust/compat/**, rust/src/bin/**, rust/README.md | Shared fixtures, differential and resource measurements | Both processes return identical results; bounded benchmark |

The schema/parser and routing foundation establish the shared API before the dependent handler, hosting and client lanes begin. The A/B fixture corpus can be prepared independently. Each writer uses a separate worktree. The parent integrates handoffs and runs all checks before committing each working increment.

## Shared Rust API

The package is `switchboard` in `rust/`, edition 2024. Use `#![forbid(unsafe_code)]` in authored Rust crates. Common dependencies are axum 0.8, bytes 1, http 1, http-body-util 0.1, futures-util 0.3, serde 1 (derive), serde_json 1 (preserve_order), indexmap 2 (serde), url 2, tokio 1 (full), schemars 1. Hosting adds tokio-rustls, rustls-pemfile, rcgen, tokio-tungstenite, notify and tracing. Client uses reqwest with rustls. Optional Maud integration uses maud. Versions are resolved and locked by Cargo.

Shared definitions live in `lib.rs`:

- `pub type Request = http::Request<axum::body::Body>`.
- `pub type Response = http::Response<axum::body::Body>`.
- `pub type Handler = Arc<dyn Fn(Request, Option<String>) -> BoxFuture<'static, Response> + Send + Sync>`.
- `Method` has Get, Post, Put, Patch, Delete, Head, Options; `as_str()`, `parse(&str) -> Result<Self, HttpError>`, `success_status() -> u16`, `has_body() -> bool`.
- `HttpError { pub status: u16, pub message: String, pub kind: ErrorKind }`, constructors `new`, `not_found`, `unauthorized`, `forbidden`, `internal`; implements Display and Error.
- `DefinitionError(pub String)` implements Display and Error.
- `RequestContext { pub method: Method, pub url: url::Url, pub headers: http::HeaderMap, pub uploads: Vec<UploadedFile> }`, cloneable; `from_request(&Request) -> Result<Self, HttpError>`. Missing absolute URI uses Host, default `localhost`, and http.
- `RegisteredRoute { pub method: Method, pub path: String, pub prefixes: Vec<String>, pub handler: Handler }`.
- `response<T: Into<Body> + 'static>(status: u16, content_type: &str, body: T) -> Response`. Body identity is retained in response extensions for HTTP-only MIME inference; in-memory headers stay unchanged. `BodyKind::{Text,Binary}` and `Formatted::with_body` preserve identity through formatter body erasure.

`Router::new()`, `Router::with_default(Handler)`, `add_route(&mut self, RegisteredRoute) -> Result<(), DefinitionError>`, `async handle_request(&self, Request) -> Response`. Router construction is mutable; dispatch is immutable and supports Arc sharing. Expose a route lookup method for router-only benchmarking. Preserve static-first fallback, optional trailing segments, duplicate/conflicting edge detection across methods, prefix rewriting, custom defaults, and HEAD behavior.

## Schema and parameter API

`Schema` is cloneable and implements serde serialization/deserialization as an adjacently tagged enum (`type` and `value`, camelCase variants). Required variants: Unknown, String, Number, Boolean, Literal(Value), Enum(Vec<String>), Object(IndexMap<String, Schema>), Array(Box<Schema>), Union(Vec<Schema>), Optional(Box<Schema>), Nullable(Box<Schema>), Default(Box<Schema>, Value), Catch(Box<Schema>, Value), Readonly(Box<Schema>). Additional custom refinements/transforms and typed schema derivation may use non-serializable variants or separate methods.

- `Schema::parse(&self, Option<Value>) -> Result<Option<Value>, ValidationError>` distinguishes absent input from null.
- `Schema::parse_value(&self, Value) -> Result<Value, ValidationError>`.
- `Schema::validate_params(&self) -> Result<(), DefinitionError>` rejects ambiguous string-origin unions recursively through wrappers.
- `Schema::from_type<T: schemars::JsonSchema>() -> Result<Self, DefinitionError>` derives runtime schemas from Rust types, including structs, options, arrays and enums. Unsupported generated schema constructs return explicit errors.
- `ValidationError { pub issues: Vec<Value> }` implements Display as Zod-compatible pretty JSON where possible. Never conceal mismatches by normalizing errors in the A/B runner.
- `params::auto_schema(path: &str) -> Result<Schema, DefinitionError>`.
- `params::validate_path(path: &str, schema: &Schema, prefixes: &[String]) -> Result<(), DefinitionError>`.
- `params::parse_url(url: &Url, path: &str, schema: &Schema) -> Result<Value, ValidationError>`.
- `params::coerce_form(schema: &Schema, value: Value) -> Result<Value, ValidationError>` (flat form bodies, nested query keys are handled separately).
- `params::def_to_url(path: &str, schema: &Schema, params: Value) -> Result<String, HttpError>`.

Preserve path-over-query precedence, repeated values, dot keys and indexed arrays, both numeric ID conventions, optional-path truncation, URI encoding, empty-array omission and recursive number/boolean conversion. Preserve validation and transformation behavior through Rust equivalents. Sparse input must have a safe bounded representation; report any reference behavior that cannot be reproduced safely.

## Handler API

The handler lane owns the exact ergonomic APIs but must expose these integration anchors:

- `RouteDef<Permission> { pub method: Method, pub path: String, pub permission: Permission, pub params: Schema, pub body: Option<Schema>, pub output: Schema, pub prefixes: Vec<String> }` and validated constructor `new(method, path, permission, params: Option<Schema>, body: Option<Schema>, output, prefixes) -> Result<Self, DefinitionError>`.
- `Authorization::{Ok, Forbidden, Unauthenticated}`.
- `HandlerDefiner<User, Permission>` resolves user then authorizes before parsing.
- `bind(route, handler) -> RegisteredRoute`, where handler accepts `(Value, Option<Value>, User, Option<String>)` and returns a future of `Result<DeferredValue, HttpError>`.
- `DeferredValue` supports ready JSON, future values, objects and arrays, recursively resolved for JSON. Formatting must receive unresolved futures, with per-property warning validation that does not delay streaming.
- Error options support output warning callback, custom error parser, HTML error formatter, and error logger. Default error negotiation follows Accept, including JSON-string error bodies. Generic errors must not expose internals unless explicitly configured.
- Typed route helpers derive schemas from Rust types and deserialize validated input before invoking typed callbacks. Output types are inferred from Rust contracts. Client use must preserve these contracts.

Status defaults match the reference: POST 201, DELETE 204, other supported methods 200. Invalid JSON returns text 400. Unsupported body media type returns text 415. Validated outputs strip unknown fields; invalid outputs warn and retain the original value. Deferred formatting supports custom headers, status and redirects.

## Differential protocol

A shared JSON corpus describes schemas, routes, requests, and expected results. A Bun reference runner imports the original library; a Rust runner uses the new library. Runners consume one JSON object per line and emit one JSON object per line. No embedded runtime or subprocess exists in the Rust product. Test tooling may launch Bun as the reference.

Responses include status, normalized sorted application headers and body (JSON values or text with an explicit discriminator). Ignore only transport-generated Date/connection headers. Never remove validation text or otherwise normalize away a semantic mismatch. Registration failures, URL generation, client errors, streaming, TLS and WebSockets need their own observable comparisons or explicit test evidence.

Benchmarks use release builds, equivalent routes and response bytes, warmup, alternating order and multiple samples. Separate router-only from full HTTP and template rendering. Record requests/sec, latency p50/p95/p99, CPU time, RSS/peak RSS, startup and browser artifact sizes where available. Include concurrency, route count, payload size, toolchain/OS/CPU and exact commands. Report negative results and limitations. Preserve raw measurements under .tmp and publish a bounded reproducible report.
