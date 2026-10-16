use super::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::cell::Cell;

fn client(options: ClientOptions) -> BrowserClient {
    BrowserClient::new("https://example.test/api", options)
}

fn decode<O: serde::de::DeserializeOwned>(
    method: &str,
    status: u16,
    mime: &str,
    text: &str,
) -> Result<Option<O>, ClientError> {
    decode_response(method, status, mime, Ok(text.into()))
}

#[test]
fn prepare_preserves_encoded_paths_and_applies_per_call_settings() {
    let client = client(ClientOptions {
        credentials: Credentials::Include,
        headers: vec![("X-Client".into(), "one".into())],
        ..Default::default()
    });
    let settings = CallSettings {
        base_url_override: Some("https://override.test".into()),
        method_override: Some("hEaD".into()),
        credentials: Some(Credentials::Omit),
        auth_token_override: Some("Bearer token".into()),
        headers: vec![("X-Client".into(), "two".into())],
    };
    let request = client
        .prepare::<Value>("get", "/items/a%2Fb?q=x%20y", None, &settings)
        .unwrap();
    assert_eq!(request.url, "https://override.test/items/a%2Fb?q=x%20y");
    assert_eq!(request.method, "HEAD");
    assert_eq!(request.body, None);
    assert_eq!(request.credentials, Credentials::Omit);
    assert_eq!(
        request.headers,
        vec![
            ("Accept".into(), "application/json".into()),
            ("X-Client".into(), "one".into()),
            ("X-Client".into(), "two".into()),
            ("Authorization".into(), "Bearer token".into()),
        ]
    );
}

#[test]
fn defaults_and_separate_clients_are_independent() {
    let first = client(ClientOptions::default())
        .prepare::<Value>("get", "/x", None, &CallSettings::default())
        .unwrap();
    let second = BrowserClient::new("/proxy", ClientOptions::default())
        .prepare::<Value>("get", "/x", None, &CallSettings::default())
        .unwrap();
    assert_eq!(first.url, "https://example.test/api/x");
    assert_eq!(second.url, "/proxy/x");
    assert_eq!(first.method, "GET");
    assert_eq!(first.credentials, Credentials::SameOrigin);
    assert_eq!(
        first.headers,
        vec![("Accept".into(), "application/json".into())]
    );
}

#[test]
fn falsy_and_null_bodies_are_serialized_and_missing_body_is_absent() {
    let client = client(ClientOptions::default());
    for value in [json!(false), json!(0), json!(""), Value::Null] {
        let request = client
            .prepare("post", "/x", Some(&value), &CallSettings::default())
            .unwrap();
        assert_eq!(request.body, Some(value.to_string()));
        assert!(
            request
                .headers
                .contains(&("Content-Type".into(), "application/json".into()))
        );
    }
    let request = client
        .prepare::<Value>("post", "/x", None, &CallSettings::default())
        .unwrap();
    assert_eq!(request.body, None);
    assert_eq!(request.headers.len(), 1);
}

#[test]
fn unserializable_body_is_a_caller_error() {
    let invalid = std::collections::BTreeMap::from([((1, 2), "not a JSON object key")]);
    let error = client(ClientOptions::default())
        .prepare("post", "/x", Some(&invalid), &CallSettings::default())
        .err()
        .unwrap();
    assert_eq!(error.kind, ClientErrorKind::Request);
    assert_eq!(error.status, 0);
    assert!(error.message.contains("key must be a string"));
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Item {
    item_id: u32,
    title: String,
}

#[test]
fn successful_json_uses_application_serde_contracts() {
    assert_eq!(
        decode::<Item>(
            "get",
            200,
            "application/json; charset=utf-8",
            r#"{"itemId":42,"title":"ok"}"#
        )
        .unwrap(),
        Some(Item {
            item_id: 42,
            title: "ok".into()
        })
    );
    assert_eq!(
        decode::<Value>("get", 200, "application/json", "false").unwrap(),
        Some(json!(false))
    );
    assert_eq!(
        decode::<Value>("get", 200, "application/json", "null").unwrap(),
        Some(Value::Null)
    );
    for body in [
        r#"{"itemId":"42","title":"ok"}"#,
        r#"{"itemId":42}"#,
        r#"{"itemId":42,"title":"ok","extra":true}"#,
    ] {
        let error = decode::<Item>("get", 200, "application/json", body).unwrap_err();
        assert_eq!(error.kind, ClientErrorKind::InvalidResponse);
        assert_eq!(error.status, INVALID_RESPONSE_STATUS);
    }
}

#[test]
fn head_delete_204_and_205_ignore_body_and_type_contract() {
    for method in ["head", "HEAD", "delete", "DeLeTe"] {
        assert_eq!(
            decode::<Item>(method, 200, "application/json", "malformed").unwrap(),
            None
        );
        assert_eq!(
            decode_response::<Item>(method, 200, "application/json", Err("unreadable".into()))
                .unwrap(),
            None
        );
    }
    for status in [204, 205] {
        assert_eq!(
            decode::<Item>("post", status, "application/json", "malformed").unwrap(),
            None
        );
    }
    assert_eq!(decode::<Item>("get", 200, "text/plain", "").unwrap(), None);
    assert_eq!(
        decode::<Value>("get", 200, "application/json", "")
            .unwrap_err()
            .kind,
        ClientErrorKind::InvalidResponse
    );
}

#[test]
fn text_is_a_string_and_does_not_silently_coerce_to_json_or_number() {
    assert_eq!(
        decode::<String>("get", 200, "text/plain", "42").unwrap(),
        Some("42".into())
    );
    assert_eq!(
        decode::<Value>("get", 200, "text/plain", "false").unwrap(),
        Some(json!("false"))
    );
    assert_eq!(
        decode::<u32>("get", 200, "text/plain", "42")
            .unwrap_err()
            .kind,
        ClientErrorKind::InvalidResponse
    );
}

#[test]
fn http_errors_keep_status_even_for_delete_head_and_unreadable_bodies() {
    for method in ["get", "head", "delete"] {
        for (body, message) in [
            (
                Ok("  { \"error\" : \"denied\" } ".into()),
                r#"{"error":"denied"}"#,
            ),
            (Ok("\"denied\"".into()), "\"denied\""),
            (Ok("broken {".into()), "broken {"),
            (Ok(String::new()), "Unknown error, status: 403"),
            (Err("disconnected".into()), "Unknown error, status: 403"),
        ] {
            assert_eq!(
                decode_response::<Item>(method, 403, "application/json", body).unwrap_err(),
                ClientError {
                    status: 403,
                    kind: ClientErrorKind::Http,
                    message: message.into(),
                }
            );
        }
    }
}

#[test]
fn upstream_502_is_distinct_from_bad_payload_and_network_failure() {
    let http = decode::<Value>("get", 502, "text/plain", "gateway unavailable").unwrap_err();
    let invalid = decode::<Value>("get", 200, "application/json", "{").unwrap_err();
    let network = decode_response::<Value>("get", 200, "text/plain", Err("connection lost".into()))
        .unwrap_err();
    let broken_json_stream = decode_response::<Value>(
        "get",
        200,
        "application/json",
        Err("connection lost".into()),
    )
    .unwrap_err();
    assert_eq!((http.kind, http.status), (ClientErrorKind::Http, 502));
    assert_eq!(
        (invalid.kind, invalid.status),
        (ClientErrorKind::InvalidResponse, 502)
    );
    assert_eq!(
        (network.kind, network.status),
        (ClientErrorKind::Network, 0)
    );
    assert_eq!(
        network.message,
        "Failed to read response body: connection lost"
    );
    assert_eq!(
        (broken_json_stream.kind, broken_json_stream.status),
        (ClientErrorKind::InvalidResponse, 502)
    );
}

#[test]
fn authorization_callbacks_are_specific_to_status() {
    let unauthorized = Rc::new(Cell::new(0));
    let forbidden = Rc::new(Cell::new(0));
    let client = client(ClientOptions {
        on_unauthorized: Some({
            let count = unauthorized.clone();
            Rc::new(move || count.set(count.get() + 1))
        }),
        on_forbidden: Some({
            let count = forbidden.clone();
            Rc::new(move || count.set(count.get() + 1))
        }),
        ..Default::default()
    });
    client.notify_status(401);
    assert_eq!((unauthorized.get(), forbidden.get()), (1, 0));
    client.notify_status(403);
    client.notify_status(500);
    client.notify_status(200);
    assert_eq!((unauthorized.get(), forbidden.get()), (1, 1));
}
