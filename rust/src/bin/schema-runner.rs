#![forbid(unsafe_code)]

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{self, BufRead, Write},
};
use switchboard::{Schema, params, schema::ValidationError};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
enum Case {
    Parse {
        schema: Source,
        #[serde(default, deserialize_with = "present_value")]
        input: Option<Value>,
    },
    Form {
        schema: Source,
        input: Value,
    },
    Coerce {
        schema: Source,
        input: Value,
    },
    Url {
        schema: Source,
        path: String,
        url: String,
    },
    Def {
        schema: Source,
        path: String,
        input: Value,
    },
}

// serde's ordinary Option treats JSON null as absence, while Zod distinguishes them.
fn present_value<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Source {
    Derived(DerivedSource),
    Portable(Schema),
}

#[derive(Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "camelCase",
    deny_unknown_fields
)]
enum DerivedSource {
    Derived(DerivedName),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum DerivedName {
    Map,
    Tuple,
    Open,
    Tree,
    Mutual,
    Containers,
    Strict,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct MapValue {
    count: f64,
    pairs: Vec<(String, bool)>,
}
#[derive(JsonSchema)]
#[allow(dead_code)]
struct OpenFields {
    known: String,
    #[serde(flatten)]
    extra: BTreeMap<String, f64>,
}
#[derive(JsonSchema)]
#[allow(dead_code)]
struct Tree {
    name: String,
    children: Vec<Tree>,
    next: Option<Box<Tree>>,
}
#[derive(JsonSchema)]
#[allow(dead_code)]
struct MutualA {
    b: Option<Box<MutualB>>,
}
#[derive(JsonSchema)]
#[allow(dead_code)]
struct MutualB {
    a: Vec<MutualA>,
    count: f64,
}
#[derive(JsonSchema)]
#[allow(dead_code)]
struct Containers {
    map: BTreeMap<String, String>,
    tuple: (String, String),
}

#[derive(JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct StrictFields {
    known: String,
}

impl Source {
    fn compile(self) -> Result<Schema, switchboard::DefinitionError> {
        match self {
            Self::Portable(schema) => Ok(schema),
            Self::Derived(DerivedSource::Derived(name)) => match name {
                DerivedName::Map => Schema::from_type::<BTreeMap<String, MapValue>>(),
                DerivedName::Tuple => Schema::from_type::<(String, bool, f64)>(),
                DerivedName::Open => Schema::from_type::<OpenFields>(),
                DerivedName::Tree => Schema::from_type::<Tree>(),
                DerivedName::Mutual => Schema::from_type::<MutualA>(),
                DerivedName::Containers => Schema::from_type::<Containers>(),
                DerivedName::Strict => Schema::from_type::<StrictFields>(),
            },
        }
    }
}

fn validation_output(result: Result<Option<Value>, ValidationError>) -> Value {
    match result {
        Ok(Some(data)) => json!({"success":true,"present":true,"data":data}),
        Ok(None) => json!({"success":true,"present":false}),
        Err(error) => json!({"success":false,"issues":error.issues,"message":error.to_string()}),
    }
}

fn run(input: Case) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(match input {
        Case::Parse { schema, input } => validation_output(schema.compile()?.parse(input)),
        Case::Form { schema, input } => {
            validation_output(params::parse_form(&schema.compile()?, input).map(Some))
        }
        Case::Coerce { schema, input } => match params::coerce_form(&schema.compile()?, input) {
            Ok(value) => value,
            Err(error) => validation_output(Err(error)),
        },
        Case::Url { schema, path, url } => validation_output(
            params::parse_url(&url::Url::parse(&url)?, &path, &schema.compile()?).map(Some),
        ),
        Case::Def {
            schema,
            path,
            input,
        } => match params::def_to_url(&path, &schema.compile()?, input) {
            Ok(data) => json!({"success":true,"data":data}),
            Err(error) => json!({"success":false,"message":error.message}),
        },
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for (index, line) in io::stdin().lock().lines().enumerate() {
        let result = (|| {
            let case: Case = serde_json::from_str(&line?)?;
            run(case)
        })();
        let output = result.map_err(|error| format!("Line {}: {error}", index + 1))?;
        serde_json::to_writer(&mut stdout, &output)?;
        writeln!(stdout)?;
    }
    stdout.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(input: Value) -> Value {
        run(serde_json::from_value(input).unwrap()).unwrap()
    }

    #[test]
    fn schema_runner_preserves_absence_and_explicit_json_null() {
        assert_eq!(
            output(json!({"op":"parse","schema":{"type":"optional","value":{"type":"string"}}})),
            json!({"success":true,"present":false})
        );
        assert_eq!(
            output(json!({"op":"parse","schema":{"type":"literal","value":null},"input":null})),
            json!({"success":true,"present":true,"data":null})
        );
        assert_eq!(
            output(json!({"op":"parse","schema":{"type":"string"},"input":null}))["issues"][0]["message"],
            "Invalid input: expected string, received null"
        );
    }

    #[test]
    fn schema_runner_uses_the_checked_form_coercion_result() {
        assert_eq!(
            output(
                json!({"op":"coerce","schema":{"type":"object","value":{"n":{"type":"number"}}},"input":{"n":"2"}})
            ),
            json!({"n":2})
        );
        assert_eq!(
            output(
                json!({"op":"form","schema":{"type":"object","value":{"n":{"type":"number"}}},"input":{"n":"wrong"}})
            )["issues"][0]["path"],
            json!(["n"])
        );
    }
}
