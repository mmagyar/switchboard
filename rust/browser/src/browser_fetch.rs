use crate::client::{decode_json_response, decode_response, is_json, no_response_body};
use crate::{BrowserClient, CallSettings, ClientError, Credentials};
use serde::{Serialize, de::DeserializeOwned};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{RequestCredentials, RequestInit, Response};

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
        // Fetch owns validation so initialization errors retain the native fetch message.
        let mut normalized: Vec<(&str, &str)> = Vec::new();
        for (name, value) in &prepared.headers {
            if let Some((_, previous)) = normalized
                .iter_mut()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
            {
                *previous = value;
            } else {
                normalized.push((name, value));
            }
        }
        let headers = js_sys::Array::new();
        for (name, value) in normalized {
            headers.push(&js_sys::Array::of2(
                &JsValue::from_str(name),
                &JsValue::from_str(value),
            ));
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
        if status < 400 && is_json(&content_type) {
            let value = read_json(&response).await.map_err(|error| {
                ClientError::invalid_response(format!("Response body is not valid JSON: {error}"))
            })?;
            return decode_json_response(value);
        }
        let text = read_text(&response).await;
        decode_response(&prepared.method, status, &content_type, text)
    }
}

async fn read_json(response: &Response) -> Result<serde_json::Value, String> {
    // Cloning tees the body stream and can change native syntax-error diagnostics.
    let original = JsFuture::from(response.json().map_err(describe)?)
        .await
        .map_err(describe)?;
    let text = js_sys::JSON::stringify(&original)
        .map_err(describe)?
        .as_string()
        .ok_or_else(|| "Response JSON could not be serialized".to_string())?;
    let mut value = switchboard::json::parse(&text).map_err(|error| error.to_string())?;
    preserve_numbers(&original, &mut value)?;
    Ok(value)
}

fn preserve_numbers(original: &JsValue, value: &mut serde_json::Value) -> Result<(), String> {
    if let Some(number) = original.as_f64() {
        if !number.is_finite() {
            return Err("Response JSON contains a non-finite number".into());
        }
        // JSON.stringify erases negative zero; keep it observable in unchecked results.
        if number == 0.0 && number.is_sign_negative() {
            *value = serde_json::Value::from(number);
        }
    }
    match value {
        serde_json::Value::Array(values) => {
            for (original, value) in js_sys::Array::from(original).iter().zip(values) {
                preserve_numbers(&original, value)?;
            }
        }
        serde_json::Value::Object(fields) => {
            for (key, value) in fields {
                let original =
                    js_sys::Reflect::get(original, &JsValue::from_str(key)).map_err(describe)?;
                preserve_numbers(&original, value)?;
            }
        }
        _ => {}
    }
    Ok(())
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
