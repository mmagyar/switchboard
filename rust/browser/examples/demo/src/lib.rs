#![forbid(unsafe_code)]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use switchboard::{DefinitionError, Method, Schema, TypedRoute};

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemParams {
    pub item_id: u32,
    pub query: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct Item {
    pub title: String,
    pub query: String,
}

pub fn item_route() -> Result<TypedRoute<ItemParams, (), Item, ()>, DefinitionError> {
    let mut route = TypedRoute::new(Method::Get, "/api/items/:itemId", (), vec![])?;
    // A serde-compatible string can still violate the shared output refinement.
    if let Schema::Object(fields) = &mut route.definition.output {
        fields.insert("title".into(), Schema::Literal("Item 42".into()));
    }
    Ok(route)
}

pub fn refinement_route() -> Result<TypedRoute<ItemParams, (), Item, ()>, DefinitionError> {
    let mut route = item_route()?;
    route.definition.path = "/api/refinement/:itemId".into();
    Ok(route)
}

#[cfg(target_arch = "wasm32")]
mod page;
#[cfg(target_arch = "wasm32")]
pub use page::{Page, start};
