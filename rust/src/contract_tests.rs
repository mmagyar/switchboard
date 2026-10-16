use crate::{Authorization, HandlerDefiner, Method, Router, route};
use axum::body::{Body, to_bytes};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema)]
struct Payload {
    title: String,
}

#[tokio::test]
async fn inferred_params_bind_real_typed_handler_with_prefix_and_body() {
    let route =
        route!(post, "/items/:itemId/:type/:slug?", "user", Payload => String, prefixes = ["de"])
            .unwrap();
    assert_eq!(route.definition.method, Method::Post);
    let define = HandlerDefiner::new(
        |_| async { Ok(()) },
        |(), _: &str, _| async { Ok(Authorization::Ok) },
    );
    let bound = define.bind_typed(route, |params, body, (), prefix| async move {
        Ok(format!(
            "{}/{}/{}/{}/{}",
            params.itemId,
            params.r#type,
            params.slug.unwrap_or_default(),
            body.expect("body validated by POST route").title,
            prefix.unwrap_or_default()
        ))
    });
    let mut router = Router::new();
    router.add_route(bound).unwrap();
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/de/items/42/news/hello")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"title":"value"}"#))
        .unwrap();
    let response = router.handle_request(request).await;
    assert_eq!(response.status(), 201);
    assert_eq!(
        to_bytes(response.into_body(), 4096).await.unwrap(),
        r#""42/news/hello/value/de""#
    );
    define.drain_background().await.unwrap();
}

#[test]
fn inferred_structs_are_hygienic_and_optional_ids_are_numbers() {
    let first = route!(get, "/items/:itemId?", (), String).unwrap();
    let second = route!(get, "/:type", (), String).unwrap();
    assert_eq!(first.definition.path, "/items/:itemId?");
    assert_eq!(second.definition.path, "/:type");
    assert_eq!(
        first
            .definition
            .params
            .parse_value(serde_json::json!({}))
            .unwrap(),
        serde_json::json!({})
    );
}
