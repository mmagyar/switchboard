#![forbid(unsafe_code)]

use axum::body::Body;
use http::HeaderMap;
use http_body_util::BodyExt;
use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::{self, BufRead},
    sync::{Arc, Mutex},
};
use switchboard::handler::{
    Authorization, DeferredValue, Formatted, HandlerDefiner, HandlerOptions, ParsedError, RouteDef,
};
use switchboard::{ErrorKind, Handler, HttpError, Method, Request, Router, Schema, response};

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Case {
    Http(Box<HttpCase>),
    Definition(DefinitionCase),
    Url(Box<UrlCase>),
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HttpCase {
    id: String,
    routes: Vec<FixtureRoute>,
    requests: Vec<FixtureRequest>,
    #[serde(default)]
    user: Value,
    #[serde(default)]
    auth: Option<String>,
    error_parser: Option<Parser>,
    #[serde(default)]
    html_errors: bool,
    default_response: Option<Fallback>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DefinitionCase {
    id: String,
    routes: Vec<FixtureRoute>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UrlCase {
    id: String,
    route: FixtureRoute,
    params: Value,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Parser {
    status: u16,
    message: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fallback {
    status: u16,
    text: String,
    headers: Option<IndexMap<String, String>>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureRoute {
    method: Method,
    path: String,
    permission: Option<String>,
    params: Option<Schema>,
    body: Option<Schema>,
    output: Option<Schema>,
    prefixes: Option<Vec<String>>,
    handler: Option<Action>,
    format: Option<Format>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureRequest {
    method: String,
    url: String,
    headers: Option<IndexMap<String, String>>,
    body: Option<String>,
}
#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Action {
    Echo,
    Value {
        value: Value,
    },
    Deferred {
        value: Deferred,
    },
    Throw {
        error: Throw,
        status: Option<u16>,
        message: Option<String>,
    },
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Throw {
    Request,
    NotFound,
    Unauthorized,
    Generic,
}
#[derive(Clone, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
enum Deferred {
    Ready(Value),
    Future(Box<Deferred>),
    Object(IndexMap<String, Deferred>),
    Array(Vec<Deferred>),
}
#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Format {
    Constant {
        text: Option<String>,
        headers: Option<IndexMap<String, String>>,
        status: Option<u16>,
        redirect: Option<bool>,
    },
    InspectDeferred,
}

fn definition(route: &FixtureRoute) -> Result<RouteDef<String>, String> {
    RouteDef::new(
        route.method,
        route.path.clone(),
        route.permission.clone().unwrap_or_else(|| "public".into()),
        route.params.clone(),
        route.body.clone(),
        route.output.clone().unwrap_or(Schema::Unknown),
        route.prefixes.clone().unwrap_or_default(),
    )
    .map_err(|e| e.to_string())
}

fn materialize(value: Deferred) -> DeferredValue {
    match value {
        Deferred::Ready(value) => DeferredValue::ready(value),
        Deferred::Object(fields) => DeferredValue::object(
            fields
                .into_iter()
                .map(|(key, value)| (key, materialize(value))),
        ),
        Deferred::Array(values) => DeferredValue::array(values.into_iter().map(materialize)),
        Deferred::Future(value) => DeferredValue::future(async move { Ok(materialize(*value)) }),
    }
}

fn action(
    action: Option<Action>,
    params: Value,
    body: Option<Value>,
    user: Value,
    prefix: Option<String>,
) -> Result<DeferredValue, HttpError> {
    match action.unwrap_or(Action::Echo) {
        Action::Echo => Ok(DeferredValue::ready(
            json!({"params":params,"body":body,"user":user,"prefix":prefix}),
        )),
        Action::Value { value } => Ok(DeferredValue::ready(value)),
        Action::Deferred { value } => Ok(materialize(value)),
        Action::Throw {
            error,
            status,
            message,
        } => {
            let message = message.unwrap_or_else(|| "fixture error".into());
            Err(match error {
                Throw::Request => HttpError::new(status.unwrap_or(422), message),
                Throw::NotFound => HttpError::not_found(),
                Throw::Unauthorized => HttpError {
                    kind: ErrorKind::Unauthorized,
                    ..HttpError::new(status.unwrap_or(401), message)
                },
                Throw::Generic => HttpError::unexpected(message),
            })
        }
    }
}

fn pending(value: &DeferredValue, path: &str, paths: &mut Vec<String>) {
    match value {
        DeferredValue::Future(_) => paths.push(path.into()),
        DeferredValue::Object(fields) => {
            for (key, value) in fields {
                pending(
                    value,
                    &if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    },
                    paths,
                );
            }
        }
        DeferredValue::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                pending(
                    value,
                    &if path.is_empty() {
                        index.to_string()
                    } else {
                        format!("{path}.{index}")
                    },
                    paths,
                );
            }
        }
        DeferredValue::Ready(_) => {}
    }
}

fn headers(values: Option<IndexMap<String, String>>) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    for (key, value) in values.unwrap_or_default() {
        let key = http::HeaderName::try_from(key).map_err(|e| e.to_string())?;
        let value = http::HeaderValue::try_from(value).map_err(|e| e.to_string())?;
        headers.insert(key, value);
    }
    Ok(headers)
}

fn configure(
    case: &HttpCase,
    warnings: Arc<Mutex<Vec<Value>>>,
    errors: Arc<Mutex<Vec<Value>>>,
) -> HandlerDefiner<Value, String> {
    let user = case.user.clone();
    let auth = match case.auth.as_deref() {
        Some("forbidden") => Authorization::Forbidden,
        Some("unauthenticated") => Authorization::Unauthenticated,
        _ => Authorization::Ok,
    };
    let mut options = HandlerOptions {
        output_warning: Some(Arc::new(move |warning| {
            warnings.lock().unwrap().push(json!({"message":warning.error.to_string(),"issues":warning.error.issues,"data":warning.data,"method":warning.method.as_str(),"url":warning.url}));
        })),
        error_logger: Arc::new(move |log| {
            let args = json!([log.message,log.url,log.error.message,{"name":"Error","message":log.error.message}]);
            errors
                .lock()
                .unwrap()
                .push(Value::String(switchboard::json::stringify(&args).unwrap()));
        }),
        ..HandlerOptions::default()
    };
    if let Some(parser) = case.error_parser.clone() {
        options.error_parser = Some(Arc::new(move |_| {
            let parser = parser.clone();
            Box::pin(async move {
                Some(ParsedError {
                    status: parser.status,
                    message: parser.message,
                })
            })
        }));
    }
    if case.html_errors {
        options.error_html_formatter = Some(Arc::new(|status, message, _, _| {
            Box::pin(async move { Ok(format!("<error status=\"{status}\">{message}</error>")) })
        }));
    }
    HandlerDefiner::new(
        move |_| {
            let user = user.clone();
            async move { Ok(user) }
        },
        move |_, _, _| async move { Ok(auth) },
    )
    .with_options(options)
}

fn register(
    router: &mut Router,
    definer: &HandlerDefiner<Value, String>,
    route: FixtureRoute,
) -> Result<(), String> {
    let definition = definition(&route)?;
    let handler = route.handler;
    let registered = if let Some(format) = route.format {
        definer.bind_formatted(
            definition,
            move |params, body, user, prefix| {
                let value = action(handler.clone(), params, body, user, prefix);
                async move { value }
            },
            move |data, _, _, _| {
                let format = format.clone();
                async move {
                    match format {
                        Format::InspectDeferred => {
                            let mut paths = Vec::new();
                            pending(&data, "", &mut paths);
                            let body = switchboard::json::stringify(&json!({"pending":paths}))
                                .map_err(|e| HttpError::unexpected(e.to_string()))?;
                            Ok(Formatted {
                                data: Body::from(body),
                                headers: headers(Some(IndexMap::from([(
                                    "content-type".into(),
                                    "application/json".into(),
                                )])))
                                .map_err(HttpError::unexpected)?,
                                ..Formatted::default()
                            })
                        }
                        Format::Constant {
                            text,
                            headers: values,
                            status,
                            redirect,
                        } => Ok(Formatted {
                            data: text.map(Body::from).unwrap_or_else(Body::empty),
                            headers: headers(values).map_err(HttpError::unexpected)?,
                            status,
                            redirect: redirect.unwrap_or(false),
                        }),
                    }
                }
            },
        )
    } else {
        definer.bind(definition, move |params, body, user, prefix| {
            let value = action(handler.clone(), params, body, user, prefix);
            async move { value }
        })
    };
    router.add_route(registered).map_err(|e| e.to_string())
}

async fn run_http(case: HttpCase) -> Result<Value, String> {
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let definer = configure(&case, warnings.clone(), errors.clone());
    let mut router = if let Some(fallback) = case.default_response {
        let headers = headers(fallback.headers)?;
        let handler: Handler = Arc::new(move |_, _| {
            let mut response = response(fallback.status, "", fallback.text.clone());
            *response.headers_mut() = headers.clone();
            Box::pin(async move { response })
        });
        Router::with_default(handler)
    } else {
        Router::new()
    };
    for route in case.routes {
        register(&mut router, &definer, route)?;
    }
    let mut responses = Vec::new();
    for request in case.requests {
        let mut built = Request::new(request.body.map(Body::from).unwrap_or_else(Body::empty));
        *built.method_mut() = request
            .method
            .parse::<http::Method>()
            .map_err(|e| e.to_string())?;
        *built.uri_mut() = request
            .url
            .parse::<http::Uri>()
            .map_err(|e| e.to_string())?;
        *built.headers_mut() = headers(request.headers)?;
        let response = router.handle_request(built).await;
        let status = response.status().as_u16();
        let is_json = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("json"));
        let mut header_values: Vec<_> = response
            .headers()
            .iter()
            .filter(|(key, _)| !matches!(key.as_str(), "date" | "connection" | "keep-alive"))
            .map(|(key, value)| {
                Ok((
                    key.as_str().to_owned(),
                    value.to_str().map_err(|e| e.to_string())?.to_owned(),
                ))
            })
            .collect::<Result<_, String>>()?;
        header_values.sort();
        let bytes = response
            .into_body()
            .collect()
            .await
            .map_err(|e| e.to_string())?
            .to_bytes();
        let text = String::from_utf8(bytes.to_vec()).map_err(|e| e.to_string())?;
        let body = if is_json && !text.is_empty() {
            json!({"type":"json","value":serde_json::from_str::<Value>(&text).map_err(|e| e.to_string())?})
        } else {
            json!({"type":"text","value":text})
        };
        responses.push(json!({"status":status,"headers":header_values,"body":body}));
        definer
            .drain_background()
            .await
            .map_err(|e| e.to_string())?;
    }
    let warnings = warnings.lock().unwrap().clone();
    let errors = errors.lock().unwrap().clone();
    Ok(
        json!({"id":case.id,"kind":"http","responses":responses,"warnings":warnings,"errors":errors}),
    )
}

async fn run_case(case: Case) -> Value {
    let id = match &case {
        Case::Http(c) => &c.id,
        Case::Definition(c) => &c.id,
        Case::Url(c) => &c.id,
    }
    .clone();
    let result = match case {
        Case::Http(case) => run_http(*case).await,
        Case::Definition(case) => {
            let warnings = Arc::new(Mutex::new(Vec::new()));
            let errors = Arc::new(Mutex::new(Vec::new()));
            let defaults = HttpCase {
                id: case.id.clone(),
                routes: vec![],
                requests: vec![],
                user: Value::Null,
                auth: None,
                error_parser: None,
                html_errors: false,
                default_response: None,
            };
            let definer = configure(&defaults, warnings, errors);
            let mut router = Router::new();
            case.routes
                .into_iter()
                .try_for_each(|route| register(&mut router, &definer, route))
                .map(|()| json!({"id":case.id,"kind":"definition"}))
        }
        Case::Url(case) => match definition(&case.route) {
            Err(error) => Err(error),
            Ok(route) => switchboard::params::def_to_url(&route.path, &route.params, case.params)
                .map(|url| json!({"id":case.id,"kind":"url","url":url}))
                .map_err(|e| e.to_string()),
        },
    };
    result.unwrap_or_else(|message| json!({"id":id,"kind":"error","error":{"name":if message.starts_with('['){"ZodError"}else{"Error"},"message":message}}))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    for line in io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let case: Case = serde_json::from_str(&line)?;
        println!("{}", serde_json::to_string(&run_case(case).await)?);
    }
    Ok(())
}
