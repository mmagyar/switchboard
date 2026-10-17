#![forbid(unsafe_code)]

use std::{
    io::{self, Write},
    path::PathBuf,
    sync::Arc,
};
use switchboard::render::{PreEscaped, html, html_response};
use switchboard::server::{ServerConfig, ServerOptions, serve};
use switchboard::{Authorization, HandlerDefiner, Method, RegisteredRoute, Router, response};
use switchboard_browser_demo::{Item, item_route};

fn page(bootstrap: &str) -> switchboard::Response {
    html_response(html! {
        (PreEscaped("<!DOCTYPE html>"))
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Switchboard Rust browser demo" }
                link rel="stylesheet" href="/demo.css";
            }
            body { main {
                h1 { "Switchboard Rust browser demo" }
                p { "Server-rendered Maud HTML. Rust/WASM adds fetch and owned browser connections." }
                h2 { "Shared typed route" }
                dl {
                    dt { "Fetch" } dd id="fetch" { "Loading item..." }
                    dt { "Validation" } dd id="validation" { "Checking output schema..." }
                    dt { "Unchecked" } dd id="unchecked" { "Loading untrusted JSON..." }
                    dt { "Fetch initialization" } dd id="transport" { "Checking fetch failures..." }
                }
                h2 { "Owned browser resources" }
                p { "Count: " output id="count" { "0" } }
                button id="increment" type="button" disabled { "Increment" }
                button id="dispose" type="button" disabled { "Dispose resources" }
                p { "Resources: " span id="disposal" { "Active" } }
                p { "WebSocket: " span id="socket" { "Connecting..." } }
                pre id="logs" { "Waiting for logs..." }
                p role="status" id="status" { "Starting Rust application..." }
                script type="module" src=(bootstrap) {}
            } }
        }
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (reload_tx, mut reload_rx) = tokio::sync::mpsc::channel(1);
    let static_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static");
    let mut router = Router::with_default(Arc::new(move |request, _| {
        let reload_tx = reload_tx.clone();
        let static_dir = static_dir.clone();
        Box::pin(async move {
            match (request.method().as_str(), request.uri().path()) {
                ("GET", "/") => page("/bootstrap.js"),
                ("GET", "/reference") => page("/generated/reference.js"),
                (
                    "GET",
                    path @ ("/demo.css"
                    | "/bootstrap.js"
                    | "/generated/demo.js"
                    | "/generated/reference.js"
                    | "/generated/demo_bg.wasm"),
                ) => {
                    let mime = if path.ends_with(".wasm") {
                        "application/wasm"
                    } else if path.ends_with(".css") {
                        "text/css"
                    } else {
                        "text/javascript"
                    };
                    match std::fs::read(static_dir.join(&path[1..])) {
                        Ok(bytes) => response(200, mime, bytes),
                        Err(error) => response(404, "text/plain", error.to_string()),
                    }
                }
                ("POST", "/reload") => match reload_tx.send(()).await {
                    Ok(()) => response(204, "", ""),
                    Err(error) => response(503, "text/plain", error.to_string()),
                },
                ("GET", "/api/refinement/42") => response(
                    200,
                    "application/json",
                    r#"{"title":"wrong title","query":"a b&c"}"#,
                ),
                _ => response(404, "text/plain", "Not found"),
            }
        })
    }));
    let definer = HandlerDefiner::new(
        |_| async { Ok(()) },
        |(), (), _| async { Ok(Authorization::Ok) },
    );
    router.add_route(
        definer.bind_typed(item_route()?, |params, _, _, _| async move {
            Ok(Item {
                title: format!("Item {}", params.item_id),
                query: params.query,
            })
        }),
    )?;
    // Raw transport remains usable without a TypedRoute.
    router.add_route(RegisteredRoute {
        method: Method::Get,
        path: "/health".into(),
        prefixes: vec![],
        handler: Arc::new(|_, _| Box::pin(async { response(200, "text/plain", "ready") })),
    })?;
    let mut server = serve(
        ServerConfig {
            hostname: "127.0.0.1".into(),
            port: Some(3217),
            redirect: None,
            ..ServerConfig::default()
        },
        Arc::new(router),
        ServerOptions {
            access_log: None,
            read_logs: Some(Arc::new(|| {
                Box::pin(async { Ok("Rust server log\nsecond line".into()) })
            })),
            ..ServerOptions::default()
        },
    )
    .await?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    println!(
        "{}",
        serde_json::json!({"ready":true,"port":server.local_addr().port()})
    );
    io::stdout().flush()?;
    loop {
        tokio::select! {
            result = tokio::signal::ctrl_c() => { result?; break; }
            _ = terminate.recv() => break,
            Some(()) = reload_rx.recv() => server.reload(),
        }
    }
    server.stop().await?;
    definer.cleanup_background().await?;
    Ok(())
}
