use crate::ClientError;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::fmt;
use switchboard::{HttpError, TypedRoute, ValidationError, params::def_to_url};

/// Typed-route boundary failures retain their original schema/serde error and source.
#[derive(Debug)]
pub enum RouteCallError {
    Transport(ClientError),
    Serialization(serde_json::Error),
    Parameters(HttpError),
    Validation(ValidationError),
    Deserialization(serde_json::Error),
}

impl fmt::Display for RouteCallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => error.fmt(f),
            Self::Serialization(error) => write!(f, "Request serialization failed: {error}"),
            Self::Parameters(error) => error.fmt(f),
            Self::Validation(error) => error.fmt(f),
            Self::Deserialization(error) => write!(f, "Typed response decoding failed: {error}"),
        }
    }
}

impl std::error::Error for RouteCallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::Transport(error) => error,
            Self::Serialization(error) | Self::Deserialization(error) => error,
            Self::Parameters(error) => error,
            Self::Validation(error) => error,
        })
    }
}

#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn route_path<P: Serialize, B, O, Permission>(
    route: &TypedRoute<P, B, O, Permission>,
    params: &P,
) -> Result<String, RouteCallError> {
    let params = serde_json::to_value(params).map_err(RouteCallError::Serialization)?;
    def_to_url(&route.definition.path, &route.definition.params, params)
        .map_err(RouteCallError::Parameters)
}

#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn decode_output<P, B, O: DeserializeOwned, Permission>(
    route: &TypedRoute<P, B, O, Permission>,
    value: Option<Value>,
) -> Result<Option<O>, RouteCallError> {
    value
        .map(|value| {
            let validated = route
                .definition
                .output
                .parse_value(value)
                .map_err(RouteCallError::Validation)?;
            serde_json::from_value(validated).map_err(RouteCallError::Deserialization)
        })
        .transpose()
}

#[cfg(target_arch = "wasm32")]
impl crate::BrowserClient {
    /// Validates the shared output schema before decoding the Rust output contract.
    pub async fn call_route<P: Serialize, B: Serialize, O: DeserializeOwned, Permission>(
        &self,
        route: &TypedRoute<P, B, O, Permission>,
        params: &P,
        body: Option<&B>,
        settings: &crate::CallSettings,
    ) -> Result<Option<O>, RouteCallError> {
        let value = self
            .call_route_unchecked(route, params, body, settings)
            .await?;
        decode_output(route, value)
    }

    /// Validates/encodes parameters, but returns untrusted JSON without claiming O.
    pub async fn call_route_unchecked<P: Serialize, B: Serialize, O, Permission>(
        &self,
        route: &TypedRoute<P, B, O, Permission>,
        params: &P,
        body: Option<&B>,
        settings: &crate::CallSettings,
    ) -> Result<Option<Value>, RouteCallError> {
        let path = route_path(route, params)?;
        let body = body
            .map(serde_json::to_value)
            .transpose()
            .map_err(RouteCallError::Serialization)?;
        self.call(
            route.definition.method.as_str(),
            &path,
            body.as_ref(),
            settings,
        )
        .await
        .map_err(RouteCallError::Transport)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serializer};
    use serde_json::json;
    use switchboard::{Method, Schema};

    #[derive(Serialize, Deserialize, JsonSchema)]
    struct Params {
        #[serde(rename = "itemId")]
        item_id: u32,
        query: String,
    }

    #[derive(Debug, PartialEq, Deserialize, JsonSchema)]
    struct Output {
        title: String,
    }

    fn route() -> TypedRoute<Params, Value, Output, ()> {
        TypedRoute::new(Method::Get, "/items/:itemId", (), vec![]).unwrap()
    }

    #[test]
    fn shared_parameters_encode_path_and_query() {
        assert_eq!(
            route_path(
                &route(),
                &Params {
                    item_id: 42,
                    query: "a b&c".into()
                }
            )
            .unwrap(),
            "/items/42?query=a+b%26c"
        );
    }

    #[test]
    fn schema_validation_is_distinct_from_serde_and_transport() {
        let mut route = route();
        route.definition.output =
            Schema::Object([("title".into(), Schema::Literal(json!("allowed")))].into());
        assert_eq!(
            decode_output(&route, Some(json!({"title":"allowed","extra":1}))).unwrap(),
            Some(Output {
                title: "allowed".into()
            })
        );
        let error = decode_output(&route, Some(json!({"title":"rejected"}))).unwrap_err();
        assert!(std::error::Error::source(&error).is_some());
        let RouteCallError::Validation(error) = error else {
            panic!("expected schema validation error");
        };
        assert_eq!(
            error.issues,
            vec![json!({
                "code": "invalid_value", "values": ["allowed"], "path": ["title"],
                "message": "Invalid input: expected \"allowed\""
            })]
        );
        route.definition.output = Schema::Unknown;
        assert!(matches!(
            decode_output(&route, Some(json!({"title":1}))),
            Err(RouteCallError::Deserialization(_))
        ));
        assert_eq!(decode_output(&route, None).unwrap(), None);
    }

    #[test]
    fn parameter_errors_and_serialization_are_distinct() {
        let mut route = route();
        route.definition.params = Schema::Object([("missing".into(), Schema::String)].into());
        assert!(matches!(
            route_path(
                &route,
                &Params {
                    item_id: 42,
                    query: String::new()
                }
            ),
            Err(RouteCallError::Parameters(_))
        ));
        #[derive(JsonSchema)]
        struct Broken {}
        impl Serialize for Broken {
            fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("broken params"))
            }
        }
        let route =
            TypedRoute::<Broken, Value, Value, ()>::new(Method::Get, "/", (), vec![]).unwrap();
        assert!(matches!(
            route_path(&route, &Broken {}),
            Err(RouteCallError::Serialization(_))
        ));
    }
}
