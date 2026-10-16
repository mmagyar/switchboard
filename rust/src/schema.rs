use crate::DefinitionError;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fmt, sync::Arc};

pub type Refinement = Arc<dyn Fn(Option<&Value>) -> Result<(), ValidationError> + Send + Sync>;
pub type Transformation =
    Arc<dyn Fn(Option<Value>) -> Result<Option<Value>, ValidationError> + Send + Sync>;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum Schema {
    Unknown,
    String,
    Number,
    Boolean,
    Literal(Value),
    Enum(Vec<String>),
    Object(IndexMap<String, Schema>),
    Array(Box<Schema>),
    Union(Vec<Schema>),
    Optional(Box<Schema>),
    Nullable(Box<Schema>),
    Default(Box<Schema>, Value),
    Catch(Box<Schema>, Value),
    Readonly(Box<Schema>),
    #[serde(skip)]
    Integer {
        signed: bool,
        bits: u8,
    },
    #[serde(skip)]
    Refinement(Box<Schema>, Refinement),
    #[serde(skip)]
    Transform(Box<Schema>, Transformation),
    #[serde(skip)]
    Preprocess(Box<Schema>, Transformation),
}

impl fmt::Debug for Schema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Integer { signed, bits } => f
                .debug_struct("Integer")
                .field("signed", signed)
                .field("bits", bits)
                .finish(),
            Self::Refinement(inner, _) => f.debug_tuple("Refinement").field(inner).finish(),
            Self::Transform(inner, _) => f.debug_tuple("Transform").field(inner).finish(),
            Self::Preprocess(inner, _) => f.debug_tuple("Preprocess").field(inner).finish(),
            _ => match serde_json::to_string(self) {
                Ok(value) => f.write_str(&value),
                Err(_) => f.write_str("Schema with runtime callbacks"),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValidationError {
    pub issues: Vec<Value>,
}

impl ValidationError {
    pub fn custom(message: impl Into<String>) -> Self {
        Self {
            issues: vec![json!({"code": "custom", "path": [], "message": message.into()})],
        }
    }

    fn at(mut self, path: &[Value]) -> Self {
        for issue in &mut self.issues {
            if let Some(fields) = issue.as_object_mut() {
                let mut full = path.to_vec();
                if let Some(Value::Array(relative)) = fields.get("path") {
                    full.extend(relative.iter().cloned());
                }
                fields.insert("path".into(), Value::Array(full));
            }
        }
        self
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match serde_json::to_string_pretty(&self.issues) {
            Ok(message) => f.write_str(&message),
            Err(_) => Err(fmt::Error),
        }
    }
}
impl std::error::Error for ValidationError {}

// JSON cannot represent sparse array holes. Keeping absence here preserves Zod's undefined errors.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Input {
    Absent,
    Promise,
    Scalar(Value),
    NonFinite { original: String },
    Object(IndexMap<String, Input>),
    Array(Vec<Input>),
}

impl Input {
    pub(crate) fn from_value(value: Value) -> Self {
        match value {
            Value::Object(fields) => Self::Object(
                fields
                    .into_iter()
                    .map(|(k, v)| (k, Self::from_value(v)))
                    .collect(),
            ),
            Value::Array(items) => Self::Array(items.into_iter().map(Self::from_value).collect()),
            other => Self::Scalar(other),
        }
    }

    pub(crate) fn to_value(&self) -> Option<Value> {
        match self {
            Self::Absent => None,
            Self::Promise => Some(json!({})),
            Self::Scalar(value) => Some(value.clone()),
            Self::NonFinite { .. } => Some(Value::Null),
            Self::Object(fields) => Some(Value::Object(
                fields
                    .iter()
                    .filter_map(|(k, v)| v.to_value().map(|v| (k.clone(), v)))
                    .collect(),
            )),
            Self::Array(items) => Some(Value::Array(
                items
                    .iter()
                    .map(|v| v.to_value().unwrap_or(Value::Null))
                    .collect(),
            )),
        }
    }

    fn type_name(&self) -> &'static str {
        match self {
            Self::Absent => "undefined",
            Self::Promise => "Promise",
            Self::Scalar(Value::Null) => "null",
            Self::Scalar(Value::Bool(_)) => "boolean",
            Self::Scalar(Value::Number(_)) | Self::NonFinite { .. } => "number",
            Self::Scalar(Value::String(_)) => "string",
            Self::Object(_) => "object",
            Self::Array(_) => "array",
            Self::Scalar(_) => unreachable!("containers use dedicated input variants"),
        }
    }
}

fn invalid_type(expected: &str, input: &Input, path: &[Value]) -> ValidationError {
    let mut issue = serde_json::Map::new();
    issue.insert("expected".into(), json!(expected));
    issue.insert("code".into(), json!("invalid_type"));
    if matches!(input, Input::NonFinite { .. }) && expected == "number" {
        issue.insert("received".into(), json!("Infinity"));
    }
    issue.insert("path".into(), json!(path));
    issue.insert(
        "message".into(),
        json!(format!(
            "Invalid input: expected {expected}, received {}",
            input.type_name()
        )),
    );
    ValidationError {
        issues: vec![Value::Object(issue)],
    }
}

fn invalid_values(values: Vec<Value>, path: &[Value]) -> ValidationError {
    let options = values
        .iter()
        .map(|v| {
            if let Value::String(s) = v {
                format!("\"{s}\"")
            } else {
                v.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("|");
    let message = if values.len() == 1 {
        format!("Invalid input: expected {options}")
    } else {
        format!("Invalid option: expected one of {options}")
    };
    ValidationError {
        issues: vec![
            json!({"code": "invalid_value", "values": values, "path": path, "message": message}),
        ],
    }
}

struct Parsed {
    value: Option<Value>,
    fallback: bool,
}

impl Parsed {
    fn new(value: Option<Value>) -> Self {
        Self {
            value,
            fallback: false,
        }
    }
}

impl Schema {
    pub fn parse(&self, value: Option<Value>) -> Result<Option<Value>, ValidationError> {
        self.parse_input(&value.map(Input::from_value).unwrap_or(Input::Absent))
    }

    pub fn parse_value(&self, value: Value) -> Result<Value, ValidationError> {
        self.parse(Some(value))?
            .ok_or_else(|| ValidationError::custom("Schema returned undefined for a JSON value"))
    }

    pub(crate) fn parse_input(&self, input: &Input) -> Result<Option<Value>, ValidationError> {
        self.parse_at(input, &[], 0).map(|parsed| parsed.value)
    }

    fn parse_at(
        &self,
        input: &Input,
        path: &[Value],
        depth: usize,
    ) -> Result<Parsed, ValidationError> {
        if depth > 128 {
            return Err(ValidationError::custom("Schema nesting exceeds 128 levels").at(path));
        }
        let inner = |schema: &Schema| schema.parse_at(input, path, depth + 1);
        match self {
            Self::Unknown => Ok(Parsed::new(input.to_value())),
            Self::String if matches!(input, Input::Scalar(Value::String(_))) => {
                Ok(Parsed::new(input.to_value()))
            }
            Self::Number if matches!(input, Input::Scalar(Value::Number(_))) => {
                Ok(Parsed::new(input.to_value()))
            }
            Self::Boolean if matches!(input, Input::Scalar(Value::Bool(_))) => {
                Ok(Parsed::new(input.to_value()))
            }
            Self::String => Err(invalid_type("string", input, path)),
            Self::Number => Err(invalid_type("number", input, path)),
            Self::Integer { signed, bits } => parse_integer(input, *signed, *bits)
                .map(|v| Parsed::new(Some(v)))
                .map_err(|e| e.at(path)),
            Self::Boolean => Err(invalid_type("boolean", input, path)),
            Self::Literal(expected) => {
                if !matches!(input, Input::NonFinite { .. } | Input::Promise)
                    && input
                        .to_value()
                        .as_ref()
                        .is_some_and(|actual| match (actual, expected) {
                            (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
                            _ => actual == expected,
                        })
                {
                    Ok(Parsed::new(input.to_value()))
                } else {
                    Err(invalid_values(vec![expected.clone()], path))
                }
            }
            Self::Enum(options) => {
                if matches!(input, Input::Scalar(Value::String(s)) if options.contains(s)) {
                    Ok(Parsed::new(input.to_value()))
                } else {
                    Err(invalid_values(
                        options.iter().cloned().map(Value::String).collect(),
                        path,
                    ))
                }
            }
            Self::Object(shape) => {
                let empty = IndexMap::new();
                let fields = match input {
                    Input::Object(fields) => fields,
                    Input::Promise => &empty,
                    _ => return Err(invalid_type("object", input, path)),
                };
                let mut output = serde_json::Map::new();
                let mut issues = Vec::new();
                for (key, schema) in shape {
                    let mut child_path = path.to_vec();
                    child_path.push(Value::String(key.clone()));
                    let present = fields.contains_key(key);
                    match schema.parse_at(fields.get(key).unwrap_or(&Input::Absent), &child_path, depth + 1) {
                        Ok(_) if !present && !schema.optional_input() => issues.push(json!({"code":"invalid_type", "expected":"nonoptional", "path":child_path, "message":"Invalid input: expected nonoptional, received undefined"})),
                        Ok(Parsed { value: Some(value), .. }) => { output.insert(key.clone(), value); }
                        Ok(Parsed { value: None, .. }) => {}
                        Err(_) if !present && schema.optional_input() && schema.optional_output() => {}
                        Err(error) => issues.extend(error.issues),
                    }
                }
                if issues.is_empty() {
                    Ok(Parsed::new(Some(Value::Object(output))))
                } else {
                    Err(ValidationError { issues })
                }
            }
            Self::Array(element) => {
                let Input::Array(items) = input else {
                    return Err(invalid_type("array", input, path));
                };
                let mut output = Vec::with_capacity(items.len());
                let mut issues = Vec::new();
                for (index, item) in items.iter().enumerate() {
                    let mut child_path = path.to_vec();
                    child_path.push(json!(index));
                    match element.parse_at(item, &child_path, depth + 1) {
                        Ok(parsed) => output.push(parsed.value.unwrap_or(Value::Null)),
                        Err(error) => issues.extend(error.issues),
                    }
                }
                if issues.is_empty() {
                    Ok(Parsed::new(Some(Value::Array(output))))
                } else {
                    Err(ValidationError { issues })
                }
            }
            Self::Union(branches) => {
                let mut errors = Vec::new();
                for branch in branches {
                    match branch.parse_at(input, &[], depth + 1) {
                        Ok(value) => return Ok(value),
                        Err(error) => errors.push(error),
                    }
                }
                let continuable = errors
                    .iter()
                    .filter(|e| {
                        e.issues.iter().all(|i| {
                            !matches!(
                                i["code"].as_str(),
                                Some("invalid_type" | "invalid_value" | "invalid_union")
                            )
                        })
                    })
                    .collect::<Vec<_>>();
                if continuable.len() == 1 {
                    return Err(continuable[0].clone().at(path));
                }
                Err(ValidationError {
                    issues: vec![json!({
                        "code": "invalid_union", "errors": errors.into_iter().map(|e| e.issues).collect::<Vec<_>>(),
                        "path": path, "message": "Invalid input"
                    })],
                })
            }
            Self::Optional(schema) if matches!(input, Input::Absent) => {
                if !schema.optional_input() {
                    return Ok(Parsed::new(None));
                }
                match inner(schema) {
                    Ok(parsed) if !parsed.fallback => Ok(parsed),
                    _ => Ok(Parsed::new(None)),
                }
            }
            Self::Nullable(_) if matches!(input, Input::Scalar(Value::Null)) => {
                Ok(Parsed::new(Some(Value::Null)))
            }
            Self::Default(_, default) if matches!(input, Input::Absent) => {
                Ok(Parsed::new(Some(default.clone())))
            }
            Self::Default(schema, default) => {
                let mut parsed = inner(schema)?;
                if parsed.value.is_none() {
                    parsed.value = Some(default.clone());
                }
                Ok(parsed)
            }
            Self::Catch(schema, fallback) => Ok(inner(schema).unwrap_or_else(|_| Parsed {
                value: Some(fallback.clone()),
                fallback: true,
            })),
            Self::Refinement(schema, check) => {
                let parsed = inner(schema)?;
                check(parsed.value.as_ref()).map_err(|e| e.at(path))?;
                Ok(parsed)
            }
            Self::Transform(schema, transform) => {
                let parsed = inner(schema)?;
                Ok(Parsed {
                    value: transform(parsed.value).map_err(|e| e.at(path))?,
                    fallback: true,
                })
            }
            Self::Preprocess(schema, preprocess) => {
                let value = preprocess(input.to_value()).map_err(|e| e.at(path))?;
                let mut parsed = schema.parse_at(
                    &value.map(Input::from_value).unwrap_or(Input::Absent),
                    path,
                    depth + 1,
                )?;
                parsed.fallback = true;
                Ok(parsed)
            }
            Self::Optional(schema) | Self::Nullable(schema) | Self::Readonly(schema) => {
                inner(schema)
            }
        }
    }

    fn optional_input(&self) -> bool {
        match self {
            Self::Optional(_) | Self::Default(_, _) | Self::Catch(_, _) => true,
            Self::Nullable(inner)
            | Self::Readonly(inner)
            | Self::Refinement(inner, _)
            | Self::Transform(inner, _) => inner.optional_input(),
            Self::Preprocess(_, _) => true,
            Self::Union(branches) => branches.iter().any(Self::optional_input),
            _ => false,
        }
    }

    fn optional_output(&self) -> bool {
        match self {
            Self::Optional(_) | Self::Catch(_, _) => true,
            Self::Nullable(inner)
            | Self::Readonly(inner)
            | Self::Refinement(inner, _)
            | Self::Preprocess(inner, _) => inner.optional_output(),
            Self::Union(branches) => branches.iter().any(Self::optional_output),
            _ => false,
        }
    }

    pub fn refine(
        self,
        check: impl Fn(Option<&Value>) -> Result<(), ValidationError> + Send + Sync + 'static,
    ) -> Self {
        Self::Refinement(Box::new(self), Arc::new(check))
    }

    pub fn transform(
        self,
        transform: impl Fn(Option<Value>) -> Result<Option<Value>, ValidationError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self::Transform(Box::new(self), Arc::new(transform))
    }

    pub fn preprocess(
        self,
        preprocess: impl Fn(Option<Value>) -> Result<Option<Value>, ValidationError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self::Preprocess(Box::new(self), Arc::new(preprocess))
    }

    pub(crate) fn unwrapped(&self) -> &Self {
        match self {
            Self::Optional(inner)
            | Self::Nullable(inner)
            | Self::Default(inner, _)
            | Self::Catch(inner, _)
            | Self::Readonly(inner)
            | Self::Refinement(inner, _)
            | Self::Transform(inner, _) => inner.unwrapped(),
            _ => self,
        }
    }

    pub fn validate_params(&self) -> Result<(), DefinitionError> {
        self.validate_params_at("", 0)
    }

    fn validate_params_at(&self, path: &str, depth: usize) -> Result<(), DefinitionError> {
        if depth > 128 {
            return Err(DefinitionError("Schema nesting exceeds 128 levels".into()));
        }
        match self.unwrapped() {
            Self::Union(branches) => {
                let origins = branches
                    .iter()
                    .filter(|b| {
                        matches!(
                            b.unwrapped(),
                            Self::String
                                | Self::Number
                                | Self::Integer { .. }
                                | Self::Boolean
                                | Self::Enum(_)
                                | Self::Literal(Value::String(_))
                        )
                    })
                    .collect::<Vec<_>>();
                if origins.len() > 1 {
                    let at = if path.is_empty() {
                        String::new()
                    } else {
                        format!(" at field \"{path}\"")
                    };
                    let names = origins
                        .iter()
                        .map(|b| b.zod_name())
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(DefinitionError(format!(
                        "paramsValidation contains an ambiguous union{at}: multiple branches ({names}) can all match a raw URL string. Use a single unambiguous type, or use z.preprocess() to handle the conversion explicitly."
                    )));
                }
                for branch in branches {
                    branch.validate_params_at(path, depth + 1)?;
                }
            }
            Self::Object(shape) => {
                for (key, schema) in shape {
                    schema.validate_params_at(
                        &if path.is_empty() {
                            key.clone()
                        } else {
                            format!("{path}.{key}")
                        },
                        depth + 1,
                    )?;
                }
            }
            Self::Array(element) => element.validate_params_at(&format!("{path}[]"), depth + 1)?,
            _ => {}
        }
        Ok(())
    }

    fn zod_name(&self) -> &'static str {
        match self {
            Self::String => "ZodString",
            Self::Number | Self::Integer { .. } => "ZodNumber",
            Self::Boolean => "ZodBoolean",
            Self::Enum(_) => "ZodEnum",
            Self::Literal(_) => "ZodLiteral",
            Self::Optional(_) => "ZodOptional",
            Self::Nullable(_) => "ZodNullable",
            Self::Default(_, _) => "ZodDefault",
            Self::Catch(_, _) => "ZodCatch",
            Self::Readonly(_) => "ZodReadonly",
            Self::Transform(_, _) => "ZodPipe",
            Self::Refinement(inner, _) => inner.zod_name(),
            _ => "ZodType",
        }
    }

    pub fn from_type<T: schemars::JsonSchema>() -> Result<Self, DefinitionError> {
        let generated = schemars::schema_for!(T).to_value();
        compile_schema(&generated, &generated, &mut Vec::new(), 0)
    }
}

fn parse_integer(input: &Input, signed: bool, bits: u8) -> Result<Value, ValidationError> {
    if !matches!(bits, 8 | 16 | 32 | 64) {
        return Err(ValidationError::custom(
            "Integer width must be 8, 16, 32 or 64 bits",
        ));
    }
    let Input::Scalar(Value::Number(number)) = input else {
        return Err(invalid_type("number", input, &[]));
    };
    let value = number
        .as_f64()
        .ok_or_else(|| invalid_type("number", input, &[]))?;
    if value.fract() != 0.0 {
        return Err(ValidationError {
            issues: vec![
                json!({"expected":"int", "format":"safeint", "code":"invalid_type", "path":[], "message":"Invalid input: expected int, received number"}),
            ],
        });
    }
    let minimum = if signed { -(1_i128 << (bits - 1)) } else { 0 };
    let maximum = if signed {
        (1_i128 << (bits - 1)) - 1
    } else {
        (1_i128 << bits) - 1
    };
    let exact = if let Some(value) = number.as_i64() {
        i128::from(value)
    } else if let Some(value) = number.as_u64() {
        i128::from(value)
    } else {
        value as i128
    };
    if exact < minimum || exact > maximum {
        let lower = exact < minimum;
        let bound = if lower { minimum } else { maximum };
        let mut issue = serde_json::Map::new();
        issue.insert("origin".into(), json!("number"));
        issue.insert(
            "code".into(),
            json!(if lower { "too_small" } else { "too_big" }),
        );
        issue.insert(
            if lower { "minimum" } else { "maximum" }.into(),
            json!(bound),
        );
        issue.insert("inclusive".into(), json!(true));
        issue.insert("path".into(), json!([]));
        issue.insert(
            "message".into(),
            json!(format!(
                "{}: expected number to be {}{bound}",
                if lower { "Too small" } else { "Too big" },
                if lower { ">=" } else { "<=" }
            )),
        );
        return Err(ValidationError {
            issues: vec![Value::Object(issue)],
        });
    }
    if signed {
        Ok(json!(exact as i64))
    } else {
        Ok(json!(exact as u64))
    }
}

fn unsupported(detail: impl fmt::Display) -> DefinitionError {
    DefinitionError(format!("Unsupported generated JSON schema: {detail}"))
}

fn compile_schema(
    value: &Value,
    root: &Value,
    references: &mut Vec<String>,
    depth: usize,
) -> Result<Schema, DefinitionError> {
    if depth > 128 {
        return Err(unsupported("nesting exceeds 128 levels"));
    }
    if value == &Value::Bool(true) {
        return Ok(Schema::Unknown);
    }
    let object = value.as_object().ok_or_else(|| unsupported(value))?;
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "$schema"
                | "$id"
                | "$defs"
                | "definitions"
                | "$ref"
                | "title"
                | "description"
                | "examples"
                | "deprecated"
                | "readOnly"
                | "writeOnly"
                | "default"
                | "type"
                | "const"
                | "enum"
                | "anyOf"
                | "oneOf"
                | "properties"
                | "required"
                | "additionalProperties"
                | "items"
                | "minItems"
                | "maxItems"
                | "minimum"
                | "maximum"
                | "exclusiveMinimum"
                | "exclusiveMaximum"
                | "minLength"
                | "maxLength"
                | "format"
        ) {
            return Err(unsupported(format!("keyword {key}")));
        }
    }
    if let Some(reference) = object.get("$ref") {
        let reference = reference
            .as_str()
            .ok_or_else(|| unsupported("non-string $ref"))?;
        if object.keys().any(|k| {
            !matches!(
                k.as_str(),
                "$ref" | "$defs" | "$schema" | "title" | "description" | "default"
            )
        }) {
            return Err(unsupported("$ref with validation siblings"));
        }
        if references.iter().any(|r| r == reference) {
            return Err(unsupported(format!("recursive reference {reference}")));
        }
        let pointer = reference
            .strip_prefix('#')
            .ok_or_else(|| unsupported("external reference"))?;
        let target = root
            .pointer(pointer)
            .ok_or_else(|| unsupported(format!("unresolved reference {reference}")))?;
        references.push(reference.into());
        let result = compile_schema(target, root, references, depth + 1);
        references.pop();
        return result;
    }
    if let Some(format) = object.get("format") {
        if !matches!(
            format.as_str(),
            Some(
                "int8"
                    | "int16"
                    | "int32"
                    | "int64"
                    | "uint8"
                    | "uint16"
                    | "uint32"
                    | "uint64"
                    | "float"
                    | "double"
            )
        ) {
            return Err(unsupported(format!("format {format}")));
        }
    }
    let mut schema = if let Some(constant) = object.get("const") {
        Schema::Literal(constant.clone())
    } else if let Some(options) = object.get("enum") {
        let options = options
            .as_array()
            .ok_or_else(|| unsupported("non-array enum"))?;
        if options.iter().all(Value::is_string) {
            Schema::Enum(
                options
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
            )
        } else {
            Schema::Union(options.iter().cloned().map(Schema::Literal).collect())
        }
    } else if let Some(branches) = object.get("anyOf").or_else(|| object.get("oneOf")) {
        if object.contains_key("type") {
            return Err(unsupported("union with sibling type"));
        }
        Schema::Union(
            branches
                .as_array()
                .ok_or_else(|| unsupported("non-array union"))?
                .iter()
                .map(|v| compile_schema(v, root, references, depth + 1))
                .collect::<Result<_, _>>()?,
        )
    } else if let Some(Value::Array(types)) = object.get("type") {
        let branches = types
            .iter()
            .map(|kind| {
                let mut branch = object.clone();
                branch.insert("type".into(), kind.clone());
                compile_schema(&Value::Object(branch), root, references, depth + 1)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Schema::Union(branches)
    } else {
        match object.get("type").and_then(Value::as_str) {
            Some("string") => Schema::String,
            Some("integer") => match object.get("format").and_then(Value::as_str) {
                Some("int8") => Schema::Integer {
                    signed: true,
                    bits: 8,
                },
                Some("int16") => Schema::Integer {
                    signed: true,
                    bits: 16,
                },
                Some("int32") => Schema::Integer {
                    signed: true,
                    bits: 32,
                },
                Some("int64") => Schema::Integer {
                    signed: true,
                    bits: 64,
                },
                Some("uint8") => Schema::Integer {
                    signed: false,
                    bits: 8,
                },
                Some("uint16") => Schema::Integer {
                    signed: false,
                    bits: 16,
                },
                Some("uint32") => Schema::Integer {
                    signed: false,
                    bits: 32,
                },
                Some("uint64") => Schema::Integer {
                    signed: false,
                    bits: 64,
                },
                _ => Schema::Number,
            },
            Some("number") => Schema::Number,
            Some("boolean") => Schema::Boolean,
            Some("null") => Schema::Literal(Value::Null),
            Some("array") => Schema::Array(Box::new(compile_schema(
                object.get("items").unwrap_or(&Value::Bool(true)),
                root,
                references,
                depth + 1,
            )?)),
            Some("object") => {
                if object
                    .get("additionalProperties")
                    .is_some_and(|v| v != &Value::Bool(false))
                {
                    return Err(unsupported("open objects and map additionalProperties"));
                }
                let required = object.get("required").and_then(Value::as_array);
                let mut fields = IndexMap::new();
                if let Some(properties) = object.get("properties") {
                    for (name, value) in properties
                        .as_object()
                        .ok_or_else(|| unsupported("non-object properties"))?
                    {
                        let field = compile_schema(value, root, references, depth + 1)?;
                        let field = if required
                            .is_some_and(|r| r.iter().any(|v| v.as_str() == Some(name)))
                        {
                            field
                        } else {
                            Schema::Optional(Box::new(field))
                        };
                        fields.insert(name.clone(), field);
                    }
                }
                Schema::Object(fields)
            }
            None if object.is_empty()
                || object.keys().all(|k| {
                    matches!(k.as_str(), "$schema" | "$defs" | "title" | "description")
                }) =>
            {
                Schema::Unknown
            }
            kind => return Err(unsupported(format!("type {kind:?}"))),
        }
    };
    if object.get("type").and_then(Value::as_str) == Some("integer")
        && !matches!(schema, Schema::Integer { .. })
    {
        schema = schema.refine(|value| {
            if value.and_then(Value::as_f64).is_some_and(|n| n.fract() == 0.0) { Ok(()) }
            else { Err(ValidationError { issues: vec![json!({"expected":"int", "format":"safeint", "code":"invalid_type", "path":[], "message":"Invalid input: expected int, received number"})] }) }
        });
    }
    for (key, origin, lower, inclusive) in [
        ("minimum", "number", true, true),
        ("maximum", "number", false, true),
        ("exclusiveMinimum", "number", true, false),
        ("exclusiveMaximum", "number", false, false),
        ("minLength", "string", true, true),
        ("maxLength", "string", false, true),
        ("minItems", "array", true, true),
        ("maxItems", "array", false, true),
    ] {
        if let Some(bound) = object.get(key) {
            let limit = bound
                .as_f64()
                .ok_or_else(|| unsupported(format!("non-number {key}")))?;
            let bound = bound.clone();
            schema = schema.refine(move |value| {
                let actual = value.and_then(|v| match origin {
                    "number" => v.as_f64(),
                    "string" => v.as_str().map(|s| s.encode_utf16().count() as f64),
                    _ => v.as_array().map(|a| a.len() as f64),
                });
                let Some(actual) = actual else {
                    return Ok(());
                };
                let valid = if lower {
                    if inclusive {
                        actual >= limit
                    } else {
                        actual > limit
                    }
                } else if inclusive {
                    actual <= limit
                } else {
                    actual < limit
                };
                if valid {
                    return Ok(());
                }
                let code = if lower { "too_small" } else { "too_big" };
                let comparison = match (lower, inclusive) {
                    (true, true) => ">=",
                    (true, false) => ">",
                    (false, true) => "<=",
                    (false, false) => "<",
                };
                let unit = match origin {
                    "string" => " characters",
                    "array" => " items",
                    _ => "",
                };
                let mut issue = serde_json::Map::new();
                issue.insert("origin".into(), json!(origin));
                issue.insert("code".into(), json!(code));
                issue.insert(
                    if lower { "minimum" } else { "maximum" }.into(),
                    bound.clone(),
                );
                issue.insert("inclusive".into(), json!(inclusive));
                issue.insert("path".into(), json!([]));
                issue.insert(
                    "message".into(),
                    json!(format!(
                        "{}: expected {origin} to have {comparison}{}{unit}",
                        if lower { "Too small" } else { "Too big" },
                        bound
                    )),
                );
                if origin == "number" {
                    issue.insert(
                        "message".into(),
                        json!(format!(
                            "{}: expected number to be {comparison}{}",
                            if lower { "Too small" } else { "Too big" },
                            bound
                        )),
                    );
                }
                Err(ValidationError {
                    issues: vec![Value::Object(issue)],
                })
            });
        }
    }
    Ok(schema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    fn object(fields: impl IntoIterator<Item = (&'static str, Schema)>) -> Schema {
        Schema::Object(fields.into_iter().map(|(k, s)| (k.into(), s)).collect())
    }

    #[test]
    fn schema_wire_format_round_trips_all_portable_variants() {
        let cases = vec![
            (Schema::Unknown, json!({"type":"unknown"})),
            (Schema::String, json!({"type":"string"})),
            (Schema::Number, json!({"type":"number"})),
            (Schema::Boolean, json!({"type":"boolean"})),
            (
                Schema::Literal(json!(null)),
                json!({"type":"literal","value":null}),
            ),
            (
                Schema::Enum(vec!["a".into(), "b".into()]),
                json!({"type":"enum","value":["a","b"]}),
            ),
            (
                object([("z", Schema::String), ("a", Schema::Number)]),
                json!({"type":"object","value":{"z":{"type":"string"},"a":{"type":"number"}}}),
            ),
            (
                Schema::Array(Box::new(Schema::Boolean)),
                json!({"type":"array","value":{"type":"boolean"}}),
            ),
            (
                Schema::Union(vec![Schema::String, Schema::Number]),
                json!({"type":"union","value":[{"type":"string"},{"type":"number"}]}),
            ),
            (
                Schema::Optional(Box::new(Schema::Number)),
                json!({"type":"optional","value":{"type":"number"}}),
            ),
            (
                Schema::Nullable(Box::new(Schema::String)),
                json!({"type":"nullable","value":{"type":"string"}}),
            ),
            (
                Schema::Default(Box::new(Schema::Number), json!(7)),
                json!({"type":"default","value":[{"type":"number"},7]}),
            ),
            (
                Schema::Catch(Box::new(Schema::Boolean), json!(false)),
                json!({"type":"catch","value":[{"type":"boolean"},false]}),
            ),
            (
                Schema::Readonly(Box::new(Schema::String)),
                json!({"type":"readonly","value":{"type":"string"}}),
            ),
        ];
        for (schema, expected) in cases {
            let serialized = serde_json::to_value(schema).unwrap();
            assert_eq!(serialized, expected);
            let decoded: Schema = serde_json::from_value(serialized).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), expected);
        }
        assert!(serde_json::from_value::<Schema>(json!({"type":"number","value":false})).is_err());
    }

    #[test]
    fn unresolved_promises_preserve_formatter_warning_types() {
        let schema = Schema::Object(IndexMap::from([(
            "items".into(),
            Schema::Array(Box::new(Schema::Number)),
        )]));
        let input = Input::Object(IndexMap::from([(
            "items".into(),
            Input::Array(vec![Input::Promise]),
        )]));
        let error = schema.parse_input(&input).unwrap_err();
        assert_eq!(
            error.issues,
            vec![json!({
                "expected":"number", "code":"invalid_type", "path":["items",0],
                "message":"Invalid input: expected number, received Promise"
            })]
        );
        assert_eq!(
            Schema::Object(IndexMap::new())
                .parse_input(&Input::Promise)
                .unwrap(),
            Some(json!({}))
        );
        let required = Schema::Object(IndexMap::from([("n".into(), Schema::Number)]));
        assert_eq!(
            required.parse_input(&Input::Promise).unwrap_err().issues[0]["message"],
            "Invalid input: expected number, received undefined"
        );
        assert_eq!(input.to_value(), Some(json!({"items":[{}]})));
    }

    #[test]
    fn missing_and_null_have_distinct_exact_zod_errors() {
        let missing = Schema::Number.parse(None).unwrap_err();
        assert_eq!(
            missing.issues,
            vec![
                json!({"expected":"number","code":"invalid_type","path":[],"message":"Invalid input: expected number, received undefined"})
            ]
        );
        assert_eq!(
            missing.to_string(),
            "[\n  {\n    \"expected\": \"number\",\n    \"code\": \"invalid_type\",\n    \"path\": [],\n    \"message\": \"Invalid input: expected number, received undefined\"\n  }\n]"
        );
        assert_eq!(
            Schema::Number.parse(Some(Value::Null)).unwrap_err().issues[0]["message"],
            "Invalid input: expected number, received null"
        );
        assert_eq!(Schema::Unknown.parse(None).unwrap(), None);
        assert_eq!(
            Schema::Unknown.parse(Some(Value::Null)).unwrap(),
            Some(Value::Null)
        );
    }

    #[test]
    fn object_unknown_fields_require_presence_even_though_root_unknown_accepts_absence() {
        let schema = object([("value", Schema::Unknown)]);
        assert_eq!(
            schema.parse_value(json!({})).unwrap_err().issues,
            vec![
                json!({"code":"invalid_type","expected":"nonoptional","path":["value"],"message":"Invalid input: expected nonoptional, received undefined"})
            ]
        );
        assert_eq!(
            schema.parse_value(json!({"value":null})).unwrap(),
            json!({"value":null})
        );
        let refined_optional = object([(
            "value",
            Schema::Optional(Box::new(Schema::Number))
                .refine(|_| Err(ValidationError::custom("Refined"))),
        )]);
        assert_eq!(refined_optional.parse_value(json!({})).unwrap(), json!({}));
        assert!(refined_optional.parse_value(json!({"value":1})).is_err());
    }

    #[test]
    fn primitive_types_do_not_coerce() {
        for (schema, valid, invalid, expected) in [
            (Schema::String, json!("x"), json!(3), "string"),
            (Schema::Number, json!(2.5), json!("2.5"), "number"),
            (Schema::Boolean, json!(false), json!("false"), "boolean"),
        ] {
            assert_eq!(schema.parse_value(valid.clone()).unwrap(), valid);
            assert_eq!(
                schema.parse_value(invalid).unwrap_err().issues[0]["expected"],
                expected
            );
        }
    }

    #[test]
    fn literal_enum_and_empty_enum_errors_match_zod() {
        for (schema, expected) in [
            (
                Schema::Literal(json!("a")),
                json!({"code":"invalid_value","values":["a"],"path":[],"message":"Invalid input: expected \"a\""}),
            ),
            (
                Schema::Literal(json!(null)),
                json!({"code":"invalid_value","values":[null],"path":[],"message":"Invalid input: expected null"}),
            ),
            (
                Schema::Enum(vec!["a".into(), "b".into()]),
                json!({"code":"invalid_value","values":["a","b"],"path":[],"message":"Invalid option: expected one of \"a\"|\"b\""}),
            ),
            (
                Schema::Enum(vec![]),
                json!({"code":"invalid_value","values":[],"path":[],"message":"Invalid option: expected one of "}),
            ),
        ] {
            assert_eq!(
                schema.parse_value(json!(false)).unwrap_err().issues,
                vec![expected]
            );
        }
        assert_eq!(
            Schema::Enum(vec!["a".into()])
                .parse_value(json!("a"))
                .unwrap(),
            "a"
        );
        assert_eq!(Schema::Literal(json!(2)).parse_value(json!(2)).unwrap(), 2);
        assert_eq!(
            Schema::Literal(json!(2)).parse_value(json!(2.0)).unwrap(),
            json!(2.0)
        );
    }

    #[test]
    fn object_strips_unknown_fields_and_preserves_schema_order() {
        let schema = object([("z", Schema::String), ("a", Schema::Number)]);
        let output = schema
            .parse_value(json!({"a":1,"extra":true,"z":"last"}))
            .unwrap();
        assert_eq!(output.to_string(), "{\"z\":\"last\",\"a\":1}");
        assert_eq!(
            schema.parse_value(json!([])).unwrap_err().issues[0]["message"],
            "Invalid input: expected object, received array"
        );
    }

    #[test]
    fn collects_nested_issues_in_schema_then_array_order() {
        let schema = object([
            (
                "z",
                Schema::Array(Box::new(object([("count", Schema::Number)]))),
            ),
            ("a", Schema::Boolean),
        ]);
        let error = schema
            .parse_value(json!({"z":[{"count":"bad"},{}],"a":null}))
            .unwrap_err();
        assert_eq!(
            error
                .issues
                .iter()
                .map(|i| i["path"].clone())
                .collect::<Vec<_>>(),
            vec![
                json!(["z", 0, "count"]),
                json!(["z", 1, "count"]),
                json!(["a"])
            ]
        );
        assert_eq!(
            error.issues[1]["message"],
            "Invalid input: expected number, received undefined"
        );
    }

    #[test]
    fn optional_nullable_default_catch_and_readonly_have_distinct_semantics() {
        let optional = Schema::Optional(Box::new(Schema::Number));
        assert_eq!(optional.parse(None).unwrap(), None);
        assert!(optional.parse(Some(Value::Null)).is_err());
        let nullable = Schema::Nullable(Box::new(Schema::Number));
        assert_eq!(nullable.parse_value(Value::Null).unwrap(), Value::Null);
        assert!(nullable.parse(None).is_err());
        let default = Schema::Default(Box::new(Schema::Number), json!(9));
        assert_eq!(default.parse(None).unwrap(), Some(json!(9)));
        assert!(default.parse_value(Value::Null).is_err());
        let catch = Schema::Catch(Box::new(Schema::Number), json!(7));
        assert_eq!(catch.parse(None).unwrap(), Some(json!(7)));
        assert_eq!(catch.parse_value(json!("bad")).unwrap(), 7);
        assert_eq!(catch.parse_value(json!(2)).unwrap(), 2);
        assert_eq!(
            Schema::Readonly(Box::new(default)).parse(None).unwrap(),
            Some(json!(9))
        );
    }

    #[test]
    fn optional_preserves_default_but_suppresses_catch_and_transform_fallbacks() {
        let default = Schema::Optional(Box::new(Schema::Readonly(Box::new(Schema::Default(
            Box::new(Schema::String),
            json!("default"),
        )))));
        assert_eq!(default.parse(None).unwrap(), Some(json!("default")));
        let catch = Schema::Optional(Box::new(Schema::Catch(
            Box::new(Schema::String),
            json!("catch"),
        )));
        assert_eq!(catch.parse(None).unwrap(), None);
        let transform = Schema::Optional(Box::new(
            Schema::Optional(Box::new(Schema::String))
                .transform(|v| Ok(Some(v.unwrap_or(json!("transform"))))),
        ));
        assert_eq!(transform.parse(None).unwrap(), None);
        let default_after_transform = Schema::Default(
            Box::new(Schema::String.transform(|_| Ok(None))),
            json!("replacement"),
        );
        assert_eq!(
            default_after_transform.parse_value(json!("input")).unwrap(),
            "replacement"
        );
    }

    #[test]
    fn defaults_bypass_validation_and_objects_populate_optional_defaults() {
        let schema = object([
            ("missing", Schema::Optional(Box::new(Schema::String))),
            (
                "default",
                Schema::Default(Box::new(Schema::Number), json!("unchecked")),
            ),
        ]);
        assert_eq!(
            schema.parse_value(json!({})).unwrap(),
            json!({"default":"unchecked"})
        );
    }

    #[test]
    fn unions_use_first_success_and_relative_branch_error_paths() {
        let schema = object([(
            "choice",
            Schema::Union(vec![
                object([("a", Schema::Number)]),
                object([("b", Schema::Boolean)]),
            ]),
        )]);
        assert_eq!(
            schema
                .parse_value(json!({"choice":{"a":1,"b":true}}))
                .unwrap(),
            json!({"choice":{"a":1}})
        );
        let error = schema.parse_value(json!({"choice":{}})).unwrap_err();
        assert_eq!(error.issues[0]["path"], json!(["choice"]));
        assert_eq!(error.issues[0]["errors"][0][0]["path"], json!(["a"]));
        assert_eq!(error.issues[0]["errors"][1][0]["path"], json!(["b"]));
    }

    #[test]
    fn custom_refinement_transform_and_preprocess_are_runtime_only() {
        let schema = Schema::Number
            .refine(|v| {
                if v.and_then(Value::as_i64).is_some_and(|n| n > 0) {
                    Ok(())
                } else {
                    Err(ValidationError::custom("Must be positive"))
                }
            })
            .transform(|v| Ok(v.and_then(|v| v.as_i64().map(|n| json!(n * 2)))));
        assert_eq!(schema.parse_value(json!(3)).unwrap(), 6);
        let nested = object([("n", schema.clone())]);
        assert_eq!(
            nested.parse_value(json!({"n":0})).unwrap_err().issues,
            vec![json!({"code":"custom","path":["n"],"message":"Must be positive"})]
        );
        assert!(serde_json::to_value(&schema).is_err());
        let pre = Schema::Number
            .preprocess(|v| Ok(v.map(|v| if v == json!("three") { json!(3) } else { v })));
        assert_eq!(pre.parse_value(json!("three")).unwrap(), 3);
        assert!(pre.parse_value(json!(false)).is_err());
        assert!(serde_json::from_value::<Schema>(json!({"type":"transform"})).is_err());
    }

    #[test]
    fn union_preserves_single_continuable_refinement_error() {
        let schema = Schema::Union(vec![
            Schema::Number.refine(|_| Err(ValidationError::custom("Too low"))),
            Schema::String,
        ]);
        assert_eq!(
            schema.parse_value(json!(0)).unwrap_err().issues,
            vec![json!({"code":"custom","path":[],"message":"Too low"})]
        );
    }

    #[test]
    fn ambiguity_walks_wrappers_objects_arrays_and_nested_unions() {
        let bad = Schema::Union(vec![
            Schema::String,
            Schema::Optional(Box::new(Schema::Number)),
        ]);
        let schema = object([(
            "outer",
            Schema::Array(Box::new(Schema::Catch(Box::new(bad), json!(null)))),
        )]);
        assert_eq!(
            schema.validate_params().unwrap_err().0,
            "paramsValidation contains an ambiguous union at field \"outer[]\": multiple branches (ZodString, ZodOptional) can all match a raw URL string. Use a single unambiguous type, or use z.preprocess() to handle the conversion explicitly."
        );
        for wrapper in [Schema::Nullable, Schema::Readonly, Schema::Optional] {
            let schema = wrapper(Box::new(Schema::Union(vec![
                Schema::Boolean,
                Schema::Enum(vec!["x".into()]),
            ])));
            assert!(
                schema
                    .validate_params()
                    .unwrap_err()
                    .0
                    .contains("ambiguous union")
            );
        }
        let nested = Schema::Union(vec![
            object([]),
            Schema::Union(vec![Schema::Number, Schema::Boolean]),
        ]);
        assert!(nested.validate_params().is_err());
    }

    #[test]
    fn unambiguous_unions_and_explicit_preprocess_are_accepted() {
        for schema in [
            Schema::Union(vec![Schema::Number, Schema::Literal(json!(42))]),
            Schema::Union(vec![Schema::Boolean, object([("x", Schema::String)])]),
            Schema::Union(vec![
                object([("a", Schema::Number)]),
                object([("b", Schema::String)]),
            ]),
            Schema::Union(vec![Schema::String, Schema::Number]).preprocess(Ok),
        ] {
            schema.validate_params().unwrap();
        }
    }

    #[derive(schemars::JsonSchema, Deserialize, Debug, PartialEq)]
    struct TypedParams {
        z: i32,
        optional: Option<bool>,
        nested: Vec<TypedNested>,
        pair: [String; 2],
    }
    #[derive(schemars::JsonSchema, Deserialize, Debug, PartialEq)]
    struct TypedNested {
        flag: bool,
    }

    #[test]
    fn derives_structs_options_arrays_references_and_integer_normalization() {
        let schema = Schema::from_type::<TypedParams>().unwrap();
        let value = schema
            .parse_value(
                json!({"z":3.0,"nested":[{"flag":true,"ignored":1}],"pair":["a","b"],"ignored":1}),
            )
            .unwrap();
        assert_eq!(
            value.to_string(),
            "{\"z\":3,\"nested\":[{\"flag\":true}],\"pair\":[\"a\",\"b\"]}"
        );
        let typed: TypedParams = serde_json::from_value(value).unwrap();
        assert_eq!(
            typed,
            TypedParams {
                z: 3,
                optional: None,
                nested: vec![TypedNested { flag: true }],
                pair: ["a".into(), "b".into()]
            }
        );
        assert!(
            schema
                .parse_value(json!({"z":3,"nested":[],"pair":["a"]}))
                .unwrap_err()
                .issues
                .iter()
                .any(|i| i["code"] == "too_small")
        );
        assert!(
            schema
                .parse_value(json!({"z":3,"optional":"true","nested":[],"pair":["a","b"]}))
                .is_err()
        );
    }

    #[test]
    fn derived_integer_formats_enforce_bounds_and_fractional_errors() {
        let signed = Schema::from_type::<i32>().unwrap();
        assert_eq!(signed.parse_value(json!(2147483647)).unwrap(), 2147483647);
        assert_eq!(
            signed
                .parse_value(json!(2147483648_u64))
                .unwrap_err()
                .issues[0]["code"],
            "too_big"
        );
        assert_eq!(
            signed
                .parse_value(json!(-2147483649_i64))
                .unwrap_err()
                .issues[0]["code"],
            "too_small"
        );
        assert_eq!(
            signed.parse_value(json!(1.5)).unwrap_err().issues[0]["message"],
            "Invalid input: expected int, received number"
        );
        let unsigned = Schema::from_type::<u64>().unwrap();
        assert_eq!(
            unsigned.parse_value(json!(u64::MAX)).unwrap(),
            json!(u64::MAX)
        );
        assert_eq!(
            unsigned.parse_value(json!(-1)).unwrap_err().issues[0]["code"],
            "too_small"
        );
        assert_eq!(
            Schema::from_type::<i64>()
                .unwrap()
                .parse_value(json!(i64::MAX as u64 + 1))
                .unwrap_err()
                .issues[0]["code"],
            "too_big"
        );
    }

    #[derive(schemars::JsonSchema, Deserialize, Debug, PartialEq)]
    enum Choice {
        Plain,
        Count(i32),
        Named { name: String },
    }

    #[test]
    fn derives_unit_and_external_tagged_enums() {
        let schema = Schema::from_type::<Choice>().unwrap();
        for (input, expected) in [
            (json!("Plain"), Choice::Plain),
            (json!({"Count":2}), Choice::Count(2)),
            (
                json!({"Named":{"name":"a"}}),
                Choice::Named { name: "a".into() },
            ),
        ] {
            assert_eq!(
                serde_json::from_value::<Choice>(schema.parse_value(input).unwrap()).unwrap(),
                expected
            );
        }
        assert!(schema.parse_value(json!("missing")).is_err());
    }

    #[derive(schemars::JsonSchema, Deserialize, Debug, PartialEq)]
    #[serde(tag = "kind", rename_all = "camelCase")]
    enum Tagged {
        A { value: bool },
        B,
    }

    #[test]
    fn derives_internal_tagged_enums_and_root_options() {
        let schema = Schema::from_type::<Tagged>().unwrap();
        assert_eq!(
            serde_json::from_value::<Tagged>(
                schema
                    .parse_value(json!({"kind":"a","value":true}))
                    .unwrap()
            )
            .unwrap(),
            Tagged::A { value: true }
        );
        let option = Schema::from_type::<Option<Vec<String>>>().unwrap();
        assert_eq!(option.parse_value(Value::Null).unwrap(), Value::Null);
        assert_eq!(option.parse_value(json!(["a"])).unwrap(), json!(["a"]));
        assert!(option.parse_value(json!([1])).is_err());
    }

    #[derive(schemars::JsonSchema)]
    #[allow(dead_code)]
    struct Recursive {
        next: Option<Box<Recursive>>,
    }

    #[test]
    fn unsupported_typed_constructs_fail_explicitly() {
        assert!(
            Schema::from_type::<Recursive>()
                .unwrap_err()
                .0
                .contains("recursive reference")
        );
        assert!(
            Schema::from_type::<std::collections::HashMap<String, String>>()
                .unwrap_err()
                .0
                .contains("additionalProperties")
        );
        assert!(
            Schema::from_type::<(String, bool)>()
                .unwrap_err()
                .0
                .contains("prefixItems")
        );
        for value in [
            json!({"type":"string","pattern":"a"}),
            json!({"type":"string","format":"email"}),
            json!({"$ref":"https://example.com"}),
            json!({"type":"array","uniqueItems":true}),
        ] {
            assert!(
                compile_schema(&value, &value, &mut vec![], 0)
                    .unwrap_err()
                    .0
                    .starts_with("Unsupported generated JSON schema:")
            );
        }
    }

    #[test]
    fn generated_numeric_and_utf16_length_constraints_emit_zod_issues() {
        let number = json!({"type":"number","minimum":2});
        let schema = compile_schema(&number, &number, &mut vec![], 0).unwrap();
        assert_eq!(
            schema.parse_value(json!(1)).unwrap_err().issues,
            vec![
                json!({"origin":"number","code":"too_small","minimum":2,"inclusive":true,"path":[],"message":"Too small: expected number to be >=2"})
            ]
        );
        let string = json!({"type":"string","maxLength":1});
        let schema = compile_schema(&string, &string, &mut vec![], 0).unwrap();
        assert_eq!(
            schema.parse_value(json!("😀")).unwrap_err().issues[0]["message"],
            "Too big: expected string to have <=1 characters"
        );
    }
}
