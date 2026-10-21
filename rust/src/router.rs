use crate::{
    DefinitionError, Handler, Method, RegisteredRoute, Request, RequestContext, Response, response,
};
use axum::body::Body;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

#[derive(Default)]
struct Node {
    static_children: HashMap<String, Node>,
    param: Option<Box<ParamEdge>>,
    handlers: [Option<usize>; 7],
}

struct ParamEdge {
    name: String,
    optional: bool,
    node: Node,
}

pub struct RouteEntry {
    pub method: Method,
    pub path: String,
    pub prefix: Option<String>,
    handler: Handler,
}

pub struct Router {
    root: Node,
    routes: Vec<RouteEntry>,
    registered: HashSet<String>,
    default: Handler,
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

impl Router {
    pub fn new() -> Self {
        Self::with_default(Arc::new(|_, _| {
            Box::pin(async { response(404, "", "Not found - Route not defined") })
        }))
    }

    pub fn with_default(default: Handler) -> Self {
        Self {
            root: Node::default(),
            routes: Vec::new(),
            registered: HashSet::new(),
            default,
        }
    }

    pub fn add_route(&mut self, route: RegisteredRoute) -> Result<(), DefinitionError> {
        validate_optional_order(&route.path)?;
        let path = if route.path.starts_with('/') {
            route.path.clone()
        } else {
            format!("/{}", route.path)
        };
        self.insert(route.method, path.clone(), None, route.handler.clone())?;
        for prefix in route.prefixes {
            self.insert(
                route.method,
                format!("/{prefix}{path}"),
                Some(prefix),
                route.handler.clone(),
            )?;
        }
        Ok(())
    }

    fn insert(
        &mut self,
        method: Method,
        path: String,
        prefix: Option<String>,
        handler: Handler,
    ) -> Result<(), DefinitionError> {
        let key = format!("{}:{path}", method.as_str());
        if !self.registered.insert(key) {
            return Err(DefinitionError(format!(
                "Duplicate route: {} {path}",
                method.as_str().to_ascii_uppercase()
            )));
        }
        let mut node = &mut self.root;
        for segment in path.split('/').filter(|s| !s.is_empty()) {
            if let Some(parameter) = segment.strip_prefix(':') {
                let optional = parameter.ends_with('?');
                let name = parameter.strip_suffix('?').unwrap_or(parameter);
                if let Some(edge) = &node.param {
                    if edge.name != name {
                        return Err(DefinitionError(format!(
                            "Conflicting param names: cannot add \":{name}\" because \":{}\" already exists at the same trie position. Route: {path}",
                            edge.name
                        )));
                    }
                    if edge.optional != optional {
                        let existing = if edge.optional {
                            "optional"
                        } else {
                            "mandatory"
                        };
                        let incoming = if optional { "optional" } else { "mandatory" };
                        let suffix = if edge.optional { "?" } else { "" };
                        return Err(DefinitionError(format!(
                            "Conflicting param edges: cannot add {incoming} param \":{name}\" because an {existing} param \":{}{suffix}\" already exists at the same position. Route: {path}",
                            edge.name
                        )));
                    }
                }
                let edge = node.param.get_or_insert_with(|| {
                    Box::new(ParamEdge {
                        name: name.to_owned(),
                        optional,
                        node: Node::default(),
                    })
                });
                node = &mut edge.node;
            } else {
                node = node.static_children.entry(segment.to_owned()).or_default();
            }
        }
        let index = self.routes.len();
        self.routes.push(RouteEntry {
            method,
            path,
            prefix,
            handler,
        });
        node.handlers[method.index()] = Some(index);
        Ok(())
    }

    pub fn lookup(&self, method: Method, path: &str) -> Option<&RouteEntry> {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut pending = vec![(&self.root, 0)];
        while let Some((node, index)) = pending.pop() {
            if index == segments.len() {
                if let Some(route) = node.handlers[method.index()] {
                    return self.routes.get(route);
                }
                if let Some(edge) = &node.param {
                    if edge.optional {
                        pending.push((&edge.node, index));
                    }
                }
                continue;
            }
            if let Some(edge) = &node.param {
                pending.push((&edge.node, index + 1));
            }
            if let Some(child) = node.static_children.get(segments[index]) {
                pending.push((child, index + 1));
            }
        }
        None
    }

    pub async fn handle_request(&self, request: Request) -> Response {
        let context = match RequestContext::from_request(&request) {
            Ok(context) => context,
            Err(error) => return response(error.status, "", error.message),
        };
        if let Some(route) = self.lookup(context.method, context.url.path()) {
            return dispatch(route, request, context).await;
        }
        if context.method == Method::Head {
            if let Some(route) = self.lookup(Method::Get, context.url.path()) {
                let mut response = dispatch(route, request, context).await;
                *response.body_mut() = Body::empty();
                return response;
            }
        }
        (self.default)(request, None).await
    }
}

async fn dispatch(
    route: &RouteEntry,
    mut request: Request,
    mut context: RequestContext,
) -> Response {
    if let Some(prefix) = &route.prefix {
        let start = format!("/{prefix}");
        let stripped = context
            .url
            .path()
            .strip_prefix(&start)
            .unwrap_or(context.url.path());
        let path = if stripped.is_empty() {
            "/".to_owned()
        } else {
            stripped.to_owned()
        };
        context.url.set_path(&path);
        match context.url.as_str().parse() {
            Ok(uri) => *request.uri_mut() = uri,
            Err(_) => return response(400, "", "Invalid request URL"),
        }
    }
    (route.handler)(request, route.prefix.clone()).await
}

fn validate_optional_order(path: &str) -> Result<(), DefinitionError> {
    let mut optional_seen = false;
    for segment in path.strip_suffix('/').unwrap_or(path).split('/') {
        if segment.ends_with('?') {
            optional_seen = true;
        } else if optional_seen {
            return Err(DefinitionError(
                "Cannot have a non optional parameter after an optional parameter".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    fn route(method: Method, path: &str, label: &'static str) -> RegisteredRoute {
        RegisteredRoute {
            method,
            path: path.into(),
            prefixes: vec![],
            handler: Arc::new(move |_, _| {
                Box::pin(async move { response(200, "text/plain", label) })
            }),
        }
    }
    fn request(method: &str, path: &str) -> Request {
        http::Request::builder()
            .method(method)
            .uri(format!("http://localhost{path}"))
            .body(Body::empty())
            .unwrap()
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

    #[tokio::test]
    async fn static_precedence_and_parameter_backtracking() {
        let mut router = Router::new();
        router
            .add_route(route(Method::Get, "/users/:id", "parameter"))
            .unwrap();
        router
            .add_route(route(Method::Get, "/users/me", "static"))
            .unwrap();
        router
            .add_route(route(Method::Get, "/users/:id/posts", "posts"))
            .unwrap();
        assert_eq!(
            text(router.handle_request(request("GET", "/users/me")).await).await,
            "static"
        );
        assert_eq!(
            text(
                router
                    .handle_request(request("GET", "/users/me/posts"))
                    .await
            )
            .await,
            "posts"
        );
        assert_eq!(
            text(router.handle_request(request("GET", "/users/12")).await).await,
            "parameter"
        );
    }

    #[tokio::test]
    async fn optional_paths_methods_and_normalization() {
        let mut router = Router::new();
        router
            .add_route(route(Method::Get, "x/:a?/:b?", "optional"))
            .unwrap();
        router
            .add_route(route(Method::Post, "/x/:a?/:b?", "post"))
            .unwrap();
        for path in ["/x", "/x/1", "/x/1/2", "//x//1//2/"] {
            assert_eq!(
                text(router.handle_request(request("GET", path)).await).await,
                "optional"
            );
        }
        assert_eq!(
            text(router.handle_request(request("POST", "/x")).await).await,
            "post"
        );
        assert_eq!(
            router
                .handle_request(request("GET", "/x/1/2/3"))
                .await
                .status(),
            404
        );
    }

    #[test]
    fn definitions_reject_duplicates_invalid_order_and_cross_method_conflicts() {
        let mut router = Router::new();
        router.add_route(route(Method::Get, "/x/:a", "x")).unwrap();
        assert_eq!(
            router
                .add_route(route(Method::Get, "/x/:a", "x"))
                .unwrap_err()
                .0,
            "Duplicate route: GET /x/:a"
        );
        assert!(
            router
                .add_route(route(Method::Post, "/x/:b", "x"))
                .unwrap_err()
                .0
                .starts_with("Conflicting param names:")
        );
        assert!(
            router
                .add_route(route(Method::Post, "/x/:a?", "x"))
                .unwrap_err()
                .0
                .starts_with("Conflicting param edges:")
        );
        assert_eq!(
            router
                .add_route(route(Method::Get, "/bad/:a?/b", "x"))
                .unwrap_err()
                .0,
            "Cannot have a non optional parameter after an optional parameter"
        );
    }

    #[tokio::test]
    async fn head_fallback_keeps_metadata_and_explicit_head_wins() {
        let mut router = Router::new();
        let mut get = route(Method::Get, "/x", "get");
        get.handler =
            Arc::new(|_, _| Box::pin(async { response(202, "application/custom", "body") }));
        router.add_route(get).unwrap();
        let head = router.handle_request(request("HEAD", "/x")).await;
        assert_eq!(head.status(), 202);
        assert_eq!(head.headers()["content-type"], "application/custom");
        assert_eq!(text(head).await, "");
        router.add_route(route(Method::Head, "/x", "head")).unwrap();
        assert_eq!(
            text(router.handle_request(request("HEAD", "/x")).await).await,
            "head"
        );
    }

    #[tokio::test]
    async fn unknown_method_is_405_before_default_handler() {
        let router = Router::new();
        let response = router.handle_request(request("TRACE", "/x")).await;
        assert_eq!(response.status(), 405);
        assert_eq!(text(response).await, "Method Not Allowed: TRACE");
        assert_eq!(
            router.handle_request(request("GET", "/x")).await.status(),
            404
        );
    }

    #[tokio::test]
    async fn prefix_rewrites_url_and_preserves_headers_body_and_query() {
        let mut router = Router::new();
        router
            .add_route(RegisteredRoute {
                method: Method::Post,
                path: "/x/:id".into(),
                prefixes: vec!["hu".into(), "de".into()],
                handler: Arc::new(|request, prefix| {
                    Box::pin(async move {
                        let uri = request.uri().to_string();
                        let header = request.headers()["x-check"].to_str().unwrap().to_owned();
                        let body = request.into_body().collect().await.unwrap().to_bytes();
                        response(
                            200,
                            "text/plain",
                            format!(
                                "{}|{uri}|{header}|{}",
                                prefix.unwrap_or_default(),
                                String::from_utf8_lossy(&body)
                            ),
                        )
                    })
                }),
            })
            .unwrap();
        let req = http::Request::builder()
            .method("POST")
            .uri("http://localhost/hu/x/4?q=yes")
            .header("x-check", "kept")
            .body(Body::from("payload"))
            .unwrap();
        assert_eq!(
            text(router.handle_request(req).await).await,
            "hu|http://localhost/x/4?q=yes|kept|payload"
        );
        assert_eq!(router.lookup(Method::Post, "/x/4").unwrap().prefix, None);
    }

    #[tokio::test]
    async fn custom_default_and_relative_server_uris() {
        let router = Router::with_default(Arc::new(|req, _| {
            Box::pin(async move {
                let context = RequestContext::from_request(&req).unwrap();
                response(410, "text/plain", context.url.to_string())
            })
        }));
        let req = http::Request::builder()
            .uri("/gone?q=1")
            .header("host", "example.test:99")
            .body(Body::empty())
            .unwrap();
        let result = router.handle_request(req).await;
        assert_eq!(result.status(), 410);
        assert_eq!(text(result).await, "http://example.test:99/gone?q=1");
    }
}
