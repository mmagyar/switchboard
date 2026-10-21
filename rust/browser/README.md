# switchboard-browser

Optional browser code authored in Rust and compiled to WASM. Server-rendered Maud HTML works without this crate. There is no hydration runtime or virtual DOM. Browser APIs require `wasm32-unknown-unknown` and a browser `Window`; native tests exercise the shared pure request/response and typed-route boundaries.

This independent `cdylib`/`rlib` crate forbids unsafe authored Rust. It depends on the shared `switchboard` core with `default-features = false`, serde, serde_json, wasm-bindgen, wasm-bindgen-futures, js-sys and web-sys. Native HTTP client and server hosting features are disabled in browser builds. Cargo.lock records the resolved versions.

## Application integration

Keep request/response structs in a shared application crate with serde derives. Add this library to the application's browser crate:

```toml
[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
switchboard-browser = { path = "../switchboard/rust/browser" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
wasm-bindgen = "0.2"
wasm-bindgen-futures = "0.4"
web-sys = { version = "0.3", features = ["Window", "Document", "Element"] }
```

A minimal application entry point can fetch typed data and update server-rendered HTML. Serve an element with `id="title"` and a JSON endpoint at `/api/items/42`:

```rust
#![forbid(unsafe_code)]

use serde::Deserialize;
use switchboard_browser::{BrowserClient, CallSettings, ClientOptions};
use wasm_bindgen::prelude::*;

// Import this type from the application's shared contracts crate in production.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Item {
    title: String,
}

#[wasm_bindgen(start)]
pub async fn start() -> Result<(), JsValue> {
    let client = BrowserClient::new("/api", ClientOptions::default());
    let item: Option<Item> = client
        .call::<(), Item>("get", "/items/42", None, &CallSettings::default())
        .await
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
    if let Some(item) = item {
        let title = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id("title"))
            .ok_or_else(|| JsValue::from_str("Missing #title"))?;
        title.set_text_content(Some(&item.title));
    }
    Ok(())
}
```

`BrowserClient::call<B, O>` accepts an already encoded path, a method string, an optional `&B: Serialize`, and `CallSettings`. It returns `Result<Option<O>, ClientError>` where `O: DeserializeOwned`. Bodies such as `false`, `0`, `""` and JSON null are sent when present. Use `None::<&()>` for no body. The base URL and path are concatenated literally, matching the Bun client. They must contain the intended slash boundary. Browser fetch resolves relative URLs against the document.

### Shared typed routes

`BrowserClient::call_route(&route, &params, body, &settings)` accepts the core `switchboard::TypedRoute<P, B, O, Permission>` directly and returns `Result<Option<O>, RouteCallError>`. Parameters are serialized and validated/encoded by core `params::def_to_url`. Successful values pass through the route's output `Schema::parse_value` before serde decodes `O`. Schema transforms and object stripping therefore precede Rust deserialization. The adapter defines no duplicate route or schema model.

`call_route_unchecked` has the same arguments but returns `Result<Option<serde_json::Value>, RouteCallError>`. It still validates parameters and parses JSON, but skips the output schema and has no `O: DeserializeOwned` bound. It never claims the route's output type. Empty-response semantics and per-call overrides are shared with raw `call`.

```rust,ignore
let route = shared_contracts::item_route()?;
let client = BrowserClient::new("", ClientOptions::default());
let item = client.call_route(&route, &params, None, &CallSettings::default()).await?;
let untrusted = client.call_route_unchecked(&route, &params, None, &CallSettings::default()).await?;
```

`RouteCallError` preserves failures as `Transport(ClientError)`, `Serialization(serde_json::Error)`, `Parameters(HttpError)`, `Validation(ValidationError)` and `Deserialization(serde_json::Error)`, each with an error source. A serde-compatible string can fail an output refinement and remains a `Validation` error rather than an HTTP/502 failure.

Raw `call` remains available with its original signature. It validates only the serde output contract; use the typed-route adapter for schema refinements. `serde_json::Value` explicitly requests untyped JSON. Text responses deserialize from a Rust string value, so use `String` or `Value` for text, not a numeric type. Requests and compacted JSON HTTP errors reuse core ECMAScript JSON stringification (numeric spelling and index-key order).

## Settings and failures

- `ClientOptions` supplies default headers, credentials and optional `Rc<dyn Fn()>` unauthorized/forbidden callbacks. Callbacks run only for 401/403 respectively, outside the transport error mapping.
- `CallSettings` overrides method, base URL and credentials. Headers are applied in order: Accept/Content-Type defaults, client headers, call headers, then `auth_token_override` as the complete Authorization value (for example `Bearer ...`). Fetch normalizes header names case-insensitively. Browser forbidden-header and CORS restrictions still apply.
- Credentials default to `SameOrigin`, matching default fetch. `Include` also sends cross-origin credentials when the server's CORS policy permits it. `Omit` sends none.
- `ClientErrorKind::Http` preserves the actual HTTP status for statuses >= 400. Error bodies are read once; valid JSON is compacted, otherwise text is retained. An unreadable/empty error body keeps a status-only message.
- `Network` uses status 0 for failed fetch or unreadable non-JSON success bodies. `InvalidResponse` uses status 502 for malformed/unreadable JSON or a serde output mismatch. A genuine upstream 502 remains `Http`. `Request` uses status 0 for caller serialization or a missing browser Window. Fetch initialization failures, including GET/HEAD bodies, forbidden methods, invalid URLs and header construction, are `Network`/0. JavaScript errors use their `message` rather than a JsValue debug wrapper. Branch on `kind` as well as `status`.
- A successful effective HEAD or DELETE, or status 204/205, returns `None` without consuming or validating a body. An empty non-JSON body also returns `None`. An empty body advertised as JSON is invalid. JSON null deserialized as `Value` returns `Some(Value::Null)`.
- There is no validation opt-out that claims a typed result. Use `call_route_unchecked` or request `Value` from raw `call`. GET/HEAD bodies and forbidden methods are rejected by browser fetch and classified as `Network`/0, matching the original client. Auth callbacks remain outside transport error mapping.

Successful JSON uses the original native `Response.json()` call before entering the shared finite Rust JSON boundary. Native malformed/empty JSON diagnostics remain intact; no response clone or diagnostic-prefix normalization is used. Parsed numbers reach schema callbacks and serde with ECMAScript precision, and negative-zero leaves remain negative zero. Nonfinite leaves, lone UTF-16 surrogates and excessive JSON nesting retain the documented Rust boundaries.

## Event and WebSocket ownership

`EventListener::new(&EventTarget, event, callback)` registers an ordinary non-capturing DOM listener. It owns the closure and removes the listener on `close()` or `Drop`. Keep the handle in your application's Rust-owned page/component state. Dropping a local handle immediately removes its listener. `close()` is idempotent and reports browser removal errors; on removal failure it retains the callback for a retry. Standard DOM removal does not throw; Drop logs unexpected cleanup errors to the browser console.

`HotReloadConnection::connect(url, on_text)` opens the server's WS endpoint. Exact text `RELOAD` calls `window.location().reload()`. Every other text frame, including empty text, reaches `on_text(String)` unchanged. Binary frames are ignored. Reload errors are logged to the console. Choose `ws://` or `wss://` to match the page's transport and CSP. The reference server accepts upgrades at any path.

`request_logs()` sends an empty text message after the socket opens. Any outbound payload requests logs under the server protocol. Calling it while not OPEN returns an error. `socket()` exposes the underlying WebSocket for `ready_state()` and open/error/close event registration with `EventListener`. Initial logs and reconnection are explicit application policy.

Both handles clean up through `Drop`; the library never uses `Closure::forget`. Connection `close()` detaches its callback and closes its socket. Handle callback capture cycles remain the application's responsibility. For example, a Rust page owner can store the following fields, and construct an open listener that requests initial logs:

```rust
use switchboard_browser::{EventListener, HotReloadConnection};
use wasm_bindgen::JsValue;

pub struct PageConnections {
    _open: EventListener,
    _hot: HotReloadConnection,
}

impl PageConnections {
    pub fn connect(url: &str) -> Result<Self, JsValue> {
        let hot = HotReloadConnection::connect(url, |logs| {
            // Use text_content for display. Log text is untrusted input.
            web_sys::console::log_1(&JsValue::from_str(&logs));
        })?;
        let socket = hot.socket().clone();
        let open = EventListener::new(hot.socket().as_ref(), "open", move |_| {
            if let Err(error) = socket.send_with_str("") {
                web_sys::console::error_1(&error);
            }
        })?;
        Ok(Self { _open: open, _hot: hot })
    }
}
```

The example's console calls require web-sys's `console` feature, enabled by this library. Drop `PageConnections` at page disposal. Rust applications that export an owning handle through wasm-bindgen should call its generated `free()` method at disposal, including SPA page replacement. No event closure is intentionally leaked for process lifetime.

## Build and generated loader

From the repository root, with Rust and the wasm target installed:

```sh
rustup target add wasm32-unknown-unknown
cargo test --manifest-path rust/browser/Cargo.toml --locked
cargo check --manifest-path rust/browser/Cargo.toml --locked --target wasm32-unknown-unknown
cargo build --manifest-path rust/browser/Cargo.toml --locked --target wasm32-unknown-unknown --release
```

For the application crate above, build its release WASM and run a wasm-bindgen CLI version **exactly matching** the wasm-bindgen version in its Cargo.lock. This crate's Cargo.lock currently pins 0.2.129:

```sh
cargo install wasm-bindgen-cli --version 0.2.129 --locked
cargo build --manifest-path path/to/page/Cargo.toml --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir path/to/static/generated \
  path/to/page/target/wasm32-unknown-unknown/release/page.wasm
```

Serve the generated files with `application/wasm` for `.wasm`. The generated loader is initialized from the server template with this minimal module bootstrap:

```html
<script type="module">
  import init from "/generated/page.js";
  await init();
</script>
```

`page.js`, `page_bg.wasm` and TypeScript declarations are generated bindings, not authored application logic. The bootstrap only initializes those bindings; the Rust `#[wasm_bindgen(start)]` function contains the browser behavior. Serve this bootstrap as a CSP-approved module or nonce it as required by the application. No hydration payload is required. Building switchboard-browser alone exports no application entry point; link its `rlib` into the application for useful behavior.

Measure the **application's generated release artifact**, since linking removes unused helpers:

```sh
wc -c path/to/static/generated/page_bg.wasm
gzip -c -9 path/to/static/generated/page_bg.wasm | wc -c
wc -c path/to/static/generated/page.js
```

Record the exact Rust/wasm-bindgen versions, build profile, enabled features, application behavior, raw bytes and compressed bytes. The standalone crate's unbound cdylib size is not a representative page-size benchmark.

## Linkable demo and real-browser proof

[`examples/demo`](examples/demo) contains a linkable Rust WASM application, shared typed contracts and a live Rust/Maud HTTP server. The same contract function is used by `bind_typed` on the server and `call_route` in the browser. The deliberate invalid-output endpoint returns serde-compatible JSON that violates the shared title literal. The page displays its schema rejection and the unchecked JSON, a counter, resource disposal and WebSocket logs. The minimal JavaScript bootstrap initializes WASM, frees the Rust page owner on `pagehide`, and reloads on persisted `pageshow`. Rust renders API-startup failures with reload instructions and leaves controls disabled until listeners are installed. All fetch work is awaited. Rust listeners/connections belong to the exported `Page`; closing resources removes the counter listener and closes the socket. No `Closure::forget` is used.

Exact commands from the repository root (wasm-bindgen CLI must match lockfile 0.2.129):

```sh
export RUSTUP_HOME=/home/dev/switchboard-worktrees/rust-implementation/.tmp/rustup
export CARGO_HOME=/home/dev/switchboard-worktrees/rust-implementation/.tmp/cargo
export PATH="$CARGO_HOME/bin:$PATH"
export CARGO_PROFILE_DEV_DEBUG=0

cargo test --manifest-path rust/browser/Cargo.toml --locked
cargo check --manifest-path rust/browser/Cargo.toml --locked --target wasm32-unknown-unknown
cargo build --manifest-path rust/browser/examples/demo/Cargo.toml --locked --target wasm32-unknown-unknown --release
/home/dev/switchboard-worktrees/rust-implementation/.tmp/wasm-bindgen-0.2.129-x86_64-unknown-linux-musl/wasm-bindgen \
  --target web --out-name demo --out-dir rust/browser/examples/demo/static/generated \
  rust/browser/examples/demo/target/wasm32-unknown-unknown/release/switchboard_browser_demo.wasm
cargo build --manifest-path rust/browser/examples/demo/Cargo.toml --locked --features server --bin browser-demo-server
(cd rust/browser && bun install --frozen-lockfile)
bun rust/browser/measure.ts
bun rust/browser/integration.ts
```

Playwright-core 1.60.0 requires a local matching Chromium installation, for example `bun x --package playwright-core playwright-core install chromium` from `rust/browser`. Integration boots its own Rust server on `127.0.0.1:3217`, runs both Rust and reference pages, and tears down Chromium and the server. A port collision is an error. The demo can also be run directly with `rust/browser/examples/demo/target/debug/browser-demo-server`; open `http://127.0.0.1:3217/`. It is a localhost functional demo, not a production server configuration.

`reference.ts` and `measure.ts` are test/reference tooling only. Bun.build minifies a browser bundle importing the unchanged original `src/clientExport.ts` and Zod. Its route/schema, typed fetch, invalid-output rejection, unchecked fetch, counter/disposal, WebSocket logs/close, reload and negative fetch initialization behavior match the Rust demo. The same browser assertions run against `/reference`. No root package or original source is modified.

Evidence is emitted under repository `.tmp/browser/`: `browser-evidence.json`, `reference-evidence.json`, `browser-desktop.png`, `browser-mobile.png`, `reference-desktop.png`, `reference-mobile.png`, `live-server.log` and `artifact-sizes.json`. Generated application bindings and WASM stay under ignored `examples/demo/static/generated/`; build them with the commands above.

The refreshed linked application (release `opt-level=s`, LTO, one codegen unit, debug stripping; wasm-bindgen 0.2.129; Bun 1.4.0) is **598,367 bytes raw / 172,875 gzip** WASM and **26,440 / 5,798** generated JS. Their total is **624,807 / 178,673**, or **625,075 / 178,861** including the bootstrap. The comparable minified original-client reference JS is **291,595 / 66,684**. Gzip is level 9, applied independently per HTTP resource with no filename/timestamp metadata. Shared HTML/CSS, declaration files and source maps are excluded from both totals. These numbers establish no Rust size advantage. Standalone unbound crate size is not representative.

The public [`fixtures/decode-probe`](fixtures/decode-probe) consumer and `decoder-probe.ts` exercise the actual browser client and typed-route APIs without production test hooks:

```sh
cargo build --manifest-path rust/browser/fixtures/decode-probe/Cargo.toml --release --target wasm32-unknown-unknown --locked
wasm-bindgen --target web --out-dir .tmp/browser-decode/generated --out-name probe \
  rust/browser/fixtures/decode-probe/target/wasm32-unknown-unknown/release/switchboard_browser_decode_probe.wasm
bun rust/browser/decoder-probe.ts
```

All 19 probes passed in Chromium. They cover numeric precision before callbacks/serde, nested negative zero, exact malformed/empty JSON diagnostics, auth statuses, fetch initialization failures, Rust type errors and the explicit finite/Unicode/nesting limits. Empty network streams can produce different native diagnostic prefixes across independent calls. Deterministic fixtures compare exact messages, and the real-network empty fixture compares each call against its own captured native rejection. No prefix rewriting or retries are used. Evidence is under `.tmp/browser-decode/`.

The updated application journeys also passed startup API failure/recovery and history navigation. Real back navigation loaded a fresh document. Chromium's WebSocket/delegate policy prevented actual BFcache restoration; explicit persisted events verified only the reload branch. Desktop/mobile success and startup-failure screenshots were inspected. WASM download/initialization failure remains outside this API-startup journey.

Validation commands:

```sh
cargo fmt --manifest-path rust/browser/Cargo.toml -- --check
cargo fmt --manifest-path rust/browser/examples/demo/Cargo.toml -- --check
cargo clippy --manifest-path rust/browser/Cargo.toml --all-targets --locked -- -D warnings
cargo clippy --manifest-path rust/browser/Cargo.toml --lib --locked --target wasm32-unknown-unknown -- -D warnings
cargo clippy --manifest-path rust/browser/examples/demo/Cargo.toml --all-targets --features server --locked -- -D warnings
cargo clippy --manifest-path rust/browser/examples/demo/Cargo.toml --lib --target wasm32-unknown-unknown --locked -- -D warnings
bun x tsc --ignoreConfig --noEmit --strict --skipLibCheck --allowImportingTsExtensions --module esnext --moduleResolution bundler --target esnext --types bun rust/browser/integration.ts rust/browser/reference.ts rust/browser/measure.ts
bun run check
bun test
```

Native tests cover parameter encoding/failures, structured schema validation versus serde failure, schema stripping, absent bodies, ECMAScript stringification, raw error classification, auth callbacks and reload-message dispatch. Mutation proof bypassed output `Schema::parse_value`; the refinement regression failed with an incorrectly accepted `title: "rejected"`, then passed after restoration. Chromium 148.0.7778.96 exercised real fetch and literal validation, GET/HEAD-body and TRACE rejection as `Network`/0 with original fetch messages, invalid Authorization header rejection as `Network`/0, exact transport and schema-validation text matching the original client, unchanged log text, listener removal after disposal, WebSocket close, exact `RELOAD` receipt and a navigation entry of type `reload`. Desktop (1100px) and mobile (360px) screenshots were inspected: legible labels/buttons and wrapped JSON, no horizontal overflow. Cross-origin CORS, credential/cookie policy, HTTPS/WSS, offline transport and browsers other than Chromium remain unverified by this demo.
