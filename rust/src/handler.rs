use crate::{
    DefinitionError, ErrorKind, HttpError, Method, RegisteredRoute, Request, RequestContext,
    Response, UploadedFile, params, response,
    schema::{Input, Schema, ValidationError},
};
use axum::body::Body;
use futures_util::{
    FutureExt,
    future::{BoxFuture, Shared, join_all, try_join_all},
};
use http::{HeaderMap, StatusCode};
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    future::Future,
    marker::PhantomData,
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex},
};
use tokio::task::JoinHandle;

#[derive(Clone)]
pub struct RouteDef<Permission> {
    pub method: Method,
    pub path: String,
    pub permission: Permission,
    pub params: Schema,
    pub body: Option<Schema>,
    pub output: Schema,
    pub prefixes: Vec<String>,
}

impl<Permission> RouteDef<Permission> {
    pub fn new(
        method: Method,
        path: impl Into<String>,
        permission: Permission,
        params: Option<Schema>,
        body: Option<Schema>,
        output: Schema,
        prefixes: Vec<String>,
    ) -> Result<Self, DefinitionError> {
        let path = path.into();
        let params = match params {
            Some(schema) => schema,
            None => params::auto_schema(&path)?,
        };
        params::validate_path(&path, &params, &prefixes)?;
        Ok(Self {
            method,
            path,
            permission,
            params,
            body: method.has_body().then(|| body.unwrap_or(Schema::Unknown)),
            output,
            prefixes,
        })
    }
}

pub struct TypedRoute<P, B, O, Permission> {
    pub definition: RouteDef<Permission>,
    types: PhantomData<fn(P, B) -> O>,
}

impl<P: JsonSchema, B: JsonSchema, O: JsonSchema, Permission> TypedRoute<P, B, O, Permission> {
    pub fn new(
        method: Method,
        path: impl Into<String>,
        permission: Permission,
        prefixes: Vec<String>,
    ) -> Result<Self, DefinitionError> {
        Ok(Self {
            definition: RouteDef::new(
                method,
                path,
                permission,
                Some(Schema::from_type::<P>()?),
                if method.has_body() {
                    Some(Schema::from_type::<B>()?)
                } else {
                    None
                },
                Schema::from_type::<O>()?,
                prefixes,
            )?,
            types: PhantomData,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Authorization {
    Ok,
    Forbidden,
    Unauthenticated,
}

/// Cloning shares future results without flattening their object/array structure.
#[derive(Clone)]
pub enum DeferredValue {
    Ready(Value),
    Object(IndexMap<String, DeferredValue>),
    Array(Vec<DeferredValue>),
    Future(Shared<BoxFuture<'static, Result<DeferredValue, HttpError>>>),
}

impl DeferredValue {
    pub fn ready(value: impl Into<Value>) -> Self {
        Self::Ready(value.into())
    }

    pub fn object(fields: impl IntoIterator<Item = (String, DeferredValue)>) -> Self {
        Self::Object(fields.into_iter().collect())
    }

    pub fn array(values: impl IntoIterator<Item = DeferredValue>) -> Self {
        Self::Array(values.into_iter().collect())
    }

    pub fn future<F>(future: F) -> Self
    where
        F: Future<Output = Result<DeferredValue, HttpError>> + Send + 'static,
    {
        Self::Future(future.boxed().shared())
    }

    /// Resolves a root future, retaining deferred properties for a raw formatter.
    pub async fn unfold(mut self) -> Result<Self, HttpError> {
        while let Self::Future(future) = self {
            self = future.await?;
        }
        Ok(self)
    }

    pub fn resolve(self) -> BoxFuture<'static, Result<Value, HttpError>> {
        async move {
            match self.unfold().await? {
                Self::Ready(value) => Ok(value),
                Self::Object(fields) => {
                    let entries = try_join_all(fields.into_iter().map(|(key, value)| async move {
                        Ok::<_, HttpError>((key, value.resolve().await?))
                    }))
                    .await?;
                    Ok(Value::Object(entries.into_iter().collect()))
                }
                Self::Array(values) => Ok(Value::Array(
                    try_join_all(values.into_iter().map(Self::resolve)).await?,
                )),
                Self::Future(_) => unreachable!("unfold removes root futures"),
            }
        }
        .boxed()
    }

    fn validation_input(&self) -> Input {
        match self {
            Self::Ready(value) => Input::from_value(value.clone()),
            Self::Object(fields) => Input::Object(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), value.validation_input()))
                    .collect(),
            ),
            Self::Array(values) => {
                Input::Array(values.iter().map(Self::validation_input).collect())
            }
            Self::Future(_) => Input::Promise,
        }
    }

    fn fields(&self) -> Option<IndexMap<String, Self>> {
        match self {
            Self::Object(fields) => Some(fields.clone()),
            Self::Ready(Value::Object(fields)) => Some(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::Ready(value.clone())))
                    .collect(),
            ),
            _ => None,
        }
    }
}

impl From<Value> for DeferredValue {
    fn from(value: Value) -> Self {
        Self::Ready(value)
    }
}

pub struct Formatted {
    pub data: Body,
    pub body_kind: crate::BodyKind,
    pub headers: HeaderMap,
    pub status: Option<u16>,
    pub redirect: bool,
}

impl Formatted {
    pub fn with_body<T: Into<Body> + 'static>(body: T) -> Self {
        Self {
            data: body.into(),
            body_kind: crate::BodyKind::for_type::<T>(),
            ..Self::default()
        }
    }
}

impl Default for Formatted {
    fn default() -> Self {
        Self {
            data: Body::empty(),
            body_kind: crate::BodyKind::Text,
            headers: HeaderMap::new(),
            status: None,
            redirect: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct OutputWarning {
    pub error: ValidationError,
    pub data: Value,
    pub method: Method,
    pub url: String,
}

#[derive(Clone, Debug)]
pub struct ParsedError {
    pub status: u16,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct ErrorLog {
    pub message: String,
    pub url: String,
    pub error: HttpError,
}

pub type OutputWarningCallback = Arc<dyn Fn(OutputWarning) + Send + Sync>;
pub type ErrorParser =
    Arc<dyn Fn(HttpError) -> BoxFuture<'static, Option<ParsedError>> + Send + Sync>;
pub type ErrorHtmlFormatter<User> = Arc<
    dyn Fn(
            u16,
            String,
            RequestContext,
            Option<User>,
        ) -> BoxFuture<'static, Result<String, HttpError>>
        + Send
        + Sync,
>;
pub type ErrorLogger = Arc<dyn Fn(ErrorLog) + Send + Sync>;

pub struct HandlerOptions<User> {
    pub output_warning: Option<OutputWarningCallback>,
    pub error_parser: Option<ErrorParser>,
    pub error_html_formatter: Option<ErrorHtmlFormatter<User>>,
    pub error_logger: ErrorLogger,
}

impl<User> Default for HandlerOptions<User> {
    fn default() -> Self {
        Self {
            output_warning: None,
            error_parser: None,
            error_html_formatter: None,
            error_logger: Arc::new(|event| {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "level": "error", "message": event.message,
                        "url": event.url, "error": event.error,
                    })
                );
            }),
        }
    }
}

type GetUser<User> =
    Arc<dyn Fn(RequestContext) -> BoxFuture<'static, Result<User, HttpError>> + Send + Sync>;
type Authorizer<User, Permission> = Arc<
    dyn Fn(User, Permission, RequestContext) -> BoxFuture<'static, Result<Authorization, HttpError>>
        + Send
        + Sync,
>;
type Callback<User> = Arc<
    dyn Fn(
            Value,
            Option<Value>,
            User,
            Option<String>,
            RequestContext,
        ) -> BoxFuture<'static, Result<DeferredValue, HttpError>>
        + Send
        + Sync,
>;
type Formatter<User> = Arc<
    dyn Fn(
            DeferredValue,
            User,
            RequestContext,
            Value,
        ) -> BoxFuture<'static, Result<Formatted, HttpError>>
        + Send
        + Sync,
>;
struct HandlerBinding<User, Permission> {
    route: RouteDef<Permission>,
    handler: Callback<User>,
    formatter: Option<Formatter<User>>,
}

type BackgroundTask = JoinHandle<Result<(), HttpError>>;

#[derive(Default)]
struct Background {
    tasks: Mutex<Vec<BackgroundTask>>,
    failure: Mutex<Option<HttpError>>,
}

impl Background {
    fn record(&self, result: Result<Result<(), HttpError>, tokio::task::JoinError>) {
        let error = match result {
            Ok(Ok(())) => return,
            Ok(Err(error)) => error,
            Err(error) => {
                HttpError::unexpected(format!("Background validation task failed: {error}"))
            }
        };
        self.failure
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_or_insert(error);
    }

    fn reap(&self) {
        let mut tasks = self.tasks.lock().unwrap_or_else(|error| error.into_inner());
        let mut pending = Vec::new();
        for task in tasks.drain(..) {
            if task.is_finished() {
                if let Some(result) = task.now_or_never() {
                    self.record(result);
                }
            } else {
                pending.push(task);
            }
        }
        *tasks = pending;
    }

    fn spawn(&self, task: impl Future<Output = Result<(), HttpError>> + Send + 'static) {
        self.reap();
        self.tasks
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(tokio::spawn(task));
    }

    fn take(&self) -> Vec<BackgroundTask> {
        std::mem::take(&mut *self.tasks.lock().unwrap_or_else(|error| error.into_inner()))
    }

    fn result(&self) -> Result<(), HttpError> {
        match self
            .failure
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        for task in self
            .tasks
            .get_mut()
            .unwrap_or_else(|error| error.into_inner())
        {
            task.abort();
        }
    }
}

pub struct HandlerDefiner<User, Permission> {
    get_user: GetUser<User>,
    authorizer: Authorizer<User, Permission>,
    options: Arc<HandlerOptions<User>>,
    background: Arc<Background>,
}

impl<User, Permission> Clone for HandlerDefiner<User, Permission> {
    fn clone(&self) -> Self {
        Self {
            get_user: self.get_user.clone(),
            authorizer: self.authorizer.clone(),
            options: self.options.clone(),
            background: self.background.clone(),
        }
    }
}

impl<User, Permission> HandlerDefiner<User, Permission>
where
    User: Clone + Send + Sync + 'static,
    Permission: Clone + Send + Sync + 'static,
{
    pub fn new<G, GF, A, AF>(get_user: G, authorizer: A) -> Self
    where
        G: Fn(RequestContext) -> GF + Send + Sync + 'static,
        GF: Future<Output = Result<User, HttpError>> + Send + 'static,
        A: Fn(User, Permission, RequestContext) -> AF + Send + Sync + 'static,
        AF: Future<Output = Result<Authorization, HttpError>> + Send + 'static,
    {
        Self {
            get_user: Arc::new(move |request| get_user(request).boxed()),
            authorizer: Arc::new(move |user, permission, request| {
                authorizer(user, permission, request).boxed()
            }),
            options: Arc::new(HandlerOptions::default()),
            background: Arc::new(Background::default()),
        }
    }

    pub fn with_options(mut self, options: HandlerOptions<User>) -> Self {
        self.options = Arc::new(options);
        self
    }

    /// Stop accepting requests before draining. A pending field can keep this future pending.
    pub async fn drain_background(&self) -> Result<(), HttpError> {
        for result in join_all(self.background.take()).await {
            self.background.record(result);
        }
        self.background.result()
    }

    /// After server shutdown, cancel remaining fields and join every owned task.
    pub async fn cleanup_background(&self) -> Result<(), HttpError> {
        let tasks = self.background.take();
        for task in &tasks {
            task.abort();
        }
        for result in join_all(tasks).await {
            if !matches!(&result, Err(error) if error.is_cancelled()) {
                self.background.record(result);
            }
        }
        self.background.result()
    }

    pub fn bind<H, HF>(&self, route: RouteDef<Permission>, handler: H) -> RegisteredRoute
    where
        H: Fn(Value, Option<Value>, User, Option<String>) -> HF + Send + Sync + 'static,
        HF: Future<Output = Result<DeferredValue, HttpError>> + Send + 'static,
    {
        self.bind_with_context(route, move |params, body, user, prefix, _| {
            handler(params, body, user, prefix)
        })
    }

    pub fn bind_with_context<H, HF>(
        &self,
        route: RouteDef<Permission>,
        handler: H,
    ) -> RegisteredRoute
    where
        H: Fn(Value, Option<Value>, User, Option<String>, RequestContext) -> HF
            + Send
            + Sync
            + 'static,
        HF: Future<Output = Result<DeferredValue, HttpError>> + Send + 'static,
    {
        self.register(
            route,
            Arc::new(move |params, body, user, prefix, context| {
                handler(params, body, user, prefix, context).boxed()
            }),
            None,
        )
    }

    pub fn bind_formatted<H, HF, F, FF>(
        &self,
        route: RouteDef<Permission>,
        handler: H,
        formatter: F,
    ) -> RegisteredRoute
    where
        H: Fn(Value, Option<Value>, User, Option<String>) -> HF + Send + Sync + 'static,
        HF: Future<Output = Result<DeferredValue, HttpError>> + Send + 'static,
        F: Fn(DeferredValue, User, RequestContext, Value) -> FF + Send + Sync + 'static,
        FF: Future<Output = Result<Formatted, HttpError>> + Send + 'static,
    {
        self.register(
            route,
            Arc::new(move |params, body, user, prefix, _| {
                handler(params, body, user, prefix).boxed()
            }),
            Some(Arc::new(move |data, user, request, params| {
                formatter(data, user, request, params).boxed()
            })),
        )
    }

    pub fn bind_typed<P, B, O, H, HF>(
        &self,
        route: TypedRoute<P, B, O, Permission>,
        handler: H,
    ) -> RegisteredRoute
    where
        P: DeserializeOwned + Send + 'static,
        B: DeserializeOwned + Send + 'static,
        O: Serialize + Send + 'static,
        H: Fn(P, Option<B>, User, Option<String>) -> HF + Send + Sync + 'static,
        HF: Future<Output = Result<O, HttpError>> + Send + 'static,
    {
        let handler = Arc::new(handler);
        self.bind(route.definition, move |params, body, user, prefix| {
            let handler = handler.clone();
            async move {
                let params = serde_json::from_value(params).map_err(|error| {
                    HttpError::unexpected(format!("Validated params could not be decoded: {error}"))
                })?;
                let body = body
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(|error| {
                        HttpError::unexpected(format!(
                            "Validated body could not be decoded: {error}"
                        ))
                    })?;
                let value = handler(params, body, user, prefix).await?;
                serde_json::to_value(value)
                    .map(DeferredValue::Ready)
                    .map_err(|error| {
                        HttpError::unexpected(format!("Output could not be encoded: {error}"))
                    })
            }
        })
    }

    fn register(
        &self,
        route: RouteDef<Permission>,
        handler: Callback<User>,
        formatter: Option<Formatter<User>>,
    ) -> RegisteredRoute {
        RegisteredRoute {
            method: route.method,
            path: route.path.clone(),
            prefixes: route.prefixes.clone(),
            handler: {
                let definer = self.clone();
                let binding = Arc::new(HandlerBinding {
                    route,
                    handler,
                    formatter,
                });
                Arc::new(move |request, prefix| {
                    let definer = definer.clone();
                    let binding = binding.clone();
                    async move {
                        definer.background.reap();
                        let context = match RequestContext::from_request(&request) {
                            Ok(context) => context,
                            Err(error) => return response(error.status, "", error.message),
                        };
                        let mut user = None;
                        let result = AssertUnwindSafe(
                            definer.run(request, &context, &mut user, &binding, prefix),
                        )
                        .catch_unwind()
                        .await
                        .unwrap_or_else(|panic| Err(panic_error(panic)));
                        match result {
                            Ok(response) => response,
                            Err(error) => definer.error_response(error, &context, user).await,
                        }
                    }
                    .boxed()
                })
            },
        }
    }

    async fn run(
        &self,
        request: Request,
        context: &RequestContext,
        user: &mut Option<User>,
        binding: &HandlerBinding<User, Permission>,
        prefix: Option<String>,
    ) -> Result<Response, HttpError> {
        let route = &binding.route;
        let authenticated = (self.get_user)(context.clone()).await?;
        *user = Some(authenticated.clone());
        match (self.authorizer)(
            authenticated.clone(),
            route.permission.clone(),
            context.clone(),
        )
        .await?
        {
            Authorization::Ok => {}
            Authorization::Forbidden => return Err(HttpError::forbidden()),
            Authorization::Unauthenticated => return Err(HttpError::unauthorized()),
        }
        let params = match params::parse_url(&context.url, &route.path, &route.params) {
            Ok(params) => params,
            Err(error) => {
                return self
                    .message_response(
                        400,
                        format!("Path or query params did not match defined schema: {error}"),
                        context,
                        user.clone(),
                    )
                    .await;
            }
        };
        let mut handler_context = context.clone();
        let body = if route.method.has_body() {
            let schema = route.body.as_ref().unwrap_or(&Schema::Unknown);
            match parse_body(request, &mut handler_context, schema).await? {
                ParsedBody::Valid(body) => body,
                ParsedBody::RawError(status, message) => return Ok(response(status, "", message)),
                ParsedBody::Invalid(error) => {
                    return self
                        .message_response(
                            400,
                            format!("Body does not match defined schema: {error}"),
                            context,
                            user.clone(),
                        )
                        .await;
                }
            }
        } else {
            None
        };
        let value = (binding.handler)(
            params.clone(),
            body,
            authenticated.clone(),
            prefix,
            handler_context.clone(),
        )
        .await?
        .unfold()
        .await?;
        if let Some(formatter) = &binding.formatter {
            self.validate_formatted(&value, &route.output, route.method, context);
            let formatted = formatter(value, authenticated, handler_context, params).await?;
            let status = formatted.status.unwrap_or(if formatted.redirect {
                303
            } else {
                route.method.success_status()
            });
            let status = StatusCode::from_u16(status)
                .map_err(|error| HttpError::unexpected(error.to_string()))?;
            let mut response = http::Response::new(formatted.data);
            *response.status_mut() = status;
            *response.headers_mut() = formatted.headers;
            response.extensions_mut().insert(formatted.body_kind);
            return Ok(response);
        }
        let original = value.resolve().await?;
        let output = match route.output.parse_value(original.clone()) {
            Ok(value) => value,
            Err(error) => {
                warn(
                    &self.options,
                    error,
                    original.clone(),
                    route.method,
                    context.url.to_string(),
                );
                original
            }
        };
        Ok(response(
            route.method.success_status(),
            "application/json",
            crate::json::stringify(&output)
                .map_err(|error| HttpError::unexpected(error.to_string()))?,
        ))
    }

    fn validate_formatted(
        &self,
        value: &DeferredValue,
        schema: &Schema,
        method: Method,
        context: &RequestContext,
    ) {
        let mut checks: Vec<BoxFuture<'static, Result<(), HttpError>>> = Vec::new();
        if let (Schema::Object(shape), Some(fields)) = (schema, value.fields()) {
            for (key, field) in fields {
                let Some(schema) = shape.get(&key).cloned() else {
                    continue;
                };
                let url = format!("{}#{key}", context.url);
                if matches!(field, DeferredValue::Future(_)) {
                    let options = self.options.clone();
                    checks.push(
                        async move {
                            // The driver reports rejected futures once, including unknown fields.
                            if let Ok(field) = field.unfold().await {
                                warn_field(&options, &schema, &field, method, url);
                            }
                            Ok(())
                        }
                        .boxed(),
                    );
                } else {
                    warn_field(&self.options, &schema, &field, method, url);
                }
            }
        }
        // Rust futures are lazy. Drive even formatter-ignored and unknown fields under an owner.
        let options = self.options.clone();
        let url = context.url.to_string();
        checks.push(drive_background(
            value.clone(),
            options.error_logger.clone(),
            url.clone(),
        ));
        self.background.spawn(async move {
            match AssertUnwindSafe(async move {
                let results = join_all(checks).await;
                results
                    .into_iter()
                    .collect::<Result<Vec<_>, _>>()
                    .map(|_| ())
            })
            .catch_unwind()
            .await
            {
                Ok(result) => result,
                Err(panic) => {
                    let error = panic_error(panic);
                    (options.error_logger)(ErrorLog {
                        message: "Internal Server Error".into(),
                        url,
                        error: error.clone(),
                    });
                    Err(error)
                }
            }
        });
    }

    async fn error_response(
        &self,
        error: HttpError,
        context: &RequestContext,
        user: Option<User>,
    ) -> Response {
        if let Some(parser) = &self.options.error_parser {
            if let Some(parsed) = parser(error.clone()).await {
                return self
                    .safe_message_response(parsed.status, parsed.message, context, user)
                    .await;
            }
        }
        let (status, message) = match error.kind {
            ErrorKind::NotFound => (404, "Not Found - Missing entry".into()),
            ErrorKind::Request | ErrorKind::Unauthorized => (error.status, error.message.clone()),
            ErrorKind::Internal => {
                (self.options.error_logger)(ErrorLog {
                    message: "Internal Server Error".into(),
                    url: context.url.to_string(),
                    error,
                });
                (500, "Internal Server Error".into())
            }
        };
        self.safe_message_response(status, message, context, user)
            .await
    }

    async fn safe_message_response(
        &self,
        status: u16,
        message: String,
        context: &RequestContext,
        user: Option<User>,
    ) -> Response {
        match self.message_response(status, message, context, user).await {
            Ok(response) => response,
            Err(error) => {
                (self.options.error_logger)(ErrorLog {
                    message: "Internal Server Error".into(),
                    url: context.url.to_string(),
                    error,
                });
                response(500, "text/plain", "Internal Server Error")
            }
        }
    }

    async fn message_response(
        &self,
        status: u16,
        message: String,
        context: &RequestContext,
        user: Option<User>,
    ) -> Result<Response, HttpError> {
        let accept = context
            .headers
            .get(http::header::ACCEPT)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        let json = accept.contains("json");
        let html = !json && (accept.contains("html") || !accept.contains("plain"));
        let content_type = if json {
            "application/json"
        } else if html {
            "text/html"
        } else {
            "text/plain"
        };
        let body = if json {
            crate::json::stringify(&Value::String(message))
                .map_err(|error| HttpError::unexpected(error.to_string()))?
        } else if html {
            if let Some(formatter) = &self.options.error_html_formatter {
                formatter(status, message, context.clone(), user).await?
            } else {
                message
            }
        } else {
            message
        };
        Ok(response(status, content_type, body))
    }
}

fn drive_background(
    value: DeferredValue,
    logger: ErrorLogger,
    url: String,
) -> BoxFuture<'static, Result<(), HttpError>> {
    async move {
        let value = match value.unfold().await {
            Ok(value) => value,
            Err(error) => {
                logger(ErrorLog {
                    message: "Internal Server Error".into(),
                    url,
                    error: error.clone(),
                });
                return Err(error);
            }
        };
        let children = match value {
            DeferredValue::Object(fields) => fields.into_values().collect(),
            DeferredValue::Array(values) => values,
            DeferredValue::Ready(_) => return Ok(()),
            DeferredValue::Future(_) => unreachable!("unfold removes root futures"),
        };
        // Every sibling must run even if another rejects. JSON's fail-fast resolver cancels them.
        let results = join_all(
            children
                .into_iter()
                .map(|child| drive_background(child, logger.clone(), url.clone())),
        )
        .await;
        results
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .map(|_| ())
    }
    .boxed()
}

fn warn_field<User>(
    options: &HandlerOptions<User>,
    schema: &Schema,
    field: &DeferredValue,
    method: Method,
    url: String,
) {
    let input = field.validation_input();
    if let Err(error) = schema.parse_input(&input) {
        warn(
            options,
            error,
            input.to_value().unwrap_or(Value::Null),
            method,
            url,
        );
    }
}

fn warn<User>(
    options: &HandlerOptions<User>,
    error: ValidationError,
    data: Value,
    method: Method,
    url: String,
) {
    if let Some(callback) = &options.output_warning {
        callback(OutputWarning {
            error,
            data,
            method,
            url,
        });
    }
}

fn panic_error(panic: Box<dyn std::any::Any + Send>) -> HttpError {
    let message = if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_owned()
    } else {
        "Handler panicked".into()
    };
    HttpError::unexpected(message)
}

enum ParsedBody {
    Valid(Option<Value>),
    Invalid(ValidationError),
    RawError(u16, String),
}

async fn parse_body(
    request: Request,
    context: &mut RequestContext,
    schema: &Schema,
) -> Result<ParsedBody, HttpError> {
    let content_type = context
        .headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    if let Some(content_type) = content_type {
        if !content_type.contains("form") && !content_type.contains("json") {
            return Ok(ParsedBody::RawError(
                415,
                format!("Unsupported content type: {content_type}"),
            ));
        }
    }
    if content_type.is_some_and(|value| value.contains("form")) {
        let content_type = content_type.unwrap_or_default();
        let mut fields = IndexMap::new();
        if content_type.starts_with("multipart/form-data") {
            let boundary = multer::parse_boundary(content_type)
                .map_err(|error| HttpError::unexpected(error.to_string()))?;
            let mut multipart =
                multer::Multipart::new(request.into_body().into_data_stream(), boundary);
            while let Some(field) = multipart
                .next_field()
                .await
                .map_err(|error| HttpError::unexpected(error.to_string()))?
            {
                let name = field.name().unwrap_or_default().to_owned();
                let value = if let Some(filename) = field.file_name().map(str::to_owned) {
                    let content_type = field.content_type().map(ToString::to_string);
                    let data = field
                        .bytes()
                        .await
                        .map_err(|error| HttpError::unexpected(error.to_string()))?;
                    context.uploads.push(UploadedFile {
                        field_name: name.clone(),
                        filename,
                        content_type,
                        data,
                    });
                    Input::Blob
                } else {
                    Input::Scalar(Value::String(
                        field
                            .text()
                            .await
                            .map_err(|error| HttpError::unexpected(error.to_string()))?,
                    ))
                };
                append_form(&mut fields, name, value);
            }
        } else if content_type.starts_with("application/x-www-form-urlencoded") {
            let bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                .await
                .map_err(|error| HttpError::unexpected(error.to_string()))?;
            for (key, value) in url::form_urlencoded::parse(&bytes) {
                append_form(
                    &mut fields,
                    key.into_owned(),
                    Input::Scalar(Value::String(value.into_owned())),
                );
            }
        } else {
            return Err(HttpError::unexpected(
                "Can't decode form data from body because of incorrect MIME type/boundary",
            ));
        }
        return Ok(
            match params::parse_form_input(schema, Input::Object(fields)) {
                Ok(value) => ParsedBody::Valid(Some(value)),
                Err(error) => ParsedBody::Invalid(error),
            },
        );
    }
    let bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|error| HttpError::unexpected(error.to_string()))?;
    let text = String::from_utf8_lossy(&bytes);
    let value = if text.trim().is_empty() {
        None
    } else {
        match serde_json::from_str(&text) {
            Ok(value) => Some(value),
            Err(_) => return Ok(ParsedBody::RawError(400, "Body is not valid JSON".into())),
        }
    };
    Ok(match schema.parse(value) {
        Ok(value) => ParsedBody::Valid(value),
        Err(error) => ParsedBody::Invalid(error),
    })
}

fn append_form(fields: &mut IndexMap<String, Input>, key: String, value: Input) {
    match fields.entry(key) {
        indexmap::map::Entry::Vacant(entry) => {
            entry.insert(value);
        }
        indexmap::map::Entry::Occupied(mut entry) => {
            let existing = entry.get_mut();
            if let Input::Array(values) = existing {
                values.push(value);
            } else {
                let previous = std::mem::replace(existing, Input::Absent);
                *existing = Input::Array(vec![previous, value]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Router;
    use bytes::Bytes;
    use futures_util::stream;
    use http_body_util::BodyExt;
    use serde::{Deserialize, Serialize};
    use serde_json::json;
    use std::{
        convert::Infallible,
        sync::atomic::{AtomicBool, Ordering},
        time::Duration,
    };
    use tokio::sync::oneshot;

    fn definer() -> HandlerDefiner<String, String> {
        HandlerDefiner::new(
            |_| async { Ok("reader".to_owned()) },
            |_, _, _| async { Ok(Authorization::Ok) },
        )
    }

    fn object(fields: impl IntoIterator<Item = (&'static str, Schema)>) -> Schema {
        Schema::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        )
    }

    fn route(method: Method, body: Option<Schema>, output: Schema) -> RouteDef<String> {
        RouteDef::new(
            method,
            "/items/:itemId",
            "user".into(),
            None,
            body,
            output,
            vec![],
        )
        .unwrap()
    }

    fn request(method: Method, path: &str, content_type: Option<&str>, body: &str) -> Request {
        let mut builder = http::Request::builder()
            .method(method.as_str().to_uppercase().as_str())
            .uri(format!("http://localhost{path}"));
        if let Some(content_type) = content_type {
            builder = builder.header("content-type", content_type);
        }
        builder.body(Body::from(body.to_owned())).unwrap()
    }

    fn accepting(mut request: Request, accept: &str) -> Request {
        request
            .headers_mut()
            .insert(http::header::ACCEPT, accept.parse().unwrap());
        request
    }

    async fn text(response: Response) -> String {
        String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    }

    async fn json_body(response: Response) -> Value {
        serde_json::from_str(&text(response).await).unwrap()
    }

    fn router(route: RegisteredRoute) -> Router {
        let mut router = Router::new();
        router.add_route(route).unwrap();
        router
    }

    async fn echo(
        params: Value,
        body: Option<Value>,
        user: String,
        prefix: Option<String>,
    ) -> Result<DeferredValue, HttpError> {
        Ok(DeferredValue::ready(
            json!({"params":params,"body":body,"user":user,"prefix":prefix}),
        ))
    }

    #[test]
    fn route_definition_validates_paths_prefixes_and_body_methods() {
        for (path, params, prefixes, expected) in [
            (
                "/x/:id",
                Some(object([])),
                vec![],
                "missing from paramsValidation",
            ),
            ("/x/:id?/:other", None, vec![], "non optional parameter"),
            (
                "/x",
                None,
                vec!["/en".into()],
                "must not start with a slash",
            ),
            (
                "/x",
                None,
                vec![":locale".into()],
                "must not contain path params",
            ),
        ] {
            let error = RouteDef::new(
                Method::Get,
                path,
                (),
                params,
                None,
                Schema::Unknown,
                prefixes,
            )
            .err()
            .unwrap();
            assert!(error.0.contains(expected), "{error}");
        }
        let ambiguous = object([("q", Schema::Union(vec![Schema::String, Schema::Number]))]);
        let error = RouteDef::new(
            Method::Get,
            "/x",
            (),
            Some(ambiguous),
            None,
            Schema::Unknown,
            vec![],
        )
        .err()
        .unwrap();
        assert!(error.0.contains("ambiguous union"));
        assert!(
            route(Method::Get, Some(Schema::String), Schema::Unknown)
                .body
                .is_none()
        );
        assert!(matches!(
            route(Method::Post, None, Schema::Unknown).body,
            Some(Schema::Unknown)
        ));
    }

    #[tokio::test]
    async fn auth_runs_before_params_body_or_handler_and_receives_context() {
        for (authorization, status, message) in [
            (Authorization::Forbidden, 403, "Forbidden"),
            (Authorization::Unauthenticated, 401, "Unauthorized"),
        ] {
            let events = Arc::new(Mutex::new(Vec::new()));
            let authentication_events = events.clone();
            let authorization_events = events.clone();
            let definer = HandlerDefiner::new(
                move |context: RequestContext| {
                    assert_eq!(context.headers["x-auth"], "token");
                    authentication_events.lock().unwrap().push("get-user");
                    async { Ok("reader".to_owned()) }
                },
                move |user, permission: String, context: RequestContext| {
                    assert_eq!(user, "reader");
                    assert_eq!(permission, "user");
                    assert_eq!(context.url.path(), "/items/not-a-number");
                    authorization_events.lock().unwrap().push("authorize");
                    async move { Ok(authorization) }
                },
            );
            let route = definer.bind(
                route(Method::Post, Some(Schema::Number), Schema::Unknown),
                |_, _, _, _| async { panic!("auth must reject before handler") },
            );
            let mut req = request(
                Method::Post,
                "/items/not-a-number",
                Some("application/json"),
                "invalid{",
            );
            req.headers_mut().insert("x-auth", "token".parse().unwrap());
            let response = router(route)
                .handle_request(accepting(req, "application/json"))
                .await;
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()["content-type"], "application/json");
            assert_eq!(json_body(response).await, json!(message));
            assert_eq!(*events.lock().unwrap(), ["get-user", "authorize"]);
        }
    }

    #[tokio::test]
    async fn validated_params_body_and_outputs_strip_unknown_fields() {
        let definer = definer();
        let body = object([("name", Schema::String), ("age", Schema::Number)]);
        let mut definition = route(Method::Post, Some(body.clone()), body);
        definition.params = object([("itemId", Schema::Number), ("published", Schema::Boolean)]);
        let registered = definer.bind(definition, |params, body, user, prefix| async move {
            assert_eq!(params, json!({"itemId":7,"published":true}));
            assert_eq!(user, "reader");
            assert_eq!(prefix, None);
            assert_eq!(body, Some(json!({"name":"Joe","age":30})));
            Ok(DeferredValue::ready(
                json!({"name":"Joe","age":30,"extra":"strip me"}),
            ))
        });
        let response = router(registered)
            .handle_request(request(
                Method::Post,
                "/items/7?itemId=99&published=true&extra=ignored",
                Some("application/json"),
                r#"{"name":"Joe","age":30,"extra":true}"#,
            ))
            .await;
        assert_eq!(response.status(), 201);
        assert_eq!(response.headers()["content-type"], "application/json");
        assert_eq!(json_body(response).await, json!({"name":"Joe","age":30}));
    }

    #[tokio::test]
    async fn methods_have_reference_default_status_and_delete_retains_core_json() {
        for method in [
            Method::Get,
            Method::Post,
            Method::Put,
            Method::Patch,
            Method::Delete,
            Method::Head,
            Method::Options,
        ] {
            let registered = definer()
                .bind(route(method, None, Schema::String), |_, _, _, _| async {
                    Ok(DeferredValue::ready("ok"))
                });
            let response = router(registered)
                .handle_request(request(method, "/items/1", None, ""))
                .await;
            assert_eq!(response.status().as_u16(), method.success_status());
            assert_eq!(json_body(response).await, json!("ok"));
        }
    }

    #[tokio::test]
    async fn invalid_json_and_unsupported_media_have_no_implicit_headers() {
        let registered = definer().bind(route(Method::Post, None, Schema::Unknown), echo);
        let router = router(registered);
        for (content_type, body, status, expected) in [
            (
                Some("application/json"),
                "broken{",
                400,
                "Body is not valid JSON",
            ),
            (None, "{", 400, "Body is not valid JSON"),
            (
                Some("text/plain"),
                "anything",
                415,
                "Unsupported content type: text/plain",
            ),
            (
                Some("application/octet-stream"),
                "",
                415,
                "Unsupported content type: application/octet-stream",
            ),
        ] {
            let response = router
                .handle_request(accepting(
                    request(Method::Post, "/items/1", content_type, body),
                    "application/json",
                ))
                .await;
            assert_eq!(response.status(), status);
            assert!(response.headers().is_empty());
            assert_eq!(text(response).await, expected);
        }
    }

    #[tokio::test]
    async fn absent_body_is_distinct_from_explicit_null_and_defaults_apply() {
        let schema = Schema::Optional(Box::new(Schema::String));
        let registered = definer().bind(route(Method::Post, Some(schema), Schema::Unknown), echo);
        let router = router(registered);
        let response = router
            .handle_request(request(Method::Post, "/items/1", None, " \n\t"))
            .await;
        assert_eq!(response.status(), 201);
        assert_eq!(json_body(response).await["body"], Value::Null);
        let response = router
            .handle_request(accepting(
                request(Method::Post, "/items/1", None, "null"),
                "text/plain",
            ))
            .await;
        assert_eq!(response.status(), 400);
        assert_eq!(
            text(response).await,
            "Body does not match defined schema: [\n  {\n    \"expected\": \"string\",\n    \"code\": \"invalid_type\",\n    \"path\": [],\n    \"message\": \"Invalid input: expected string, received null\"\n  }\n]"
        );
        let schema = Schema::Default(Box::new(Schema::String), json!("fallback"));
        let registered = definer().bind(route(Method::Patch, Some(schema), Schema::Unknown), echo);
        let response = super::tests::router(registered)
            .handle_request(request(Method::Patch, "/items/1", None, ""))
            .await;
        assert_eq!(response.status(), 200);
        assert_eq!(json_body(response).await["body"], json!("fallback"));
    }

    #[tokio::test]
    async fn body_and_param_validation_return_exact_negotiated_json_strings() {
        let registered = definer().bind(
            route(Method::Post, Some(Schema::Number), Schema::Unknown),
            echo,
        );
        let router = router(registered);
        let response = router
            .handle_request(accepting(
                request(Method::Post, "/items/1", None, "\"wrong\""),
                "application/json",
            ))
            .await;
        assert_eq!(response.status(), 400);
        assert_eq!(
            json_body(response).await,
            json!(
                "Body does not match defined schema: [\n  {\n    \"expected\": \"number\",\n    \"code\": \"invalid_type\",\n    \"path\": [],\n    \"message\": \"Invalid input: expected number, received string\"\n  }\n]"
            )
        );
        let response = router
            .handle_request(accepting(
                request(Method::Post, "/items/nope", None, "1"),
                "text/plain",
            ))
            .await;
        assert_eq!(response.status(), 400);
        assert_eq!(
            text(response).await,
            "Path or query params did not match defined schema: [\n  {\n    \"expected\": \"number\",\n    \"code\": \"invalid_type\",\n    \"path\": [\n      \"itemId\"\n    ],\n    \"message\": \"Invalid input: expected number, received string\"\n  }\n]"
        );
    }

    #[tokio::test]
    async fn urlencoded_and_multipart_forms_coerce_repeated_fields_and_keep_dots_flat() {
        let schema = object([
            ("age", Schema::Number),
            ("enabled", Schema::Boolean),
            ("tags", Schema::Array(Box::new(Schema::Number))),
            ("nested.name", Schema::String),
        ]);
        let registered = definer().bind(route(Method::Post, Some(schema), Schema::Unknown), echo);
        let router = router(registered);
        let encoded = "age=30&enabled=true&tags=1&tags=2&nested.name=Joe&extra=strip";
        let multipart = concat!(
            "--test-boundary\r\nContent-Disposition: form-data; name=\"age\"\r\n\r\n30\r\n",
            "--test-boundary\r\nContent-Disposition: form-data; name=\"enabled\"\r\n\r\ntrue\r\n",
            "--test-boundary\r\nContent-Disposition: form-data; name=\"tags\"\r\n\r\n1\r\n",
            "--test-boundary\r\nContent-Disposition: form-data; name=\"tags\"\r\n\r\n2\r\n",
            "--test-boundary\r\nContent-Disposition: form-data; name=\"nested.name\"\r\n\r\nJoe\r\n",
            "--test-boundary--\r\n",
        );
        for (content_type, data) in [
            ("application/x-www-form-urlencoded", encoded),
            ("multipart/form-data; boundary=test-boundary", multipart),
        ] {
            let response = router
                .handle_request(request(Method::Post, "/items/1", Some(content_type), data))
                .await;
            assert_eq!(response.status(), 201);
            assert_eq!(
                json_body(response).await["body"],
                json!({"age":30,"enabled":true,"tags":[1,2],"nested.name":"Joe"})
            );
        }
    }

    #[tokio::test]
    async fn form_infinity_errors_preserve_checked_coercion() {
        let schema = object([("number", Schema::Number)]);
        let registered = definer().bind(route(Method::Post, Some(schema), Schema::Unknown), echo);
        let response = router(registered)
            .handle_request(accepting(
                request(
                    Method::Post,
                    "/items/1",
                    Some("application/x-www-form-urlencoded"),
                    "number=Infinity",
                ),
                "text/plain",
            ))
            .await;
        assert_eq!(response.status(), 400);
        assert_eq!(
            text(response).await,
            "Body does not match defined schema: [\n  {\n    \"expected\": \"number\",\n    \"code\": \"invalid_type\",\n    \"received\": \"Infinity\",\n    \"path\": [\n      \"number\"\n    ],\n    \"message\": \"Invalid input: expected number, received number\"\n  }\n]"
        );
    }

    #[tokio::test]
    async fn error_kinds_negotiate_accept_and_hide_internal_details() {
        let logs = Arc::new(Mutex::new(Vec::new()));
        let log_capture = logs.clone();
        let definer = definer().with_options(HandlerOptions {
            error_logger: Arc::new(move |event| log_capture.lock().unwrap().push(event)),
            ..HandlerOptions::default()
        });
        for (error, status, message) in [
            (HttpError::not_found(), 404, "Not Found - Missing entry"),
            (HttpError::unauthorized(), 401, "Unauthorized"),
            (HttpError::forbidden(), 403, "Forbidden"),
            (
                HttpError::new(422, "invalid operation"),
                422,
                "invalid operation",
            ),
            (
                HttpError::new(500, "explicit request error"),
                500,
                "explicit request error",
            ),
            (
                HttpError::unexpected("private credential"),
                500,
                "Internal Server Error",
            ),
        ] {
            let registered = definer.bind(
                route(Method::Get, None, Schema::Unknown),
                move |_, _, _, _| {
                    let error = error.clone();
                    async move { Err(error) }
                },
            );
            let router = router(registered);
            for (accept, content_type, is_json) in [
                ("application/json", "application/json", true),
                ("text/html,application/json", "application/json", true),
                ("text/plain,text/html", "text/html", false),
                ("text/plain", "text/plain", false),
                ("*/*", "text/html", false),
                ("", "text/html", false),
            ] {
                let response = router
                    .handle_request(accepting(
                        request(Method::Get, "/items/1", None, ""),
                        accept,
                    ))
                    .await;
                assert_eq!(response.status(), status);
                assert_eq!(response.headers()["content-type"], content_type);
                let expected = if is_json {
                    json!(message).to_string()
                } else {
                    message.into()
                };
                assert_eq!(text(response).await, expected);
            }
        }
        let logs = logs.lock().unwrap();
        assert_eq!(logs.len(), 6);
        assert_eq!(logs[0].message, "Internal Server Error");
        assert_eq!(logs[0].url, "http://localhost/items/1");
        assert_eq!(logs[0].error, HttpError::unexpected("private credential"));
    }

    #[tokio::test]
    async fn custom_error_parser_precedes_auth_and_builtin_error_kinds() {
        let logs = Arc::new(Mutex::new(Vec::new()));
        let log_capture = logs.clone();
        let options = HandlerOptions {
            error_parser: Some(Arc::new(|error| {
                async move {
                    Some(ParsedError {
                        status: 410,
                        message: format!("parsed: {}", error.message),
                    })
                }
                .boxed()
            })),
            error_logger: Arc::new(move |event| log_capture.lock().unwrap().push(event)),
            ..HandlerOptions::default()
        };
        let definer = definer().with_options(options);
        for error in [
            HttpError::not_found(),
            HttpError::forbidden(),
            HttpError::new(422, "request"),
            HttpError::unexpected("generic"),
        ] {
            let message = format!("parsed: {}", error.message);
            let registered = definer.bind(
                route(Method::Get, None, Schema::Unknown),
                move |_, _, _, _| {
                    let error = error.clone();
                    async move { Err(error) }
                },
            );
            let response = router(registered)
                .handle_request(accepting(
                    request(Method::Get, "/items/1", None, ""),
                    "text/plain",
                ))
                .await;
            assert_eq!(response.status(), 410);
            assert_eq!(text(response).await, message);
        }
        assert!(logs.lock().unwrap().is_empty());
        let definer = HandlerDefiner::new(
            |_| async { Ok(()) },
            |_, _: String, _| async { Ok(Authorization::Forbidden) },
        )
        .with_options(HandlerOptions {
            error_parser: Some(Arc::new(|_| {
                async {
                    Some(ParsedError {
                        status: 418,
                        message: "auth parsed".into(),
                    })
                }
                .boxed()
            })),
            ..HandlerOptions::default()
        });
        let registered = definer.bind(
            route(Method::Get, None, Schema::Unknown),
            |_, _, _, _| async { panic!("auth") },
        );
        let response = router(registered)
            .handle_request(request(Method::Get, "/items/1", None, ""))
            .await;
        assert_eq!(response.status(), 418);
        assert_eq!(text(response).await, "auth parsed");
    }

    #[tokio::test]
    async fn html_error_formatter_receives_user_and_effective_request_context() {
        let options = HandlerOptions {
            error_html_formatter: Some(Arc::new(|status, message, context, user| {
                async move {
                    Ok(format!(
                        "<error status=\"{status}\" user=\"{}\" url=\"{}\">{message}</error>",
                        user.unwrap_or_else(|| "anonymous".into()),
                        context.url
                    ))
                }
                .boxed()
            })),
            ..HandlerOptions::default()
        };
        let definer = definer().with_options(options);
        let mut definition = route(Method::Get, None, Schema::Unknown);
        definition.prefixes = vec!["hu".into()];
        let registered = definer.bind(definition, |_, _, _, _| async {
            Err(HttpError::not_found())
        });
        let response = router(registered)
            .handle_request(request(Method::Get, "/hu/items/1?q=yes", None, ""))
            .await;
        assert_eq!(response.headers()["content-type"], "text/html");
        assert_eq!(
            text(response).await,
            "<error status=\"404\" user=\"reader\" url=\"http://localhost/items/1?q=yes\">Not Found - Missing entry</error>"
        );
        let definer: HandlerDefiner<String, String> = HandlerDefiner::new(
            |_| async { Err(HttpError::unauthorized()) },
            |_, _, _| async { Ok(Authorization::Ok) },
        )
        .with_options(HandlerOptions {
            error_html_formatter: Some(Arc::new(|_, message, _, user| {
                async move {
                    assert!(user.is_none());
                    Ok(format!("anonymous: {message}"))
                }
                .boxed()
            })),
            ..HandlerOptions::default()
        });
        let registered = definer.bind(route(Method::Get, None, Schema::Unknown), echo);
        let response = router(registered)
            .handle_request(request(Method::Get, "/items/1", None, ""))
            .await;
        assert_eq!(response.status(), 401);
        assert_eq!(text(response).await, "anonymous: Unauthorized");
    }

    #[tokio::test]
    async fn format_status_redirect_headers_and_bytes_are_exact() {
        for (status, redirect, expected) in [
            (None, false, 201),
            (None, true, 303),
            (Some(307), true, 307),
            (Some(202), false, 202),
        ] {
            let registered = definer().bind_formatted(
                route(Method::Post, None, Schema::Unknown),
                echo,
                move |data, user, context, params| async move {
                    assert_eq!(user, "reader");
                    assert_eq!(context.url.path(), "/items/1");
                    assert_eq!(params, json!({"itemId":1}));
                    assert_eq!(data.resolve().await?["params"], params);
                    let mut headers = HeaderMap::new();
                    headers.insert("location", "/next".parse().unwrap());
                    headers.insert("x-custom", "yes".parse().unwrap());
                    Ok(Formatted {
                        data: Body::from("custom"),
                        body_kind: crate::BodyKind::Text,
                        headers,
                        status,
                        redirect,
                    })
                },
            );
            let response = router(registered)
                .handle_request(request(Method::Post, "/items/1", None, ""))
                .await;
            assert_eq!(response.status(), expected);
            assert_eq!(response.headers()["location"], "/next");
            assert_eq!(response.headers()["x-custom"], "yes");
            assert!(!response.headers().contains_key("content-type"));
            assert_eq!(text(response).await, "custom");
        }
    }

    #[tokio::test]
    async fn json_resolves_deep_futures_objects_arrays_and_future_produced_futures() {
        let value = DeferredValue::future(async {
            Ok(DeferredValue::object([(
                "rows".into(),
                DeferredValue::array([
                    DeferredValue::object([(
                        "tags".into(),
                        DeferredValue::array([DeferredValue::future(async {
                            Ok(DeferredValue::ready("x"))
                        })]),
                    )]),
                    DeferredValue::future(async {
                        Ok(DeferredValue::future(async {
                            Ok(DeferredValue::array([
                                DeferredValue::ready(4),
                                DeferredValue::future(async { Ok(DeferredValue::ready(5)) }),
                            ]))
                        }))
                    }),
                ]),
            )]))
        });
        let registered = definer().bind(
            route(Method::Get, None, Schema::Unknown),
            move |_, _, _, _| {
                let value = value.clone();
                async move { Ok(value) }
            },
        );
        let response = router(registered)
            .handle_request(request(Method::Get, "/items/1", None, ""))
            .await;
        assert_eq!(response.status(), 200);
        assert_eq!(
            json_body(response).await,
            json!({"rows":[{"tags":["x"]},[4,5]]})
        );
    }

    #[tokio::test]
    async fn invalid_outputs_warn_with_original_value_issues_method_and_rewritten_url() {
        let warnings = Arc::new(Mutex::new(Vec::new()));
        let capture = warnings.clone();
        let definer = definer().with_options(HandlerOptions {
            output_warning: Some(Arc::new(move |warning| {
                capture.lock().unwrap().push(warning)
            })),
            ..HandlerOptions::default()
        });
        let mut definition = route(Method::Get, None, object([("name", Schema::String)]));
        definition.prefixes = vec!["hu".into()];
        let registered = definer.bind(definition, |_, _, _, prefix| async move {
            assert_eq!(prefix.as_deref(), Some("hu"));
            Ok(DeferredValue::object([
                (
                    "name".into(),
                    DeferredValue::future(async { Ok(DeferredValue::ready(4)) }),
                ),
                ("extra".into(), DeferredValue::ready(true)),
            ]))
        });
        let response = router(registered)
            .handle_request(request(Method::Get, "/hu/items/1?q=x", None, ""))
            .await;
        assert_eq!(json_body(response).await, json!({"name":4,"extra":true}));
        let warnings = warnings.lock().unwrap();
        assert_eq!(warnings.len(), 1);
        let warning = &warnings[0];
        assert_eq!(warning.data, json!({"name":4,"extra":true}));
        assert_eq!(warning.method, Method::Get);
        assert_eq!(warning.url, "http://localhost/items/1?q=x");
        assert_eq!(
            warning.error.issues,
            vec![
                json!({"expected":"string","code":"invalid_type","path":["name"],"message":"Invalid input: expected string, received number"})
            ]
        );
    }

    #[tokio::test]
    async fn streaming_first_chunk_precedes_gated_fields_and_ignored_fields_still_warn() {
        let warnings = Arc::new(Mutex::new(Vec::new()));
        let capture = warnings.clone();
        let definer = definer().with_options(HandlerOptions {
            output_warning: Some(Arc::new(move |warning| {
                capture.lock().unwrap().push(warning)
            })),
            ..HandlerOptions::default()
        });
        let (release, gated) = oneshot::channel();
        let (started, started_rx) = oneshot::channel();
        let value = DeferredValue::object([
            ("title".into(), DeferredValue::ready("Hello")),
            (
                "ignored".into(),
                DeferredValue::future(async move {
                    started.send(()).unwrap();
                    gated.await.unwrap();
                    Ok(DeferredValue::ready(123))
                }),
            ),
            ("plain".into(), DeferredValue::ready(false)),
        ]);
        let mut definition = route(
            Method::Get,
            None,
            object([
                ("title", Schema::String),
                ("ignored", Schema::String),
                ("plain", Schema::Number),
            ]),
        );
        definition.prefixes = vec!["de".into()];
        let registered = definer.bind_formatted(
            definition,
            move |_, _, _, _| {
                let value = value.clone();
                async move { Ok(value) }
            },
            |data, _, _, _| async move {
                let DeferredValue::Object(fields) = data else {
                    panic!("raw object")
                };
                assert!(matches!(fields["ignored"], DeferredValue::Future(_)));
                assert!(
                    matches!(&fields["title"], DeferredValue::Ready(value) if value == "Hello")
                );
                let stream =
                    stream::iter([Ok::<_, Infallible>(Bytes::from_static(b"<h1>Hello</h1>"))]);
                Ok(Formatted {
                    data: Body::from_stream(stream),
                    ..Formatted::default()
                })
            },
        );
        let response = tokio::time::timeout(
            Duration::from_secs(1),
            router(registered).handle_request(request(Method::Get, "/de/items/1", None, "")),
        )
        .await
        .expect("response cannot wait for an ignored field");
        let mut body = response.into_body();
        let frame = tokio::time::timeout(Duration::from_secs(1), body.frame())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(frame.into_data().unwrap(), "<h1>Hello</h1>");
        tokio::time::timeout(Duration::from_secs(1), started_rx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(warnings.lock().unwrap().len(), 1);
        assert_eq!(
            warnings.lock().unwrap()[0].url,
            "http://localhost/items/1#plain"
        );
        release.send(()).unwrap();
        definer.drain_background().await.unwrap();
        let warnings = warnings.lock().unwrap();
        assert_eq!(warnings.len(), 2);
        assert_eq!(warnings[1].url, "http://localhost/items/1#ignored");
        assert_eq!(warnings[1].data, json!(123));
        assert_eq!(
            warnings[1].error.issues,
            vec![
                json!({"expected":"string","code":"invalid_type","path":[],"message":"Invalid input: expected string, received number"})
            ]
        );
    }

    #[tokio::test]
    async fn formatter_stream_can_await_a_deferred_second_chunk() {
        let definer = definer();
        let (release, gated) = oneshot::channel();
        let value = DeferredValue::future(async move {
            Ok(DeferredValue::object([(
                "content".into(),
                DeferredValue::future(async move {
                    gated.await.unwrap();
                    Ok(DeferredValue::ready("World"))
                }),
            )]))
        });
        let registered = definer.bind_formatted(
            route(Method::Get, None, Schema::Unknown),
            move |_, _, _, _| {
                let value = value.clone();
                async move { Ok(value) }
            },
            |data, _, _, _| async move {
                let DeferredValue::Object(mut fields) = data else {
                    panic!("root future should be unfolded")
                };
                let content = fields.swap_remove("content").unwrap();
                assert!(matches!(content, DeferredValue::Future(_)));
                let stream =
                    stream::once(async { Ok::<_, HttpError>(Bytes::from_static(b"Hello")) }).chain(
                        stream::once(async move {
                            let content = content.resolve().await?;
                            Ok(Bytes::from(content.as_str().unwrap().to_owned()))
                        }),
                    );
                Ok(Formatted {
                    data: Body::from_stream(stream),
                    ..Formatted::default()
                })
            },
        );
        use futures_util::StreamExt;
        let response = router(registered)
            .handle_request(request(Method::Get, "/items/1", None, ""))
            .await;
        let mut body = response.into_body();
        let first = tokio::time::timeout(Duration::from_secs(1), body.frame())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(first.into_data().unwrap(), "Hello");
        release.send(()).unwrap();
        assert_eq!(
            body.frame().await.unwrap().unwrap().into_data().unwrap(),
            "World"
        );
        assert!(body.frame().await.is_none());
        definer.drain_background().await.unwrap();
    }

    #[tokio::test]
    async fn rejected_json_futures_return_hidden_500_and_background_failures_are_drainable() {
        let logs = Arc::new(Mutex::new(Vec::new()));
        let capture = logs.clone();
        let definer = definer().with_options(HandlerOptions {
            error_logger: Arc::new(move |log| capture.lock().unwrap().push(log)),
            ..HandlerOptions::default()
        });
        let handler = |_, _, _, _| async {
            Ok(DeferredValue::array([
                DeferredValue::ready(1),
                DeferredValue::future(async { Err(HttpError::unexpected("deep failure")) }),
            ]))
        };
        let registered = definer.bind(route(Method::Get, None, Schema::Unknown), handler);
        let response = router(registered)
            .handle_request(request(Method::Get, "/items/1", None, ""))
            .await;
        assert_eq!(response.status(), 500);
        assert_eq!(text(response).await, "Internal Server Error");
        assert_eq!(logs.lock().unwrap()[0].error.message, "deep failure");
        let registered = definer.bind_formatted(
            route(Method::Get, None, Schema::Unknown),
            handler,
            |_, _, _, _| async { Ok(Formatted::default()) },
        );
        let response = router(registered)
            .handle_request(request(Method::Get, "/items/1", None, ""))
            .await;
        assert_eq!(response.status(), 200);
        assert_eq!(text(response).await, "");
        let error = definer.drain_background().await.unwrap_err();
        assert_eq!(error, HttpError::unexpected("deep failure"));
        assert_eq!(logs.lock().unwrap().len(), 2);
        definer.drain_background().await.unwrap();
    }

    #[tokio::test]
    async fn cleanup_cancels_and_joins_pending_fields_without_clone_drop_aborting_them() {
        struct Dropped(Arc<AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let definer = definer();
        let dropped = Arc::new(AtomicBool::new(false));
        let drop_guard = Dropped(dropped.clone());
        let (started, started_rx) = oneshot::channel();
        let value = DeferredValue::object([(
            "ignored".into(),
            DeferredValue::future(async move {
                let _guard = drop_guard;
                started.send(()).unwrap();
                std::future::pending().await
            }),
        )]);
        let registered = definer.bind_formatted(
            route(Method::Get, None, Schema::Unknown),
            move |_, _, _, _| {
                let value = value.clone();
                async move { Ok(value) }
            },
            |_, _, _, _| async { Ok(Formatted::default()) },
        );
        let router = router(registered);
        let clone = definer.clone();
        let response = router
            .handle_request(request(Method::Get, "/items/1", None, ""))
            .await;
        assert_eq!(response.status(), 200);
        tokio::time::timeout(Duration::from_secs(1), started_rx)
            .await
            .unwrap()
            .unwrap();
        drop(clone);
        assert!(!dropped.load(Ordering::SeqCst));
        // The handler closure also owns the shared future. Release routes before cleanup.
        drop(router);
        definer.cleanup_background().await.unwrap();
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn typed_routes_infer_params_body_and_output_and_skip_get_body_derivation() {
        #[derive(JsonSchema, Deserialize)]
        struct Params {
            #[serde(rename = "itemId")]
            item_id: u32,
            active: Option<bool>,
        }
        #[derive(JsonSchema, Deserialize)]
        struct Input {
            title: String,
            count: u16,
        }
        #[derive(JsonSchema, Serialize)]
        struct Output {
            title: String,
            count: u32,
        }
        let route = TypedRoute::<Params, Input, Output, _>::new(
            Method::Post,
            "/items/:itemId",
            "user".to_owned(),
            vec![],
        )
        .unwrap();
        let registered = definer().bind_typed(route, |params, body, user, prefix| async move {
            let input = body.unwrap();
            assert_eq!(params.active, Some(true));
            assert_eq!(user, "reader");
            assert_eq!(prefix, None);
            Ok(Output {
                title: input.title,
                count: u32::from(input.count) + params.item_id,
            })
        });
        let router = router(registered);
        let response = router
            .handle_request(request(
                Method::Post,
                "/items/7?active=true",
                Some("application/json"),
                r#"{"title":"Hi","count":5,"extra":true}"#,
            ))
            .await;
        assert_eq!(response.status(), 201);
        assert_eq!(json_body(response).await, json!({"title":"Hi","count":12}));
        let response = router
            .handle_request(request(
                Method::Post,
                "/items/7?active=true",
                Some("application/json"),
                r#"{"title":"Hi","count":-1}"#,
            ))
            .await;
        assert_eq!(response.status(), 400);
        assert!(
            text(response)
                .await
                .contains("Body does not match defined schema")
        );
        let get = TypedRoute::<Params, std::collections::HashMap<String, String>, Output, _>::new(
            Method::Get,
            "/items/:itemId",
            "user".to_owned(),
            vec![],
        )
        .unwrap();
        assert!(get.definition.body.is_none());
        let registered = definer().bind_typed(get, |params, body, _, _| async move {
            assert!(body.is_none());
            Ok(Output {
                title: "typed GET".into(),
                count: params.item_id,
            })
        });
        let response = super::tests::router(registered)
            .handle_request(request(Method::Get, "/items/3", None, ""))
            .await;
        assert_eq!(
            json_body(response).await,
            json!({"title":"typed GET","count":3})
        );
    }

    #[tokio::test]
    async fn formatted_nested_future_validation_preserves_promise_types_without_deep_awaiting() {
        let warnings = Arc::new(Mutex::new(Vec::new()));
        let capture = warnings.clone();
        let definer = definer().with_options(HandlerOptions {
            output_warning: Some(Arc::new(move |warning| {
                capture.lock().unwrap().push(warning)
            })),
            ..HandlerOptions::default()
        });
        let schema = object([
            (
                "plain",
                object([
                    ("name", Schema::String),
                    ("object", object([("n", Schema::Number)])),
                ]),
            ),
            ("later", Schema::Array(Box::new(Schema::String))),
        ]);
        let registered = definer.bind_formatted(
            route(Method::Get, None, schema),
            |_, _, _, _| async {
                Ok(DeferredValue::object([
                    (
                        "plain".into(),
                        DeferredValue::object([
                            (
                                "name".into(),
                                DeferredValue::future(async {
                                    Ok(DeferredValue::ready("eventual"))
                                }),
                            ),
                            (
                                "object".into(),
                                DeferredValue::future(async {
                                    Ok(DeferredValue::ready(json!({"n":1})))
                                }),
                            ),
                        ]),
                    ),
                    (
                        "later".into(),
                        DeferredValue::future(async {
                            Ok(DeferredValue::array([DeferredValue::future(async {
                                Ok(DeferredValue::ready("eventual"))
                            })]))
                        }),
                    ),
                ]))
            },
            |data, _, _, _| async move {
                let DeferredValue::Object(fields) = data else {
                    panic!("raw object")
                };
                assert!(matches!(fields["later"], DeferredValue::Future(_)));
                Ok(Formatted::default())
            },
        );
        let response = router(registered)
            .handle_request(request(Method::Get, "/items/1", None, ""))
            .await;
        assert_eq!(response.status(), 200);
        definer.drain_background().await.unwrap();
        let warnings = warnings.lock().unwrap();
        assert_eq!(warnings.len(), 2);
        assert_eq!(warnings[0].url, "http://localhost/items/1#plain");
        assert_eq!(warnings[0].data, json!({"name":{},"object":{}}));
        assert_eq!(
            warnings[0].error.issues,
            vec![
                json!({"expected":"string","code":"invalid_type","path":["name"],"message":"Invalid input: expected string, received Promise"}),
                json!({"expected":"number","code":"invalid_type","path":["object","n"],"message":"Invalid input: expected number, received undefined"}),
            ]
        );
        assert_eq!(warnings[1].url, "http://localhost/items/1#later");
        assert_eq!(warnings[1].data, json!([{}]));
        assert_eq!(
            warnings[1].error.issues,
            vec![
                json!({"expected":"string","code":"invalid_type","path":[0],"message":"Invalid input: expected string, received Promise"})
            ]
        );
    }

    #[tokio::test]
    async fn successful_output_transforms_apply_and_json_uses_ecmascript_number_and_key_order() {
        let schema = object([("number", Schema::Number)]).transform(|value| {
            let number = value.unwrap()["number"].as_f64().unwrap();
            Ok(Some(
                json!({"z":number,"10":1.0,"2":-0.0,"small":0.0000001}),
            ))
        });
        let registered = definer().bind(route(Method::Get, None, schema), |_, _, _, _| async {
            Ok(DeferredValue::ready(json!({"number":1e21,"ignored":true})))
        });
        let response = router(registered)
            .handle_request(request(Method::Get, "/items/1", None, ""))
            .await;
        assert_eq!(response.status(), 200);
        assert_eq!(
            text(response).await,
            r#"{"2":0,"10":1,"z":1e+21,"small":1e-7}"#
        );
    }

    fn multipart_files() -> Vec<u8> {
        let mut bytes = concat!(
            "--upload\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\ndemo\r\n",
            "--upload\r\nContent-Disposition: form-data; name=\"files\"; filename=\"a.bin\"\r\n",
            "Content-Type: application/octet-stream\r\n\r\n",
        )
        .as_bytes()
        .to_vec();
        bytes.extend([0, 255, 1]);
        bytes.extend_from_slice(concat!(
            "\r\n--upload\r\nContent-Disposition: form-data; name=\"files\"; filename=\"b.txt\"\r\n\r\nhello\n",
            "\r\n--upload--\r\n",
        ).as_bytes());
        bytes
    }

    fn upload_request() -> Request {
        http::Request::builder()
            .method("POST")
            .uri("http://localhost/items/1")
            .header("content-type", "multipart/form-data; boundary=upload")
            .body(Body::from(multipart_files()))
            .unwrap()
    }

    #[tokio::test]
    async fn multipart_file_bytes_metadata_and_repeated_fields_reach_handler_context() {
        let schema = object([
            ("title", Schema::String),
            ("files", Schema::Array(Box::new(Schema::File))),
        ]);
        let registered = definer().bind_with_context(
            route(Method::Post, Some(schema), Schema::Unknown),
            |params, body, user, prefix, context| async move {
                assert_eq!(params, json!({"itemId":1}));
                assert_eq!(body, Some(json!({"title":"demo","files":[{},{}]})));
                assert_eq!(user, "reader");
                assert_eq!(prefix, None);
                assert_eq!(context.uploads.len(), 2);
                let file = &context.uploads[0];
                assert_eq!(file.field_name, "files");
                assert_eq!(file.filename, "a.bin");
                assert_eq!(
                    file.content_type.as_deref(),
                    Some("application/octet-stream")
                );
                assert_eq!(file.data.as_ref(), &[0, 255, 1]);
                assert_eq!(context.uploads[1].filename, "b.txt");
                assert_eq!(context.uploads[1].data.as_ref(), b"hello\n");
                assert_eq!(context.uploads[1].content_type, None);
                Ok(DeferredValue::ready(
                    json!({"uploaded":context.uploads.len()}),
                ))
            },
        );
        let response = router(registered).handle_request(upload_request()).await;
        assert_eq!(response.status(), 201);
        assert_eq!(json_body(response).await, json!({"uploaded":2}));
    }

    #[tokio::test]
    async fn multipart_files_validate_as_blob_and_formatters_receive_actual_uploads() {
        let schema = object([("files", Schema::Array(Box::new(Schema::String)))]);
        let registered = definer().bind(route(Method::Post, Some(schema), Schema::Unknown), echo);
        let response = router(registered)
            .handle_request(accepting(upload_request(), "text/plain"))
            .await;
        assert_eq!(response.status(), 400);
        assert_eq!(
            text(response).await,
            "Body does not match defined schema: [\n  {\n    \"expected\": \"string\",\n    \"code\": \"invalid_type\",\n    \"path\": [\n      \"files\",\n      0\n    ],\n    \"message\": \"Invalid input: expected string, received Blob\"\n  },\n  {\n    \"expected\": \"string\",\n    \"code\": \"invalid_type\",\n    \"path\": [\n      \"files\",\n      1\n    ],\n    \"message\": \"Invalid input: expected string, received Blob\"\n  }\n]"
        );
        let registered = definer().bind_formatted(
            route(Method::Post, None, Schema::Unknown),
            echo,
            |_, _, context, _| async move {
                assert_eq!(context.uploads.len(), 2);
                Ok(Formatted {
                    data: Body::from(context.uploads[0].data.clone()),
                    ..Formatted::default()
                })
            },
        );
        let response = router(registered).handle_request(upload_request()).await;
        assert_eq!(response.status(), 201);
        assert_eq!(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .as_ref(),
            &[0, 255, 1]
        );
    }

    #[tokio::test]
    async fn overflowing_json_exponents_use_the_approved_nonportable_input_rejection() {
        let registered = definer().bind(route(Method::Post, None, Schema::Unknown), echo);
        let response = router(registered)
            .handle_request(request(
                Method::Post,
                "/items/1",
                Some("application/json"),
                "1e400",
            ))
            .await;
        assert_eq!(response.status(), 400);
        assert_eq!(text(response).await, "Body is not valid JSON");
    }

    #[tokio::test]
    async fn ignored_future_rejections_do_not_cancel_siblings_and_each_error_is_logged() {
        let logs = Arc::new(Mutex::new(Vec::new()));
        let capture = logs.clone();
        let completed = Arc::new(AtomicBool::new(false));
        let completion = completed.clone();
        let definer = definer().with_options(HandlerOptions {
            error_logger: Arc::new(move |log| capture.lock().unwrap().push(log)),
            ..HandlerOptions::default()
        });
        let registered = definer.bind_formatted(
            route(Method::Get, None, Schema::Unknown),
            move |_, _, _, _| {
                let completion = completion.clone();
                async move {
                    Ok(DeferredValue::array([
                        DeferredValue::future(async { Err(HttpError::unexpected("first")) }),
                        DeferredValue::future(async { Err(HttpError::unexpected("second")) }),
                        DeferredValue::future(async move {
                            completion.store(true, Ordering::SeqCst);
                            Ok(DeferredValue::ready(true))
                        }),
                    ]))
                }
            },
            |_, _, _, _| async { Ok(Formatted::default()) },
        );
        let response = router(registered)
            .handle_request(request(Method::Get, "/items/1", None, ""))
            .await;
        assert_eq!(response.status(), 200);
        assert_eq!(
            definer.drain_background().await.unwrap_err().message,
            "first"
        );
        assert!(completed.load(Ordering::SeqCst));
        let logs = logs.lock().unwrap();
        assert_eq!(
            logs.iter()
                .map(|log| log.error.message.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    #[tokio::test]
    async fn callback_panics_are_logged_and_return_hidden_500() {
        let logs = Arc::new(Mutex::new(Vec::new()));
        let capture = logs.clone();
        let definer = definer().with_options(HandlerOptions {
            error_logger: Arc::new(move |log| capture.lock().unwrap().push(log)),
            ..HandlerOptions::default()
        });
        let registered = definer.bind(
            route(Method::Get, None, Schema::Unknown),
            |_, _, _, _| async { panic!("private panic message") },
        );
        let response = router(registered)
            .handle_request(request(Method::Get, "/items/1", None, ""))
            .await;
        assert_eq!(response.status(), 500);
        assert_eq!(text(response).await, "Internal Server Error");
        assert_eq!(
            logs.lock().unwrap()[0].error.message,
            "private panic message"
        );
    }
}
