# switchboard-browser

Optional browser code authored in Rust and compiled to WASM. Server-rendered Maud HTML works without this crate. There is no hydration runtime, virtual DOM, server dependency or handwritten JavaScript product logic. Browser APIs require `wasm32-unknown-unknown` and a browser `Window`; native tests exercise the shared pure request/response logic.

This independent `cdylib`/`rlib` crate forbids unsafe authored Rust. It depends on serde, serde_json, wasm-bindgen, wasm-bindgen-futures and web-sys. Cargo.lock records the resolved versions.

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

This transport deliberately defines no duplicate route or schema model. The root-owned future typed-route adapter will validate/encode parameters through the shared route API and delegate to `call`. Until that adapter exists, callers resolve paths themselves. Serde deserialization validates the requested output type. Refinements require application validation; serde does not reproduce every server schema rule. `serde_json::Value` explicitly requests untyped JSON. Text responses deserialize from a Rust string value, so use `String` or `Value` for text, not a numeric type.

## Settings and failures

- `ClientOptions` supplies default headers, credentials and optional `Rc<dyn Fn()>` unauthorized/forbidden callbacks. Callbacks run only for 401/403 respectively, outside the transport error mapping.
- `CallSettings` overrides method, base URL and credentials. Headers are applied in order: Accept/Content-Type defaults, client headers, call headers, then `auth_token_override` as the complete Authorization value (for example `Bearer ...`). Fetch normalizes header names case-insensitively. Browser forbidden-header and CORS restrictions still apply.
- Credentials default to `SameOrigin`, matching default fetch. `Include` also sends cross-origin credentials when the server's CORS policy permits it. `Omit` sends none.
- `ClientErrorKind::Http` preserves the actual HTTP status for statuses >= 400. Error bodies are read once; valid JSON is compacted, otherwise text is retained. An unreadable/empty error body keeps a status-only message.
- `Network` uses status 0 for failed fetch or unreadable non-JSON success bodies. `InvalidResponse` uses status 502 for malformed/unreadable JSON or a serde output mismatch. A genuine upstream 502 remains `Http`. `Request` uses status 0 for caller serialization, header, URL or request-construction errors. Branch on `kind` as well as `status`.
- A successful effective HEAD or DELETE, or status 204/205, returns `None` without consuming or validating a body. An empty non-JSON body also returns `None`. An empty body advertised as JSON is invalid. JSON null deserialized as `Value` returns `Some(Value::Null)`.
- There is no validation opt-out that claims a typed result. Request `Value` instead. GET/HEAD bodies and forbidden methods are rejected by browser request construction rather than disguised as network errors.

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

Native tests cover request preparation, serde validation, error classification, empty-response semantics, auth callbacks and exact reload-message dispatch. wasm target checking/building verifies web-sys bindings. Real browser fetch/CORS, event cleanup, socket lifecycle and navigation/reload need a separate browser integration run against a live server. Compilation alone does not establish those behaviors.
