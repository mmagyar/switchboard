use crate::client::{decode_response, no_response_body};
use crate::{BrowserClient, CallSettings, ClientError, Credentials};
use serde::{Serialize, de::DeserializeOwned};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, RequestCredentials, RequestInit, Response};

impl BrowserClient {
    /// Deserializes successful JSON into the application's shared Rust type.
    /// Use serde_json::Value for untyped data or String for a text response.
    /// HEAD, DELETE, 204, 205 and empty non-JSON responses return None.
    pub async fn call<B: Serialize + ?Sized, O: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<&B>,
        settings: &CallSettings,
    ) -> Result<Option<O>, ClientError> {
        let prepared = self.prepare(method, path, body, settings)?;
        let init = RequestInit::new();
        init.set_method(&prepared.method);
        init.set_credentials(match prepared.credentials {
            Credentials::Omit => RequestCredentials::Omit,
            Credentials::SameOrigin => RequestCredentials::SameOrigin,
            Credentials::Include => RequestCredentials::Include,
        });
        let headers = Headers::new().map_err(network_error)?;
        for (name, value) in &prepared.headers {
            headers.set(name, value).map_err(network_error)?;
        }
        init.set_headers(&headers);
        if let Some(body) = &prepared.body {
            init.set_body(&JsValue::from_str(body));
        }
        let window = web_sys::window()
            .ok_or_else(|| ClientError::request("BrowserClient requires a browser Window"))?;
        let raw = JsFuture::from(window.fetch_with_str_and_init(&prepared.url, &init))
            .await
            .map_err(network_error)?;
        let response: Response = raw
            .dyn_into()
            .map_err(|_| ClientError::network("fetch did not return a Response"))?;
        let status = response.status();
        self.notify_status(status);
        if status < 400 && no_response_body(&prepared.method, status) {
            return Ok(None);
        }
        let content_type = response
            .headers()
            .get("Content-Type")
            .map_err(network_error)?
            .unwrap_or_default();
        let text = read_text(&response).await;
        decode_response(&prepared.method, status, &content_type, text)
    }
}

async fn read_text(response: &Response) -> Result<String, String> {
    let promise = response.text().map_err(describe)?;
    JsFuture::from(promise)
        .await
        .map_err(describe)?
        .as_string()
        .ok_or_else(|| "Response.text() did not return a string".into())
}

fn network_error(error: JsValue) -> ClientError {
    ClientError::network(format!("Network request failed: {}", describe(error)))
}

pub(crate) fn describe(error: JsValue) -> String {
    if let Some(error) = error.dyn_ref::<js_sys::Error>() {
        error.message().into()
    } else {
        error.as_string().unwrap_or_else(|| format!("{error:?}"))
    }
}
