//! Native HTTP client for typed routes.
//!
//! [`ApiClient::call_unchecked`] is the equivalent of TypeScript's
//! `validateReturn: false`: it returns JSON values instead of claiming the route's
//! output type. Native HTTP has no browser origin or CORS context. Credentials
//! default to disabled, including same-origin cookies; enabling them uses a
//! client-local cookie jar with normal cookie domain/path/security rules. Browser
//! SameSite and third-party-cookie policies cannot be enforced without an origin context.
//!
//! JSON requires finite numbers and Unicode scalar strings. Non-portable input
//! such as `1e400` or a lone UTF-16 surrogate yields `InvalidResponse` for a
//! successful JSON response; HTTP error bodies retain the original text.

#![forbid(unsafe_code)]

use crate::{HttpError, handler::TypedRoute, params::def_to_url, schema::ValidationError};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{error::Error, fmt, sync::Arc};

pub const NETWORK_ERROR_STATUS: u16 = 0;
pub const INVALID_RESPONSE_STATUS: u16 = 502;

pub type CallbackError = Box<dyn Error + Send + Sync>;
pub type AuthCallback = Arc<dyn Fn() -> Result<(), CallbackError> + Send + Sync>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiErrorKind {
    Http,
    Network,
    InvalidResponse,
}

#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
    pub kind: ApiErrorKind,
    cause: Option<CallbackError>,
}

impl ApiError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            kind: ApiErrorKind::Http,
            cause: None,
        }
    }

    fn caused(
        status: u16,
        kind: ApiErrorKind,
        context: &str,
        cause: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            status,
            message: format!("{context}: {cause}"),
            kind,
            cause: Some(Box::new(cause)),
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for ApiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.cause
            .as_ref()
            .map(|cause| cause.as_ref() as &(dyn Error + 'static))
    }
}

/// Caller, schema, and callback failures remain distinct from HTTP/transport errors.
#[derive(Debug)]
pub enum ClientError {
    Api(ApiError),
    Serialization(serde_json::Error),
    Parameters(HttpError),
    Validation(ValidationError),
    Deserialization(serde_json::Error),
    Callback(CallbackError),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api(error) => error.fmt(f),
            Self::Serialization(error) => write!(f, "Request serialization failed: {error}"),
            Self::Parameters(error) => error.fmt(f),
            Self::Validation(error) => error.fmt(f),
            Self::Deserialization(error) => write!(f, "Typed response decoding failed: {error}"),
            Self::Callback(error) => error.fmt(f),
        }
    }
}

impl Error for ClientError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(match self {
            Self::Api(error) => error,
            Self::Serialization(error) | Self::Deserialization(error) => error,
            Self::Parameters(error) => error,
            Self::Validation(error) => error,
            Self::Callback(error) => error.as_ref(),
        })
    }
}

#[derive(Clone, Default)]
pub struct ClientOptions {
    pub on_unauthorized: Option<AuthCallback>,
    pub on_forbidden: Option<AuthCallback>,
    pub with_credentials: bool,
}

/// Output validation is selected by method: `call` validates, `call_unchecked` does not.
#[derive(Clone, Debug, Default)]
pub struct CallSettings {
    pub method_override: Option<String>,
    /// The complete Authorization header value, without an implicit Bearer prefix.
    pub auth_token_override: Option<String>,
    pub base_url_override: Option<String>,
    pub with_credentials: Option<bool>,
}

#[derive(Clone)]
pub struct ApiClient {
    base_url: String,
    options: ClientOptions,
    anonymous: reqwest::Client,
    credentialed: reqwest::Client,
}

impl ApiClient {
    /// Native requests require an absolute base URL because there is no page URL to resolve against.
    pub fn new(
        base_url: impl Into<String>,
        options: ClientOptions,
    ) -> Result<Self, reqwest::Error> {
        Ok(Self {
            base_url: base_url.into(),
            options,
            // Separate transports ensure a disabled call neither sends nor stores cookies.
            anonymous: reqwest::Client::builder().build()?,
            credentialed: reqwest::Client::builder().cookie_store(true).build()?,
        })
    }

    pub async fn call<P, B, O, Permission>(
        &self,
        route: &TypedRoute<P, B, O, Permission>,
        params: &P,
        body: Option<&B>,
        settings: CallSettings,
    ) -> Result<Option<O>, ClientError>
    where
        P: Serialize,
        B: Serialize,
        O: DeserializeOwned,
    {
        let Some(value) = self.call_unchecked(route, params, body, settings).await? else {
            return Ok(None);
        };
        let validated = route
            .definition
            .output
            .parse_value(value)
            .map_err(ClientError::Validation)?;
        serde_json::from_value(validated)
            .map(Some)
            .map_err(ClientError::Deserialization)
    }

    /// Skips the output schema and returns an untrusted value. Empty responses are `None`.
    pub async fn call_unchecked<P, B, O, Permission>(
        &self,
        route: &TypedRoute<P, B, O, Permission>,
        params: &P,
        body: Option<&B>,
        settings: CallSettings,
    ) -> Result<Option<Value>, ClientError>
    where
        P: Serialize,
        B: Serialize,
    {
        let params = serde_json::to_value(params).map_err(ClientError::Serialization)?;
        let path = def_to_url(&route.definition.path, &route.definition.params, params)
            .map_err(ClientError::Parameters)?;
        // Serialization precedes transport, including for false, zero, empty strings and null.
        let body = body
            .map(|body| serde_json::to_value(body).and_then(|value| crate::json::stringify(&value)))
            .transpose()
            .map_err(ClientError::Serialization)?;
        let method = settings
            .method_override
            .as_deref()
            .unwrap_or(route.definition.method.as_str());
        let effective_method = method.to_ascii_lowercase();
        let method = reqwest::Method::from_bytes(method.to_ascii_uppercase().as_bytes()).map_err(
            |error| {
                ClientError::Api(ApiError::caused(
                    NETWORK_ERROR_STATUS,
                    ApiErrorKind::Network,
                    "Network request failed",
                    error,
                ))
            },
        )?;
        if body.is_some() && (method == reqwest::Method::GET || method == reqwest::Method::HEAD) {
            return Err(ClientError::Api(ApiError::caused(
                NETWORK_ERROR_STATUS,
                ApiErrorKind::Network,
                "Network request failed",
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "fetch() request with GET/HEAD method cannot have body.",
                ),
            )));
        }
        let url = format!(
            "{}{}",
            settings
                .base_url_override
                .as_deref()
                .unwrap_or(&self.base_url),
            path
        );
        let transport = if settings
            .with_credentials
            .unwrap_or(self.options.with_credentials)
        {
            &self.credentialed
        } else {
            &self.anonymous
        };
        let mut request = transport
            .request(method, url)
            .header(ACCEPT, "application/json");
        if let Some(auth) = settings.auth_token_override {
            request = request.header(AUTHORIZATION, auth);
        }
        if let Some(body) = body {
            request = request.header(CONTENT_TYPE, "application/json").body(body);
        }
        let raw = request.send().await.map_err(|error| {
            ClientError::Api(ApiError::caused(
                NETWORK_ERROR_STATUS,
                ApiErrorKind::Network,
                "Network request failed",
                error,
            ))
        })?;
        let status = raw.status().as_u16();
        if status >= 400 {
            let callback = match status {
                401 => self.options.on_unauthorized.as_ref(),
                403 => self.options.on_forbidden.as_ref(),
                _ => None,
            };
            if let Some(callback) = callback {
                callback().map_err(ClientError::Callback)?;
            }
            let mut error = ApiError::new(status, format!("Unknown error, status: {status}"));
            match response_text(raw).await {
                Ok(text) if !text.is_empty() => {
                    error.message = match serde_json::from_str::<Value>(&text) {
                        Ok(value) => {
                            crate::json::stringify(&value).map_err(ClientError::Serialization)?
                        }
                        Err(_) => text,
                    };
                }
                Ok(_) => {}
                // An unreadable error body must retain the server's actual HTTP status.
                Err(cause) => error.cause = Some(Box::new(cause)),
            }
            return Err(ClientError::Api(error));
        }
        if effective_method == "delete"
            || effective_method == "head"
            || status == 204
            || status == 205
        {
            return Ok(None);
        }
        let json = raw
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .contains("application/json");
        let text = response_text(raw).await.map_err(|error| {
            ClientError::Api(if json {
                ApiError::caused(
                    INVALID_RESPONSE_STATUS,
                    ApiErrorKind::InvalidResponse,
                    "Response body is not valid JSON",
                    error,
                )
            } else {
                ApiError::caused(
                    NETWORK_ERROR_STATUS,
                    ApiErrorKind::Network,
                    "Failed to read response body",
                    error,
                )
            })
        })?;
        if json {
            crate::json::parse(&text).map(Some).map_err(|error| {
                ClientError::Api(ApiError::caused(
                    INVALID_RESPONSE_STATUS,
                    ApiErrorKind::InvalidResponse,
                    "Response body is not valid JSON",
                    error,
                ))
            })
        } else {
            Ok((!text.is_empty()).then_some(Value::String(text)))
        }
    }
}

// Fetch decodes UTF-8 and strips one BOM, regardless of Content-Type charset.
async fn response_text(raw: reqwest::Response) -> Result<String, reqwest::Error> {
    let bytes = raw.bytes().await?;
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Method, schema::Schema};
    use schemars::JsonSchema;
    use serde::{Deserialize, Serializer};
    use serde_json::json;
    use std::{collections::HashMap, io, time::Duration};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::JoinHandle,
    };

    #[derive(Debug, Serialize, Deserialize, JsonSchema)]
    struct Empty {}

    #[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
    struct Output {
        value: i32,
    }

    #[derive(Debug)]
    struct CapturedRequest {
        method: String,
        target: String,
        headers: HashMap<String, String>,
        body: String,
    }

    fn route<B: JsonSchema, O: JsonSchema>(method: Method) -> TypedRoute<Empty, B, O, ()> {
        TypedRoute::new(method, "/ping", (), vec![]).expect("valid route")
    }

    fn client(url: &str) -> ApiClient {
        ApiClient::new(url, ClientOptions::default()).expect("client builds")
    }

    fn response(status: u16, content_type: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn gzip_response(status: u16, body: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    }

    async fn peer(
        responses: Vec<impl Into<Vec<u8>>>,
    ) -> (String, JoinHandle<Vec<CapturedRequest>>) {
        let responses: Vec<Vec<u8>> = responses.into_iter().map(Into::into).collect();
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind peer");
        let address = format!("http://{}", listener.local_addr().expect("local address"));
        let handle = tokio::spawn(async move {
            let mut requests = Vec::new();
            for response in responses {
                let (mut stream, _) =
                    tokio::time::timeout(Duration::from_secs(2), listener.accept())
                        .await
                        .expect("request arrives")
                        .expect("accept request");
                let mut bytes = Vec::new();
                let header_end = loop {
                    let mut chunk = [0; 1024];
                    let count =
                        tokio::time::timeout(Duration::from_secs(2), stream.read(&mut chunk))
                            .await
                            .expect("request readable")
                            .expect("read request");
                    assert_ne!(count, 0, "request ends after headers");
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(end) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
                        break end + 4;
                    }
                    assert!(bytes.len() < 65536, "bounded request headers");
                };
                let header =
                    String::from_utf8(bytes[..header_end].to_vec()).expect("UTF-8 headers");
                let mut lines = header.split("\r\n");
                let mut first = lines.next().expect("request line").split_whitespace();
                let method = first.next().expect("method").to_owned();
                let target = first.next().expect("target").to_owned();
                let headers: HashMap<String, String> = lines
                    .filter_map(|line| line.split_once(':'))
                    .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
                    .collect();
                let length = headers
                    .get("content-length")
                    .map(|value| value.parse::<usize>().expect("body length"))
                    .unwrap_or(0);
                while bytes.len() - header_end < length {
                    let mut chunk = [0; 1024];
                    let count =
                        tokio::time::timeout(Duration::from_secs(2), stream.read(&mut chunk))
                            .await
                            .expect("body readable")
                            .expect("read body");
                    assert_ne!(count, 0, "complete body");
                    bytes.extend_from_slice(&chunk[..count]);
                }
                requests.push(CapturedRequest {
                    method,
                    target,
                    headers,
                    body: String::from_utf8(bytes[header_end..].to_vec()).expect("UTF-8 body"),
                });
                stream.write_all(&response).await.expect("write response");
            }
            requests
        });
        (address, handle)
    }

    fn api(error: ClientError, status: u16, kind: ApiErrorKind) -> ApiError {
        let ClientError::Api(error) = error else {
            panic!("expected ApiError, got {error:?}")
        };
        assert_eq!(error.status, status);
        assert_eq!(error.kind, kind);
        error
    }

    #[tokio::test]
    async fn client_api_call_validates_strips_and_unchecked_keeps_unknown_fields() {
        let (url, handle) = peer(vec![
            response(
                200,
                "application/json",
                r#"{"value":99,"extra":true}"#
            );
            3
        ])
        .await;
        let client = client(&url);
        let mut route = route::<Value, Value>(Method::Get);
        route.definition.output =
            Schema::Object([("value".into(), Schema::Number)].into_iter().collect());
        let result = client
            .call(&route, &Empty {}, None, CallSettings::default())
            .await
            .expect("valid output");
        assert_eq!(result, Some(json!({"value":99})));
        assert_eq!(
            client
                .call_unchecked(&route, &Empty {}, None, CallSettings::default())
                .await
                .expect("unchecked output"),
            Some(json!({"value":99,"extra":true}))
        );
        let typed = self::route::<Value, Output>(Method::Get);
        assert_eq!(
            client
                .call(&typed, &Empty {}, None, CallSettings::default())
                .await
                .expect("typed output"),
            Some(Output { value: 99 })
        );
        let requests = handle.await.expect("peer completes");
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].target, "/ping");
        assert_eq!(
            requests[0].headers.get("accept").map(String::as_str),
            Some("application/json")
        );
        assert!(!requests[0].headers.contains_key("authorization"));
    }

    #[tokio::test]
    async fn client_api_call_unchecked_normalizes_nested_json_numbers() {
        let (url, handle) = peer(vec![response(
            200,
            "application/json",
            r#"{"10":9007199254740993,"2":[-9007199254740993,{"n":9007199254740995}],"fraction":1.25,"zero":-0}"#,
        )])
        .await;
        let value = client(&url)
            .call_unchecked(
                &route::<Value, Value>(Method::Get),
                &Empty {},
                None,
                CallSettings::default(),
            )
            .await
            .expect("JSON response")
            .expect("nonempty response");
        assert_eq!(value["10"], json!(9_007_199_254_740_992_u64));
        assert_eq!(value["2"][0], json!(-9_007_199_254_740_992_i64));
        assert_eq!(value["2"][1]["n"], json!(9_007_199_254_740_996_u64));
        assert_eq!(value["fraction"], json!(1.25));
        assert!(value["zero"].as_f64().expect("number").is_sign_negative());
        assert_eq!(
            value
                .as_object()
                .expect("object")
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["2", "10", "fraction", "zero"]
        );
        handle.await.expect("peer completes");
    }

    #[tokio::test]
    async fn client_api_call_output_schema_observes_rounded_json_numbers() {
        let (url, handle) = peer(vec![response(200, "application/json", "9007199254740993")]).await;
        let mut route = route::<Value, Value>(Method::Get);
        route.definition.output = Schema::Number.refine(|value| {
            assert_eq!(value, Some(&json!(9_007_199_254_740_992_u64)));
            Ok(())
        });
        assert_eq!(
            client(&url)
                .call(&route, &Empty {}, None, CallSettings::default())
                .await
                .expect("schema sees the rounded value"),
            Some(json!(9_007_199_254_740_992_u64))
        );
        handle.await.expect("peer completes");
    }

    #[tokio::test]
    async fn client_api_call_typed_integral_output_decodes_after_json_rounding() {
        #[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
        struct Numbers {
            positive: u64,
            negative: i64,
            integral_float: i32,
            nested: Vec<u64>,
        }
        let (url, handle) = peer(vec![response(
            200,
            "application/json",
            r#"{"positive":9007199254740993,"negative":-9007199254740993,"integral_float":1.0,"nested":[9007199254740995]}"#,
        )])
        .await;
        assert_eq!(
            client(&url)
                .call(
                    &route::<Value, Numbers>(Method::Get),
                    &Empty {},
                    None,
                    CallSettings::default(),
                )
                .await
                .expect("typed integral output"),
            Some(Numbers {
                positive: 9_007_199_254_740_992,
                negative: -9_007_199_254_740_992,
                integral_float: 1,
                nested: vec![9_007_199_254_740_996],
            })
        );
        handle.await.expect("peer completes");
    }

    #[tokio::test]
    async fn client_api_call_gzip_success_decodes_on_both_credential_transports() {
        #[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
        struct RoundedOutput {
            value: u64,
        }
        // gzip.compress(b'{"value":9007199254740993}', mtime=0)
        let compressed = [
            31, 139, 8, 0, 0, 0, 0, 0, 2, 3, 171, 86, 42, 75, 204, 41, 77, 85, 178, 178, 52, 48,
            48, 55, 180, 180, 52, 50, 53, 49, 55, 49, 176, 180, 52, 174, 5, 0, 166, 65, 67, 250,
            26, 0, 0, 0,
        ];
        for credentials in [false, true] {
            let (url, handle) = peer(vec![gzip_response(200, &compressed); 2]).await;
            let client = ApiClient::new(
                &url,
                ClientOptions {
                    with_credentials: !credentials,
                    ..ClientOptions::default()
                },
            )
            .expect("client");
            let settings = CallSettings {
                with_credentials: Some(credentials),
                ..CallSettings::default()
            };
            let route = route::<Value, RoundedOutput>(Method::Get);
            assert_eq!(
                client
                    .call_unchecked(&route, &Empty {}, None, settings.clone())
                    .await
                    .expect("gzip JSON"),
                Some(json!({"value":9_007_199_254_740_992_u64}))
            );
            assert_eq!(
                client
                    .call(&route, &Empty {}, None, settings)
                    .await
                    .expect("typed gzip JSON"),
                Some(RoundedOutput {
                    value: 9_007_199_254_740_992
                })
            );
            handle.await.expect("peer completes");
        }
    }

    #[tokio::test]
    async fn client_api_call_gzip_errors_preserve_auth_callbacks_status_and_decoded_detail() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        // gzip.compress(b'"login required"', mtime=0), gzip.compress(b'denied', mtime=0)
        let login = [
            31, 139, 8, 0, 0, 0, 0, 0, 2, 3, 83, 202, 201, 79, 207, 204, 83, 40, 74, 45, 44, 205,
            44, 74, 77, 81, 2, 0, 36, 111, 117, 178, 16, 0, 0, 0,
        ];
        let denied = [
            31, 139, 8, 0, 0, 0, 0, 0, 2, 3, 75, 73, 205, 203, 76, 77, 1, 0, 238, 94, 223, 189, 6,
            0, 0, 0,
        ];
        for credentials in [false, true] {
            let (url, handle) = peer(vec![
                gzip_response(401, &login),
                gzip_response(403, &denied),
            ])
            .await;
            let unauthorized = Arc::new(AtomicUsize::new(0));
            let forbidden = Arc::new(AtomicUsize::new(0));
            let client = ApiClient::new(
                &url,
                ClientOptions {
                    with_credentials: credentials,
                    on_unauthorized: Some({
                        let count = unauthorized.clone();
                        Arc::new(move || {
                            count.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        })
                    }),
                    on_forbidden: Some({
                        let count = forbidden.clone();
                        Arc::new(move || {
                            count.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        })
                    }),
                },
            )
            .expect("client");
            for (status, detail, a, b) in [(401, "\"login required\"", 1, 0), (403, "denied", 1, 1)]
            {
                let error = client
                    .call(
                        &route::<Value, Value>(Method::Get),
                        &Empty {},
                        None,
                        CallSettings::default(),
                    )
                    .await
                    .expect_err("HTTP error");
                assert_eq!(api(error, status, ApiErrorKind::Http).message, detail);
                assert_eq!(unauthorized.load(Ordering::SeqCst), a);
                assert_eq!(forbidden.load(Ordering::SeqCst), b);
            }
            handle.await.expect("peer completes");
        }
    }

    #[tokio::test]
    async fn client_api_call_validation_and_typed_decoding_errors_stay_separate() {
        let (url, handle) = peer(vec![
            response(200, "application/json", r#"{"wrong":"shape"}"#),
            response(200, "application/json", r#"{"wrong":"shape"}"#),
            response(200, "application/json", "42"),
        ])
        .await;
        let client = client(&url);
        let mut route = route::<Value, Output>(Method::Get);
        assert!(matches!(
            client
                .call(&route, &Empty {}, None, CallSettings::default())
                .await,
            Err(ClientError::Validation(_))
        ));
        assert_eq!(
            client
                .call_unchecked(&route, &Empty {}, None, CallSettings::default())
                .await
                .expect("unchecked mismatch"),
            Some(json!({"wrong":"shape"}))
        );
        route.definition.output = Schema::Unknown;
        assert!(matches!(
            client
                .call(&route, &Empty {}, None, CallSettings::default())
                .await,
            Err(ClientError::Deserialization(_))
        ));
        handle.await.expect("peer completes");
    }

    #[tokio::test]
    async fn client_api_call_falsy_bodies_and_omission_are_distinct() {
        let (url, handle) = peer(vec![response(200, "application/json", "null"); 5]).await;
        let client = client(&url);
        let route = route::<Value, Value>(Method::Post);
        for body in [json!(false), json!(0), json!(""), Value::Null] {
            assert_eq!(
                client
                    .call(&route, &Empty {}, Some(&body), CallSettings::default())
                    .await
                    .expect("falsy body"),
                Some(Value::Null)
            );
        }
        client
            .call(&route, &Empty {}, None, CallSettings::default())
            .await
            .expect("omitted body");
        let requests = handle.await.expect("peer completes");
        for (request, body) in requests.iter().zip(["false", "0", "\"\"", "null"]) {
            assert_eq!(request.method, "POST");
            assert_eq!(request.body, body);
            assert_eq!(
                request.headers.get("content-type").map(String::as_str),
                Some("application/json")
            );
        }
        assert_eq!(requests[4].body, "");
        assert!(!requests[4].headers.contains_key("content-type"));
    }

    #[tokio::test]
    async fn client_api_call_overrides_and_instances_are_independent() {
        #[derive(Serialize, Deserialize, JsonSchema)]
        struct Params {
            #[serde(rename = "itemId")]
            item_id: u32,
            query: String,
        }
        let (first, a) = peer(vec![response(200, "application/json", "1")]).await;
        let (second, b) = peer(vec![response(200, "application/json", "2"); 2]).await;
        let first_client = client(&first);
        let second_client = client(&second);
        let route =
            TypedRoute::<Params, Value, Value, ()>::new(Method::Get, "/items/:itemId", (), vec![])
                .expect("parameter route");
        let params = Params {
            item_id: 12,
            query: "a b&c".into(),
        };
        assert_eq!(
            first_client
                .call(&route, &params, None, CallSettings::default())
                .await
                .expect("first client"),
            Some(json!(1))
        );
        assert_eq!(
            second_client
                .call(&route, &params, None, CallSettings::default())
                .await
                .expect("second client"),
            Some(json!(2))
        );
        let settings = CallSettings {
            base_url_override: Some(second),
            method_override: Some("patch".into()),
            auth_token_override: Some("Basic raw-token".into()),
            ..CallSettings::default()
        };
        assert_eq!(
            first_client
                .call(&route, &params, None, settings)
                .await
                .expect("overrides"),
            Some(json!(2))
        );
        let a = a.await.expect("first peer");
        let b = b.await.expect("second peer");
        assert!(a[0].target.starts_with("/items/12?"));
        let url = url::Url::parse(&format!("http://localhost{}", a[0].target)).expect("target URL");
        assert_eq!(
            url.query_pairs().into_owned().collect::<Vec<_>>(),
            vec![("query".to_owned(), "a b&c".to_owned())]
        );
        assert_eq!(b[1].method, "PATCH");
        assert_eq!(
            b[1].headers.get("authorization").map(String::as_str),
            Some("Basic raw-token")
        );
        assert!(!b[0].headers.contains_key("authorization"));
    }

    #[tokio::test]
    async fn client_api_call_empty_and_text_responses_follow_effective_method() {
        for (method, settings, status, content_type, body) in [
            (
                Method::Get,
                CallSettings::default(),
                204,
                "application/json",
                "",
            ),
            (
                Method::Get,
                CallSettings::default(),
                205,
                "application/json",
                "",
            ),
            (
                Method::Get,
                CallSettings {
                    method_override: Some("HEAD".into()),
                    ..CallSettings::default()
                },
                200,
                "application/json",
                "",
            ),
            (
                Method::Head,
                CallSettings::default(),
                200,
                "application/json",
                "",
            ),
            (
                Method::Delete,
                CallSettings::default(),
                200,
                "application/json",
                "ignored",
            ),
            (
                Method::Get,
                CallSettings {
                    method_override: Some("DELETE".into()),
                    ..CallSettings::default()
                },
                200,
                "application/json",
                "ignored",
            ),
            (Method::Get, CallSettings::default(), 200, "text/plain", ""),
        ] {
            let (url, handle) = peer(vec![response(status, content_type, body)]).await;
            assert_eq!(
                client(&url)
                    .call(&route::<Value, Output>(method), &Empty {}, None, settings)
                    .await
                    .expect("empty response"),
                None
            );
            handle.await.expect("peer completes");
        }
        let (url, handle) = peer(vec![response(200, "text/plain", "hello world")]).await;
        assert_eq!(
            client(&url)
                .call(
                    &route::<Value, String>(Method::Get),
                    &Empty {},
                    None,
                    CallSettings::default()
                )
                .await
                .expect("plain text"),
            Some("hello world".into())
        );
        handle.await.expect("peer completes");
    }

    #[tokio::test]
    async fn client_api_call_response_decoding_uses_fetch_utf8_and_bom_rules() {
        for (content_type, body, expected) in [
            (
                "application/json; charset=utf-8",
                "\u{feff}{\"value\":1}",
                json!({"value":1}),
            ),
            (
                "text/plain; charset=iso-8859-1",
                "\u{feff}héllo",
                json!("héllo"),
            ),
        ] {
            let (url, handle) = peer(vec![response(200, content_type, body)]).await;
            assert_eq!(
                client(&url)
                    .call(
                        &route::<Value, Value>(Method::Get),
                        &Empty {},
                        None,
                        CallSettings::default()
                    )
                    .await
                    .expect("decoded output"),
                Some(expected)
            );
            handle.await.expect("peer completes");
        }
    }

    #[tokio::test]
    async fn client_api_call_http_error_bodies_retain_status_and_detail() {
        for (status, body, expected) in [
            (
                404,
                "{ \"message\": \"not found\" }",
                r#"{"message":"not found"}"#,
            ),
            (401, "\"login required\"", "\"login required\""),
            (403, "false", "false"),
            (
                500,
                "<html>gateway down</html>",
                "<html>gateway down</html>",
            ),
            (502, "{broken", "{broken"),
            (503, "", "Unknown error, status: 503"),
            (400, "\u{feff}oops", "oops"),
            (422, "1e400", "1e400"),
            (422, r#""\ud800""#, r#""\ud800""#),
        ] {
            let (url, handle) = peer(vec![response(status, "application/json", body)]).await;
            let error = client(&url)
                .call(
                    &route::<Value, Output>(Method::Get),
                    &Empty {},
                    None,
                    CallSettings::default(),
                )
                .await
                .expect_err("HTTP failure");
            assert_eq!(api(error, status, ApiErrorKind::Http).message, expected);
            handle.await.expect("peer completes");
        }
    }

    #[tokio::test]
    async fn client_api_call_json_stringification_matches_number_and_key_order_rules() {
        let (url, handle) = peer(vec![response(
            422,
            "text/plain",
            r#"{ "10": 1.0, "2": 1e21, "01": -0.0, "n": 9007199254740993 }"#,
        )])
        .await;
        let body = json!({"10": 1.0, "2": 1e21, "01": -0.0, "n": 9007199254740993u64});
        let expected = r#"{"2":1e+21,"10":1,"01":0,"n":9007199254740992}"#;
        let error = client(&url)
            .call(
                &route::<Value, Value>(Method::Post),
                &Empty {},
                Some(&body),
                CallSettings::default(),
            )
            .await
            .expect_err("HTTP error");
        assert_eq!(api(error, 422, ApiErrorKind::Http).message, expected);
        let requests = handle.await.expect("peer completes");
        assert_eq!(requests[0].body, expected);
    }

    #[tokio::test]
    async fn client_api_call_malformed_json_and_unreadable_bodies_are_classified() {
        for (status, content_type, body, expected_status, kind, message) in [
            (
                200,
                "application/json",
                "{truncated",
                INVALID_RESPONSE_STATUS,
                ApiErrorKind::InvalidResponse,
                "not valid JSON",
            ),
            (
                200,
                "application/json",
                "1e400",
                INVALID_RESPONSE_STATUS,
                ApiErrorKind::InvalidResponse,
                "not valid JSON",
            ),
            (
                200,
                "application/json",
                r#""\ud800""#,
                INVALID_RESPONSE_STATUS,
                ApiErrorKind::InvalidResponse,
                "not valid JSON",
            ),
            (
                200,
                "application/json",
                "",
                INVALID_RESPONSE_STATUS,
                ApiErrorKind::InvalidResponse,
                "not valid JSON",
            ),
            (
                200,
                "application/json",
                "truncated transport",
                INVALID_RESPONSE_STATUS,
                ApiErrorKind::InvalidResponse,
                "not valid JSON",
            ),
            (
                200,
                "text/plain",
                "truncated transport",
                NETWORK_ERROR_STATUS,
                ApiErrorKind::Network,
                "Failed to read response body",
            ),
            (
                401,
                "application/json",
                "truncated transport",
                401,
                ApiErrorKind::Http,
                "Unknown error, status: 401",
            ),
        ] {
            let raw = if body == "truncated transport" {
                format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nContent-Length: 1000\r\nConnection: close\r\n\r\nshort"
                )
            } else {
                response(status, content_type, body)
            };
            let (url, handle) = peer(vec![raw]).await;
            let error = client(&url)
                .call(
                    &route::<Value, Output>(Method::Get),
                    &Empty {},
                    None,
                    CallSettings::default(),
                )
                .await
                .expect_err("unusable response");
            let error = api(error, expected_status, kind);
            assert!(error.message.contains(message));
            assert!(error.source().is_some());
            handle.await.expect("peer completes");
        }
    }

    #[tokio::test]
    async fn client_api_call_callbacks_are_status_specific_and_preserve_auth_bodies() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let unauthorized = Arc::new(AtomicUsize::new(0));
        let forbidden = Arc::new(AtomicUsize::new(0));
        let (url, handle) = peer(vec![
            response(401, "text/plain", "login"),
            response(403, "text/plain", "denied"),
            response(500, "text/plain", "failed"),
        ])
        .await;
        let client = ApiClient::new(
            &url,
            ClientOptions {
                on_unauthorized: Some({
                    let count = unauthorized.clone();
                    Arc::new(move || {
                        count.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                }),
                on_forbidden: Some({
                    let count = forbidden.clone();
                    Arc::new(move || {
                        count.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                }),
                ..ClientOptions::default()
            },
        )
        .expect("client");
        for (status, body, a, b) in [
            (401, "login", 1, 0),
            (403, "denied", 1, 1),
            (500, "failed", 1, 1),
        ] {
            let error = client
                .call(
                    &route::<Value, Value>(Method::Get),
                    &Empty {},
                    None,
                    CallSettings::default(),
                )
                .await
                .expect_err("HTTP failure");
            assert_eq!(api(error, status, ApiErrorKind::Http).message, body);
            assert_eq!(unauthorized.load(Ordering::SeqCst), a);
            assert_eq!(forbidden.load(Ordering::SeqCst), b);
        }
        handle.await.expect("peer completes");
    }

    #[tokio::test]
    async fn client_api_call_callback_failure_precedes_body_read_and_retains_original_error() {
        for (status, encoding) in [
            (401, ""),
            (403, ""),
            (401, "Content-Encoding: gzip\r\n"),
            (403, "Content-Encoding: gzip\r\n"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind peer");
            let url = format!("http://{}", listener.local_addr().expect("local address"));
            let notified = Arc::new(tokio::sync::Notify::new());
            let signal = notified.clone();
            let peer = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let mut buffer = [0; 4096];
                assert_ne!(stream.read(&mut buffer).await.expect("request"), 0);
                stream
                    .write_all(
                        format!("HTTP/1.1 {status} Test\r\n{encoding}Content-Length: 1000\r\n\r\n")
                            .as_bytes(),
                    )
                    .await
                    .expect("headers");
                tokio::time::timeout(Duration::from_secs(2), signal.notified())
                    .await
                    .expect("callback runs before body read");
            });
            let callback: AuthCallback = Arc::new(move || {
                notified.notify_one();
                Err(Box::new(io::Error::other("redirect to login")))
            });
            let options = if status == 401 {
                ClientOptions {
                    on_unauthorized: Some(callback),
                    ..ClientOptions::default()
                }
            } else {
                ClientOptions {
                    on_forbidden: Some(callback),
                    ..ClientOptions::default()
                }
            };
            let client = ApiClient::new(url, options).expect("client");
            let error = tokio::time::timeout(
                Duration::from_secs(3),
                client.call(
                    &route::<Value, Value>(Method::Get),
                    &Empty {},
                    None,
                    CallSettings::default(),
                ),
            )
            .await
            .expect("callback completes without body")
            .expect_err("callback failure");
            let ClientError::Callback(error) = error else {
                panic!("callback error expected: {error:?}")
            };
            assert_eq!(
                error
                    .downcast_ref::<io::Error>()
                    .expect("original callback error type")
                    .to_string(),
                "redirect to login"
            );
            peer.await.expect("peer completes");
        }
    }

    #[tokio::test]
    async fn client_api_call_network_failure_has_sentinel_and_source() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind unused port");
        let url = format!("http://{}", listener.local_addr().expect("local address"));
        drop(listener);
        let error = client(&url)
            .call(
                &route::<Value, Value>(Method::Get),
                &Empty {},
                None,
                CallSettings::default(),
            )
            .await
            .expect_err("connection refused");
        let error = api(error, NETWORK_ERROR_STATUS, ApiErrorKind::Network);
        assert!(error.message.starts_with("Network request failed:"));
        assert!(error.source().is_some());
    }

    #[tokio::test]
    async fn client_api_call_serialization_failure_precedes_transport() {
        #[derive(Deserialize, JsonSchema)]
        struct Unserializable {}
        impl Serialize for Unserializable {
            fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("cannot serialize body"))
            }
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind peer");
        let url = format!("http://{}", listener.local_addr().expect("local address"));
        let route = route::<Unserializable, Value>(Method::Post);
        let error = client(&url)
            .call(
                &route,
                &Empty {},
                Some(&Unserializable {}),
                CallSettings::default(),
            )
            .await
            .expect_err("serialization failure");
        let ClientError::Serialization(error) = error else {
            panic!("serialization error expected: {error:?}")
        };
        assert_eq!(error.to_string(), "cannot serialize body");
        let params_route =
            TypedRoute::<Unserializable, Value, Value, ()>::new(Method::Get, "/ping", (), vec![])
                .expect("route");
        assert!(matches!(
            client(&url)
                .call(
                    &params_route,
                    &Unserializable {},
                    None,
                    CallSettings::default()
                )
                .await,
            Err(ClientError::Serialization(_))
        ));
        let mut invalid_params_route = self::route::<Value, Value>(Method::Get);
        invalid_params_route.definition.params =
            Schema::Object([("required".into(), Schema::Number)].into_iter().collect());
        let error = client(&url)
            .call(
                &invalid_params_route,
                &Empty {},
                None,
                CallSettings::default(),
            )
            .await
            .expect_err("invalid parameters");
        let ClientError::Parameters(error) = error else {
            panic!("parameter error expected: {error:?}")
        };
        assert_eq!(error.status, 400);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err(),
            "no connection attempted"
        );
    }

    #[tokio::test]
    async fn client_api_call_get_and_head_with_bodies_fail_before_transport() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind peer");
        let url = format!("http://{}", listener.local_addr().expect("local address"));
        let client = client(&url);
        let route = route::<Value, Value>(Method::Post);
        for method in ["GET", "HEAD"] {
            let settings = CallSettings {
                method_override: Some(method.into()),
                ..CallSettings::default()
            };
            let error = tokio::time::timeout(
                Duration::from_millis(100),
                client.call(&route, &Empty {}, Some(&Value::Null), settings),
            )
            .await
            .expect("rejected before transport")
            .expect_err("forbidden request body");
            assert!(
                api(error, NETWORK_ERROR_STATUS, ApiErrorKind::Network)
                    .message
                    .contains("GET/HEAD method cannot have body")
            );
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err(),
            "no connection attempted"
        );
    }

    #[tokio::test]
    async fn client_api_call_credentials_are_opt_in_overridable_and_client_local() {
        let set_cookie = "HTTP/1.1 200 OK\r\nSet-Cookie: session=secret; Path=/\r\nContent-Length: 4\r\nConnection: close\r\n\r\nnull".to_owned();
        let plain = response(200, "application/json", "null");
        let disabled_cookie = set_cookie.replace("secret", "revoked");
        let (url, handle) = peer(vec![
            set_cookie.clone(),
            plain.clone(),
            set_cookie.clone(),
            disabled_cookie.clone(),
            plain.clone(),
            set_cookie,
            plain.clone(),
            disabled_cookie,
            plain,
        ])
        .await;
        let default_client = client(&url);
        let credentialed = ApiClient::new(
            &url,
            ClientOptions {
                with_credentials: true,
                ..ClientOptions::default()
            },
        )
        .expect("client");
        let route = route::<Value, Value>(Method::Get);
        for setting in [None, None, Some(true), Some(false), Some(true)] {
            default_client
                .call_unchecked(
                    &route,
                    &Empty {},
                    None,
                    CallSettings {
                        with_credentials: setting,
                        ..CallSettings::default()
                    },
                )
                .await
                .expect("default cookie behavior");
        }
        for setting in [None, None, Some(false), None] {
            credentialed
                .call_unchecked(
                    &route,
                    &Empty {},
                    None,
                    CallSettings {
                        with_credentials: setting,
                        ..CallSettings::default()
                    },
                )
                .await
                .expect("credentialed cookie behavior");
        }
        let requests = handle.await.expect("peer completes");
        for (index, request) in requests.iter().enumerate() {
            assert_eq!(
                request.headers.get("cookie").map(String::as_str),
                if index == 4 || index == 6 || index == 8 {
                    Some("session=secret")
                } else {
                    None
                },
                "cookie isolation on request {index}"
            );
        }
    }
}
