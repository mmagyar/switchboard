#![forbid(unsafe_code)]

#[cfg(target_arch = "wasm32")]
mod browser {
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};
    use std::{cell::Cell, rc::Rc};
    use switchboard::{Method, Schema, TypedRoute};
    use switchboard_browser::{BrowserClient, CallSettings, ClientOptions, RouteCallError};
    use wasm_bindgen::prelude::*;

    #[derive(Serialize, JsonSchema)]
    struct Params {}

    #[derive(Deserialize, JsonSchema)]
    struct Numbers {
        n: u64,
        negative: i64,
        integral: u32,
        nested: Vec<u64>,
    }

    fn observe(value: Value) -> Value {
        json!({
            "n": value["n"].to_string(),
            "negative": value["negative"].to_string(),
            "integral": value["integral"].to_string(),
            "nested": value["nested"][0].to_string(),
            "negativeZero": negative_zero(&value["zero"]),
            "nestedNegativeZero": [negative_zero(&value["zeros"][0]), negative_zero(&value["zeros"][1]["zero"])],
            "fractional": value["fractional"],
            "tiny": value["tiny"],
            "max": value["max"],
            "keys": value.as_object().map(|object| object.keys().collect::<Vec<_>>()),
        })
    }

    fn negative_zero(value: &Value) -> bool {
        value
            .as_f64()
            .is_some_and(|n| n == 0.0 && n.is_sign_negative())
    }

    #[wasm_bindgen]
    pub async fn probe(path: String, mode: String) -> Result<String, JsValue> {
        let unauthorized = Rc::new(Cell::new(0));
        let forbidden = Rc::new(Cell::new(0));
        let client = BrowserClient::new(
            if path == "/empty" {
                "data:application/json,#"
            } else {
                ""
            },
            ClientOptions {
                on_unauthorized: Some({
                    let count = unauthorized.clone();
                    Rc::new(move || count.set(count.get() + 1))
                }),
                on_forbidden: Some({
                    let count = forbidden.clone();
                    Rc::new(move || count.set(count.get() + 1))
                }),
                ..Default::default()
            },
        );
        let settings = CallSettings::default();
        let mut route = TypedRoute::<Params, (), Numbers, ()>::new(Method::Get, &path, (), vec![])
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let result: Result<Value, RouteCallError> = match mode.as_str() {
            "typed" | "raw-typed" => {
                let value = if mode == "raw-typed" {
                    client
                        .call::<(), Numbers>("get", &path, None, &settings)
                        .await
                        .map_err(RouteCallError::Transport)
                } else {
                    client.call_route(&route, &Params {}, None, &settings).await
                };
                value.map(|value| {
                    let value = value.expect("number response");
                    json!({"n":value.n.to_string(), "negative":value.negative.to_string(),
                    "integral":value.integral.to_string(), "nested":value.nested[0].to_string()})
                })
            }
            "schema" => {
                let mut route =
                    TypedRoute::<Params, (), String, ()>::new(Method::Get, &path, (), vec![])
                        .map_err(|error| JsValue::from_str(&error.to_string()))?;
                route.definition.output = Schema::Unknown.transform(|value| {
                    Ok(value.map(|value| Value::String(value["n"].to_string())))
                });
                client
                    .call_route(&route, &Params {}, None, &settings)
                    .await
                    .map(|value| json!(value))
            }
            "negative-zero" => client
                .call::<(), Value>("get", &path, None, &settings)
                .await
                .map(|value| json!(negative_zero(&value.expect("JSON response"))))
                .map_err(RouteCallError::Transport),
            "rust-type" => client
                .call::<(), u32>("get", &path, None, &settings)
                .await
                .map(|value| json!(value))
                .map_err(RouteCallError::Transport),
            "route-rust-type" => {
                route.definition.output = Schema::Unknown;
                client
                    .call_route(&route, &Params {}, None, &settings)
                    .await
                    .map(|_| Value::Null)
            }
            "get-body" | "trace" | "invalid-header" => {
                let settings = CallSettings {
                    method_override: Some(if mode == "trace" { "TRACE" } else { "GET" }.into()),
                    auth_token_override: (mode == "invalid-header").then(|| "bad\r\nvalue".into()),
                    ..Default::default()
                };
                client
                    .call::<bool, Value>(
                        "post",
                        &path,
                        (mode != "invalid-header").then_some(&false),
                        &settings,
                    )
                    .await
                    .map(|_| Value::Null)
                    .map_err(RouteCallError::Transport)
            }
            _ => client
                .call_route_unchecked(&route, &Params {}, None, &settings)
                .await
                .map(|value| observe(value.expect("JSON response"))),
        };
        let outcome = match result {
            Ok(value) => json!({"ok":true,"value":value}),
            Err(RouteCallError::Transport(error)) => json!({"ok":false,"error":error}),
            Err(RouteCallError::Deserialization(error)) => {
                json!({"ok":false,"deserialization":error.to_string()})
            }
            Err(error) => return Err(JsValue::from_str(&error.to_string())),
        };
        serde_json::to_string(&json!({"outcome":outcome,
            "unauthorized":unauthorized.get(), "forbidden":forbidden.get()}))
        .map_err(|error| JsValue::from_str(&error.to_string()))
    }
}
