#![forbid(unsafe_code)]

use axum::body::Body;
use serde_json::json;
use std::{
    io::{self, Write},
    sync::Arc,
};
use switchboard::render::html;
use switchboard::server::{ServerConfig, ServerOptions, serve};
use switchboard::{
    Authorization, DeferredValue, Formatted, HandlerDefiner, HandlerOptions, HttpError, Method,
    RouteDef, Router, Schema,
};

fn object(fields: impl IntoIterator<Item = (&'static str, Schema)>) -> Schema {
    Schema::Object(
        fields
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    )
}

fn route(
    method: Method,
    path: &str,
    params: Option<Schema>,
    body: Option<Schema>,
    output: Schema,
) -> Result<RouteDef<String>, switchboard::DefinitionError> {
    RouteDef::new(method, path, "bench".into(), params, body, output, vec![])
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let port = match (args.next(), args.next(), args.next()) {
        (Some(flag), Some(port), None) if flag == "--port" => port.parse::<u16>()?,
        (None, None, None) => 0,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Usage: bench-server [--port PORT]",
            )
            .into());
        }
    };
    let definer = HandlerDefiner::new(
        |request: switchboard::RequestContext| async move {
            Ok(request
                .headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned))
        },
        |user: Option<String>, permission: String, _| async move {
            Ok(
                if permission == "bench" && user.as_deref() == Some("Bearer bench-token") {
                    Authorization::Ok
                } else {
                    Authorization::Unauthenticated
                },
            )
        },
    )
    .with_options(HandlerOptions {
        output_warning: Some(Arc::new(|warning| {
            panic!("Benchmark output invalid: {}", warning.error)
        })),
        ..HandlerOptions::default()
    });
    let mut router = Router::new();
    router.add_route(definer.bind(
        route(
            Method::Get,
            "/static",
            None,
            None,
            object([("ok", Schema::Boolean)]),
        )?,
        |_, _, _, _| async { Ok(DeferredValue::ready(json!({"ok":true}))) },
    ))?;
    let id = object([("id", Schema::Number)]);
    router.add_route(definer.bind(
        route(
            Method::Get,
            "/items/:id",
            Some(id.clone()),
            None,
            id.clone(),
        )?,
        |params, _, _, _| async move { Ok(DeferredValue::ready(params)) },
    ))?;
    let search = object([
        ("filter", object([("active", Schema::Boolean)])),
        ("values", Schema::Array(Box::new(Schema::Number))),
    ]);
    router.add_route(definer.bind(
        route(Method::Get, "/search", Some(search.clone()), None, search)?,
        |params, _, _, _| async move { Ok(DeferredValue::ready(params)) },
    ))?;
    router.add_route(definer.bind(
        route(
            Method::Post,
            "/items",
            None,
            Some(object([
                ("name", Schema::String),
                ("tags", Schema::Array(Box::new(Schema::String))),
            ])),
            object([("name", Schema::String), ("count", Schema::Number)]),
        )?,
        |_, body, _, _| async move {
            let body = body.ok_or_else(HttpError::internal)?;
            let count = body
                .get("tags")
                .and_then(|tags| tags.as_array())
                .map(Vec::len)
                .ok_or_else(HttpError::internal)?;
            Ok(DeferredValue::ready(
                json!({"name":body["name"],"count":count}),
            ))
        },
    ))?;
    router.add_route(definer.bind_formatted(
        route(Method::Get, "/html/:id", Some(id.clone()), None, id)?,
        |params, _, _, _| async move { Ok(DeferredValue::ready(params)) },
        |data, _, _, _| async move {
            let DeferredValue::Ready(data) = data else {
                return Err(HttpError::internal());
            };
            let id = data["id"].as_f64().ok_or_else(HttpError::internal)?;
            let markup = html! { main { h1 { "Item " (id) } p { "Switchboard" } } };
            Ok(Formatted {
                data: Body::from(markup.into_string()),
                headers: http::HeaderMap::from_iter([(
                    http::header::CONTENT_TYPE,
                    http::HeaderValue::from_static("text/html; charset=utf-8"),
                )]),
                ..Formatted::default()
            })
        },
    ))?;
    let mut server = serve(
        ServerConfig {
            hostname: "127.0.0.1".into(),
            port: Some(port),
            ..ServerConfig::default()
        },
        Arc::new(router),
        ServerOptions {
            access_log: None,
            ..ServerOptions::default()
        },
    )
    .await?;
    println!(
        "{}",
        json!({"ready":true,"port":server.local_addr().port()})
    );
    io::stdout().flush()?;
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { result = tokio::signal::ctrl_c() => result?, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    server.stop().await?;
    definer.cleanup_background().await?;
    Ok(())
}
