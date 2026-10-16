#![forbid(unsafe_code)]

use axum::body::Body;
use futures_util::future::BoxFuture;
use http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use std::{fmt, sync::Arc};
use url::Url;

pub mod params;
pub mod router;
pub mod schema;
pub use router::Router;
pub use schema::{Schema, ValidationError};
pub mod json;
#[cfg(feature = "server")]
pub mod logger;
#[cfg(feature = "maud")]
pub mod render;
#[cfg(feature = "server")]
pub mod server;

pub type Request = http::Request<Body>;
pub type Response = http::Response<Body>;
pub type Handler =
    Arc<dyn Fn(Request, Option<String>) -> BoxFuture<'static, Response> + Send + Sync>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

impl Method {
    pub fn parse(value: &str) -> Result<Self, HttpError> {
        match value.to_ascii_lowercase().as_str() {
            "get" => Ok(Self::Get),
            "post" => Ok(Self::Post),
            "put" => Ok(Self::Put),
            "patch" => Ok(Self::Patch),
            "delete" => Ok(Self::Delete),
            "head" => Ok(Self::Head),
            "options" => Ok(Self::Options),
            _ => Err(HttpError::new(405, format!("Method Not Allowed: {value}"))),
        }
    }
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
            Self::Put => "put",
            Self::Patch => "patch",
            Self::Delete => "delete",
            Self::Head => "head",
            Self::Options => "options",
        }
    }
    pub const fn success_status(self) -> u16 {
        match self {
            Self::Post => 201,
            Self::Delete => 204,
            _ => 200,
        }
    }
    pub const fn has_body(self) -> bool {
        matches!(self, Self::Post | Self::Put | Self::Patch)
    }
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Get => 0,
            Self::Post => 1,
            Self::Put => 2,
            Self::Patch => 3,
            Self::Delete => 4,
            Self::Head => 5,
            Self::Options => 6,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum ErrorKind {
    #[default]
    Request,
    NotFound,
    Unauthorized,
    Internal,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HttpError {
    pub status: u16,
    pub message: String,
    #[serde(default)]
    pub kind: ErrorKind,
}
impl HttpError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            kind: ErrorKind::Request,
        }
    }
    pub fn not_found() -> Self {
        Self {
            kind: ErrorKind::NotFound,
            ..Self::new(404, "Not Found - Missing entry")
        }
    }
    pub fn unauthorized() -> Self {
        Self {
            kind: ErrorKind::Unauthorized,
            ..Self::new(401, "Unauthorized")
        }
    }
    pub fn forbidden() -> Self {
        Self {
            kind: ErrorKind::Unauthorized,
            ..Self::new(403, "Forbidden")
        }
    }
    pub fn internal() -> Self {
        Self::unexpected("Internal Server Error")
    }
    pub fn unexpected(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Internal,
            ..Self::new(500, message)
        }
    }
}
impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for HttpError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionError(pub String);
impl fmt::Display for DefinitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for DefinitionError {}

#[derive(Clone, Debug)]
pub struct UploadedFile {
    pub field_name: String,
    pub filename: String,
    pub content_type: Option<String>,
    pub data: bytes::Bytes,
}

#[derive(Clone, Debug)]
pub struct RequestContext {
    pub method: Method,
    pub url: Url,
    pub headers: HeaderMap,
    pub uploads: Vec<UploadedFile>,
}
impl RequestContext {
    pub fn from_request(request: &Request) -> Result<Self, HttpError> {
        let uri = request.uri();
        let url = if uri.scheme().is_some() {
            Url::parse(&uri.to_string())
        } else {
            let host = request
                .headers()
                .get(http::header::HOST)
                .and_then(|h| h.to_str().ok())
                .unwrap_or("localhost");
            Url::parse(&format!("http://{host}{uri}"))
        }
        .map_err(|_| HttpError::new(400, "Invalid request URL"))?;
        Ok(Self {
            method: Method::parse(request.method().as_str())?,
            url,
            headers: request.headers().clone(),
            uploads: Vec::new(),
        })
    }
}

#[derive(Clone)]
pub struct RegisteredRoute {
    pub method: Method,
    pub path: String,
    pub prefixes: Vec<String>,
    pub handler: Handler,
}

pub fn response(status: u16, content_type: &str, body: impl Into<Body>) -> Response {
    let mut response = http::Response::new(body.into());
    *response.status_mut() =
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    if !content_type.is_empty() {
        if let Ok(value) = http::HeaderValue::from_str(content_type) {
            response
                .headers_mut()
                .insert(http::header::CONTENT_TYPE, value);
        }
    }
    response
}
