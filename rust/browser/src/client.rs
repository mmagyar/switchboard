use serde::{Deserialize, Serialize};
use std::{fmt, rc::Rc};

pub const NETWORK_ERROR_STATUS: u16 = 0;
pub const INVALID_RESPONSE_STATUS: u16 = 502;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientErrorKind {
    Http,
    Network,
    InvalidResponse,
    /// Invalid caller input, including JSON serialization or browser request construction.
    Request,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientError {
    pub status: u16,
    pub kind: ClientErrorKind,
    pub message: String,
}

impl ClientError {
    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn request(message: impl Into<String>) -> Self {
        Self {
            status: 0,
            kind: ClientErrorKind::Request,
            message: message.into(),
        }
    }

    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn network(message: impl Into<String>) -> Self {
        Self {
            status: NETWORK_ERROR_STATUS,
            kind: ClientErrorKind::Network,
            message: message.into(),
        }
    }

    #[cfg(any(target_arch = "wasm32", test))]
    fn invalid_response(message: impl Into<String>) -> Self {
        Self {
            status: INVALID_RESPONSE_STATUS,
            kind: ClientErrorKind::InvalidResponse,
            message: message.into(),
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ClientError {}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Credentials {
    Omit,
    #[default]
    SameOrigin,
    Include,
}

#[derive(Clone, Default)]
pub struct ClientOptions {
    pub credentials: Credentials,
    /// Applied after Accept and Content-Type defaults. Names are case-insensitive in fetch.
    pub headers: Vec<(String, String)>,
    pub on_unauthorized: Option<Rc<dyn Fn()>>,
    pub on_forbidden: Option<Rc<dyn Fn()>>,
}

#[derive(Clone, Debug, Default)]
pub struct CallSettings {
    pub method_override: Option<String>,
    /// Complete Authorization header value, including any scheme such as Bearer.
    pub auth_token_override: Option<String>,
    pub base_url_override: Option<String>,
    pub credentials: Option<Credentials>,
    /// Applied after client headers, before auth_token_override.
    pub headers: Vec<(String, String)>,
}

#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone)]
pub struct BrowserClient {
    base_url: String,
    options: ClientOptions,
}

#[cfg(any(target_arch = "wasm32", test))]
impl BrowserClient {
    /// Paths are already encoded by the application or shared route URL generator.
    /// The base URL and path are concatenated exactly, as in the Bun client.
    pub fn new(base_url: impl Into<String>, options: ClientOptions) -> Self {
        Self {
            base_url: base_url.into(),
            options,
        }
    }

    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn prepare<B: Serialize + ?Sized>(
        &self,
        method: &str,
        path: &str,
        body: Option<&B>,
        settings: &CallSettings,
    ) -> Result<PreparedRequest, ClientError> {
        let body = body
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| {
                ClientError::request(format!("Cannot serialize request body: {error}"))
            })?;
        let mut headers = vec![("Accept".into(), "application/json".into())];
        if body.is_some() {
            headers.push(("Content-Type".into(), "application/json".into()));
        }
        headers.extend(self.options.headers.iter().cloned());
        headers.extend(settings.headers.iter().cloned());
        if let Some(auth) = &settings.auth_token_override {
            headers.push(("Authorization".into(), auth.clone()));
        }
        Ok(PreparedRequest {
            url: format!(
                "{}{}",
                settings
                    .base_url_override
                    .as_deref()
                    .unwrap_or(&self.base_url),
                path
            ),
            method: settings
                .method_override
                .as_deref()
                .unwrap_or(method)
                .to_uppercase(),
            body,
            headers,
            credentials: settings.credentials.unwrap_or(self.options.credentials),
        })
    }

    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn notify_status(&self, status: u16) {
        let callback = match status {
            401 => &self.options.on_unauthorized,
            403 => &self.options.on_forbidden,
            _ => return,
        };
        if let Some(callback) = callback {
            callback();
        }
    }
}

#[cfg(any(target_arch = "wasm32", test))]
pub(crate) struct PreparedRequest {
    pub url: String,
    pub method: String,
    pub body: Option<String>,
    pub headers: Vec<(String, String)>,
    pub credentials: Credentials,
}

#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn no_response_body(method: &str, status: u16) -> bool {
    method.eq_ignore_ascii_case("head")
        || method.eq_ignore_ascii_case("delete")
        || matches!(status, 204 | 205)
}

#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn is_json(content_type: &str) -> bool {
    content_type.contains("application/json")
}

#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn decode_response<O: serde::de::DeserializeOwned>(
    method: &str,
    status: u16,
    content_type: &str,
    body: Result<String, String>,
) -> Result<Option<O>, ClientError> {
    if status >= 400 {
        let fallback = format!("Unknown error, status: {status}");
        let message = match body {
            Ok(text) if !text.is_empty() => serde_json::from_str::<serde_json::Value>(&text)
                .map(|value| value.to_string())
                .unwrap_or(text),
            _ => fallback,
        };
        return Err(ClientError {
            status,
            kind: ClientErrorKind::Http,
            message,
        });
    }
    if no_response_body(method, status) {
        return Ok(None);
    }
    let text = body.map_err(|error| {
        if is_json(content_type) {
            ClientError::invalid_response(format!("Response body is not valid JSON: {error}"))
        } else {
            ClientError::network(format!("Failed to read response body: {error}"))
        }
    })?;
    if is_json(content_type) {
        serde_json::from_str(&text).map(Some).map_err(|error| {
            ClientError::invalid_response(format!(
                "Response body is not valid JSON for the requested Rust type: {error}"
            ))
        })
    } else if text.is_empty() {
        Ok(None)
    } else {
        serde_json::from_value(serde_json::Value::String(text))
            .map(Some)
            .map_err(|error| {
                ClientError::invalid_response(format!(
                    "Response text does not match the requested Rust type: {error}"
                ))
            })
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
