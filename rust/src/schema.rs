use crate::DefinitionError;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    fmt,
    sync::Arc,
};

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
    File,
    Literal(Value),
    Enum(Vec<String>),
    Object(IndexMap<String, Schema>),
    /// Rejects undeclared keys, including generated `additionalProperties: false` rules.
    StrictObject(IndexMap<String, Schema>),
    Array(Box<Schema>),
    Record(Box<Schema>),
    Tuple(Vec<Schema>),
    #[serde(skip)]
    OpenObject(IndexMap<String, Schema>, Box<Schema>),
    #[serde(skip)]
    Lazy(GeneratedReference),
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

// References retain only immutable source metadata, never a graph of owning Schema references.
#[derive(Clone, Debug)]
pub struct GeneratedReference {
    root: Arc<Value>,
    reference: String,
}

impl GeneratedReference {
    fn key(&self) -> (usize, String) {
        (Arc::as_ptr(&self.root) as usize, self.reference.clone())
    }

    pub(crate) fn resolve(&self) -> Schema {
        let target = self
            .root
            .pointer(&self.reference[1..])
            .expect("generated reference was checked during schema construction");
        compile_schema(target, &self.root, &mut None, 0)
            .expect("generated schema was checked during schema construction")
    }
}

impl fmt::Debug for Schema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lazy(reference) => f.debug_tuple("Lazy").field(&reference.reference).finish(),
            Self::OpenObject(shape, rest) => f
                .debug_tuple("OpenObject")
                .field(shape)
                .field(rest)
                .finish(),
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
    Blob,
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
            Self::Promise | Self::Blob => Some(json!({})),
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
            Self::Blob => "Blob",
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

/// Per-parse safety bounds. Every schema visit counts, including wrappers and union
/// attempts, so large flat JSON arrays and records also consume `max_nodes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidationLimits {
    pub max_depth: usize,
    pub max_nodes: usize,
}

impl Default for ValidationLimits {
    fn default() -> Self {
        Self {
            max_depth: 128,
            max_nodes: 65_536,
        }
    }
}

struct ValidationBudget {
    limits: ValidationLimits,
    remaining: Cell<usize>,
    failure: RefCell<Option<ValidationError>>,
}

// Continuation belongs to the parsing operation, independently of public issue codes.
struct ParseFailure {
    issues: Vec<Value>,
    aborted: bool,
}

impl ParseFailure {
    fn continuable(error: ValidationError) -> Self {
        Self {
            issues: error.issues,
            aborted: false,
        }
    }

    fn at(self, path: &[Value]) -> Self {
        Self {
            issues: ValidationError {
                issues: self.issues,
            }
            .at(path)
            .issues,
            aborted: self.aborted,
        }
    }

    fn into_validation_error(self) -> ValidationError {
        ValidationError {
            issues: self.issues,
        }
    }
}

impl From<ValidationError> for ParseFailure {
    fn from(error: ValidationError) -> Self {
        Self {
            issues: error.issues,
            aborted: true,
        }
    }
}

impl ValidationBudget {
    fn check(&self) -> Result<(), ParseFailure> {
        match self.failure.borrow().as_ref() {
            Some(error) => Err(error.clone().into()),
            None => Ok(()),
        }
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
        self.parse_with_limits(value, ValidationLimits::default())
    }

    pub fn parse_with_limits(
        &self,
        value: Option<Value>,
        limits: ValidationLimits,
    ) -> Result<Option<Value>, ValidationError> {
        self.parse_input_with_limits(
            &value.map(Input::from_value).unwrap_or(Input::Absent),
            limits,
        )
    }

    pub fn parse_value(&self, value: Value) -> Result<Value, ValidationError> {
        self.parse_value_with_limits(value, ValidationLimits::default())
    }

    pub fn parse_value_with_limits(
        &self,
        value: Value,
        limits: ValidationLimits,
    ) -> Result<Value, ValidationError> {
        self.parse_with_limits(Some(value), limits)?
            .ok_or_else(|| ValidationError::custom("Schema returned undefined for a JSON value"))
    }

    pub(crate) fn parse_input(&self, input: &Input) -> Result<Option<Value>, ValidationError> {
        self.parse_input_with_limits(input, ValidationLimits::default())
    }

    fn parse_input_with_limits(
        &self,
        input: &Input,
        limits: ValidationLimits,
    ) -> Result<Option<Value>, ValidationError> {
        let budget = ValidationBudget {
            remaining: Cell::new(limits.max_nodes),
            limits,
            failure: RefCell::new(None),
        };
        let result = self
            .parse_at(input, &[], 0, &budget)
            .map(|parsed| parsed.value)
            .map_err(ParseFailure::into_validation_error);
        match budget.failure.into_inner() {
            Some(error) => Err(error),
            None => result,
        }
    }

    fn parse_at(
        &self,
        input: &Input,
        path: &[Value],
        depth: usize,
        budget: &ValidationBudget,
    ) -> Result<Parsed, ParseFailure> {
        budget.check()?;
        let limit_error = if depth > budget.limits.max_depth {
            Some(format!(
                "Schema nesting exceeds {} levels",
                budget.limits.max_depth
            ))
        } else if budget.remaining.get() == 0 {
            Some(format!(
                "Schema validation exceeds {} nodes",
                budget.limits.max_nodes
            ))
        } else {
            None
        };
        if let Some(message) = limit_error {
            let error = ValidationError::custom(message).at(path);
            *budget.failure.borrow_mut() = Some(error.clone());
            return Err(error.into());
        }
        budget.remaining.set(budget.remaining.get() - 1);
        let inner = |schema: &Schema| schema.parse_at(input, path, depth + 1, budget);
        match self {
            Self::Lazy(reference) => inner(&reference.resolve()),
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
            Self::File if matches!(input, Input::Blob) => Ok(Parsed::new(Some(json!({})))),
            Self::File => Err(invalid_type("file", input, path).into()),
            Self::String => Err(invalid_type("string", input, path).into()),
            Self::Number => Err(invalid_type("number", input, path).into()),
            Self::Integer { signed, bits } => parse_integer(input, *signed, *bits)
                .map(|v| Parsed::new(Some(v)))
                .map_err(|e| e.at(path)),
            Self::Boolean => Err(invalid_type("boolean", input, path).into()),
            Self::Literal(expected) => {
                if !matches!(
                    input,
                    Input::NonFinite { .. } | Input::Promise | Input::Blob
                ) && input
                    .to_value()
                    .as_ref()
                    .is_some_and(|actual| match (actual, expected) {
                        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
                        _ => actual == expected,
                    })
                {
                    Ok(Parsed::new(input.to_value()))
                } else {
                    Err(invalid_values(vec![expected.clone()], path).into())
                }
            }
            Self::Enum(options) => {
                if matches!(input, Input::Scalar(Value::String(s)) if options.contains(s)) {
                    Ok(Parsed::new(input.to_value()))
                } else {
                    Err(
                        invalid_values(options.iter().cloned().map(Value::String).collect(), path)
                            .into(),
                    )
                }
            }
            Self::Object(_) | Self::StrictObject(_) | Self::OpenObject(_, _) => {
                self.parse_object(input, path, depth, budget)
            }
            Self::Record(_) => self.parse_record(input, path, depth, budget),
            Self::Tuple(_) => self.parse_tuple(input, path, depth, budget),
            Self::Array(_) => self.parse_array(input, path, depth, budget),
            Self::Union(_) => self.parse_union(input, path, depth, budget),
            Self::Optional(schema) if matches!(input, Input::Absent) => {
                if !schema.optional_input() {
                    return Ok(Parsed::new(None));
                }
                let result = inner(schema);
                budget.check()?;
                match result {
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
            Self::Catch(schema, fallback) => {
                let result = inner(schema);
                budget.check()?;
                Ok(result.unwrap_or_else(|_| Parsed {
                    value: Some(fallback.clone()),
                    fallback: true,
                }))
            }
            Self::Refinement(schema, check) => {
                let parsed = inner(schema)?;
                check(parsed.value.as_ref()).map_err(|e| ParseFailure::continuable(e.at(path)))?;
                Ok(parsed)
            }
            Self::Transform(schema, transform) => {
                let parsed = inner(schema)?;
                Ok(Parsed {
                    value: transform(parsed.value)
                        .map_err(|e| ParseFailure::continuable(e.at(path)))?,
                    fallback: true,
                })
            }
            Self::Preprocess(schema, preprocess) => {
                let value = preprocess(input.to_value())
                    .map_err(|e| ParseFailure::continuable(e.at(path)))?;
                let mut parsed = schema.parse_at(
                    &value.map(Input::from_value).unwrap_or(Input::Absent),
                    path,
                    depth + 1,
                    budget,
                )?;
                parsed.fallback = true;
                Ok(parsed)
            }
            Self::Optional(schema) | Self::Nullable(schema) | Self::Readonly(schema) => {
                inner(schema)
            }
        }
    }

    fn parse_object(
        &self,
        input: &Input,
        path: &[Value],
        depth: usize,
        budget: &ValidationBudget,
    ) -> Result<Parsed, ParseFailure> {
        let (Self::Object(shape) | Self::StrictObject(shape) | Self::OpenObject(shape, _)) = self
        else {
            unreachable!()
        };

        let empty = IndexMap::new();
        let fields = match input {
            Input::Object(fields) => fields,
            Input::Promise | Input::Blob => &empty,
            _ => return Err(invalid_type("object", input, path).into()),
        };
        let mut output = serde_json::Map::new();
        let mut issues = Vec::new();
        let mut aborted = false;
        for (key, schema) in shape {
            let mut child_path = path.to_vec();
            child_path.push(Value::String(key.clone()));
            let present = fields.contains_key(key);
            let result = schema.parse_at(
                fields.get(key).unwrap_or(&Input::Absent),
                &child_path,
                depth + 1,
                budget,
            );
            budget.check()?;
            match result {
                Ok(_) if !present && !schema.optional_input() => {
                    issues.push(json!({"code":"invalid_type", "expected":"nonoptional", "path":child_path, "message":"Invalid input: expected nonoptional, received undefined"}));
                    aborted = true;
                }
                Ok(Parsed {
                    value: Some(value), ..
                }) => {
                    output.insert(key.clone(), value);
                }
                Ok(Parsed { value: None, .. }) => {}
                Err(_) if !present && schema.optional_input() && schema.optional_output() => {}
                Err(error) => {
                    aborted |= error.aborted;
                    issues.extend(error.issues);
                }
            }
        }
        if let Self::OpenObject(_, rest) = self {
            for (key, value) in fields.iter().filter(|(key, _)| !shape.contains_key(*key)) {
                let mut child_path = path.to_vec();
                child_path.push(json!(key));
                let result = rest.parse_at(value, &child_path, depth + 1, budget);
                budget.check()?;
                match result {
                    Ok(Parsed {
                        value: Some(value), ..
                    }) => {
                        output.insert(key.clone(), value);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        aborted |= error.aborted;
                        issues.extend(error.issues);
                    }
                }
            }
        }
        if matches!(self, Self::StrictObject(_)) {
            let keys = fields
                .keys()
                .filter(|key| !shape.contains_key(*key))
                .collect::<Vec<_>>();
            if !keys.is_empty() {
                let names = keys
                    .iter()
                    .map(|key| format!("\"{key}\""))
                    .collect::<Vec<_>>()
                    .join(", ");
                issues.push(json!({
                    "code":"unrecognized_keys", "keys":keys, "path":path,
                    "message":format!("Unrecognized key{}: {names}", if keys.len() == 1 { "" } else { "s" })
                }));
                aborted = true;
            }
        }
        if issues.is_empty() {
            Ok(Parsed::new(Some(Value::Object(output))))
        } else {
            Err(ParseFailure { issues, aborted })
        }
    }

    fn parse_record(
        &self,
        input: &Input,
        path: &[Value],
        depth: usize,
        budget: &ValidationBudget,
    ) -> Result<Parsed, ParseFailure> {
        let Self::Record(element) = self else {
            unreachable!()
        };

        let Input::Object(fields) = input else {
            return Err(invalid_type("record", input, path).into());
        };
        let mut output = serde_json::Map::new();
        let mut issues = Vec::new();
        let mut aborted = false;
        for (key, item) in fields {
            let mut child_path = path.to_vec();
            child_path.push(json!(key));
            let result = element.parse_at(item, &child_path, depth + 1, budget);
            budget.check()?;
            match result {
                Ok(Parsed {
                    value: Some(value), ..
                }) => {
                    output.insert(key.clone(), value);
                }
                Ok(_) => {}
                Err(error) => {
                    aborted |= error.aborted;
                    issues.extend(error.issues);
                }
            }
        }
        if issues.is_empty() {
            Ok(Parsed::new(Some(Value::Object(output))))
        } else {
            Err(ParseFailure { issues, aborted })
        }
    }

    fn parse_tuple(
        &self,
        input: &Input,
        path: &[Value],
        depth: usize,
        budget: &ValidationBudget,
    ) -> Result<Parsed, ParseFailure> {
        let Self::Tuple(elements) = self else {
            unreachable!()
        };

        let Input::Array(items) = input else {
            return Err(invalid_type("tuple", input, path).into());
        };
        let minimum = elements
            .iter()
            .rposition(|element| !element.optional_input())
            .map_or(0, |index| index + 1);
        let mut issues = Vec::new();
        let mut aborted = false;
        if items.len() < minimum || items.len() > elements.len() {
            let lower = items.len() < minimum;
            let bound = if lower { minimum } else { elements.len() };
            let mut issue = serde_json::Map::new();
            issue.insert(
                "code".into(),
                json!(if lower { "too_small" } else { "too_big" }),
            );
            issue.insert(
                if lower { "minimum" } else { "maximum" }.into(),
                json!(bound),
            );
            issue.insert("inclusive".into(), json!(true));
            issue.insert("origin".into(), json!("array"));
            issue.insert("path".into(), json!(path));
            issue.insert(
                "message".into(),
                json!(format!(
                    "{}: expected array to have {}{} items",
                    if lower { "Too small" } else { "Too big" },
                    if lower { ">=" } else { "<=" },
                    bound
                )),
            );
            issues.push(Value::Object(issue));
            aborted = true;
            if lower {
                return Err(ParseFailure { issues, aborted });
            }
        }
        let mut output = Vec::with_capacity(elements.len().min(budget.remaining.get()));
        for (index, element) in elements.iter().enumerate() {
            let item = items.get(index).unwrap_or(&Input::Absent);
            let mut child_path = path.to_vec();
            child_path.push(json!(index));
            let result = element.parse_at(item, &child_path, depth + 1, budget);
            budget.check()?;
            match result {
                Ok(Parsed {
                    value: Some(value), ..
                }) => {
                    output.resize(index, Value::Null);
                    output.push(value);
                }
                Ok(_) if index < items.len() => output.push(Value::Null),
                Ok(_) => {}
                Err(error) => {
                    aborted |= error.aborted;
                    issues.extend(error.issues);
                }
            }
        }
        if issues.is_empty() {
            Ok(Parsed::new(Some(Value::Array(output))))
        } else {
            Err(ParseFailure { issues, aborted })
        }
    }

    fn parse_array(
        &self,
        input: &Input,
        path: &[Value],
        depth: usize,
        budget: &ValidationBudget,
    ) -> Result<Parsed, ParseFailure> {
        let Self::Array(element) = self else {
            unreachable!()
        };

        let Input::Array(items) = input else {
            return Err(invalid_type("array", input, path).into());
        };
        let mut output = Vec::with_capacity(items.len().min(budget.remaining.get()));
        let mut issues = Vec::new();
        let mut aborted = false;
        for (index, item) in items.iter().enumerate() {
            let mut child_path = path.to_vec();
            child_path.push(json!(index));
            let result = element.parse_at(item, &child_path, depth + 1, budget);
            budget.check()?;
            match result {
                Ok(parsed) => output.push(parsed.value.unwrap_or(Value::Null)),
                Err(error) => {
                    aborted |= error.aborted;
                    issues.extend(error.issues);
                }
            }
        }
        if issues.is_empty() {
            Ok(Parsed::new(Some(Value::Array(output))))
        } else {
            Err(ParseFailure { issues, aborted })
        }
    }

    fn parse_union(
        &self,
        input: &Input,
        path: &[Value],
        depth: usize,
        budget: &ValidationBudget,
    ) -> Result<Parsed, ParseFailure> {
        let Self::Union(branches) = self else {
            unreachable!()
        };

        let mut errors = Vec::new();
        for branch in branches {
            let result = branch.parse_at(input, &[], depth + 1, budget);
            budget.check()?;
            match result {
                Ok(value) => return Ok(value),
                Err(error) => errors.push(error),
            }
        }
        let mut continuable = errors.iter().filter(|error| !error.aborted);
        if let Some(error) = continuable.next() {
            if continuable.next().is_none() {
                return Err(ParseFailure {
                    issues: error.issues.clone(),
                    aborted: false,
                }
                .at(path));
            }
        }
        Err(ParseFailure {
            aborted: true,
            issues: vec![json!({
                "code": "invalid_union", "errors": errors.into_iter().map(|e| e.issues).collect::<Vec<_>>(),
                "path": path, "message": "Invalid input"
            })],
        })
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
            Self::Optional(_) => true,
            Self::Catch(inner, _)
            | Self::Nullable(inner)
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
        self.validate_params_at("", 0, &mut HashSet::new())
    }

    fn validate_params_at(
        &self,
        path: &str,
        depth: usize,
        references: &mut HashSet<(usize, String)>,
    ) -> Result<(), DefinitionError> {
        if depth > 128 {
            return Err(DefinitionError("Schema nesting exceeds 128 levels".into()));
        }
        match self.unwrapped() {
            Self::Lazy(reference) => {
                if references.insert(reference.key()) {
                    reference
                        .resolve()
                        .validate_params_at(path, depth + 1, references)?;
                }
            }
            Self::Union(branches) => {
                let mut origins = Vec::new();
                for branch in branches {
                    if branch.string_origin(&mut HashSet::new(), 0)? {
                        origins.push(branch);
                    }
                }
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
                    branch.validate_params_at(path, depth + 1, references)?;
                }
            }
            Self::Object(shape) | Self::StrictObject(shape) | Self::OpenObject(shape, _) => {
                for (key, schema) in shape {
                    schema.validate_params_at(
                        &if path.is_empty() {
                            key.clone()
                        } else {
                            format!("{path}.{key}")
                        },
                        depth + 1,
                        references,
                    )?;
                }
            }
            Self::Array(element) | Self::Record(element) => {
                element.validate_params_at(&format!("{path}[]"), depth + 1, references)?
            }
            Self::Tuple(elements) => {
                for (index, element) in elements.iter().enumerate() {
                    element.validate_params_at(
                        &format!("{path}.{index}"),
                        depth + 1,
                        references,
                    )?;
                }
            }
            _ => {}
        }
        if let Self::OpenObject(_, rest) = self.unwrapped() {
            rest.validate_params_at(&format!("{path}[]"), depth + 1, references)?;
        }
        Ok(())
    }

    fn string_origin(
        &self,
        references: &mut HashSet<(usize, String)>,
        depth: usize,
    ) -> Result<bool, DefinitionError> {
        if depth > 128 {
            return Err(DefinitionError("Schema nesting exceeds 128 levels".into()));
        }
        Ok(match self.unwrapped() {
            Self::Lazy(reference) if references.insert(reference.key()) => {
                reference.resolve().string_origin(references, depth + 1)?
            }
            Self::String
            | Self::Number
            | Self::Integer { .. }
            | Self::Boolean
            | Self::Enum(_)
            | Self::Literal(Value::String(_)) => true,
            _ => false,
        })
    }

    fn zod_name(&self) -> &'static str {
        match self {
            Self::Lazy(reference) => reference.resolve().zod_name(),
            Self::String => "ZodString",
            Self::Number | Self::Integer { .. } => "ZodNumber",
            Self::Boolean => "ZodBoolean",
            Self::File => "ZodFile",
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

    /// Compiles Schemars structs, string-key maps, fixed tuples and recursive finite data.
    /// Unsupported formats and regex constraints return errors. Use an explicit runtime
    /// schema with `refine` or `preprocess` for custom Rust validation instead.
    pub fn from_type<T: schemars::JsonSchema>() -> Result<Self, DefinitionError> {
        let generated = Arc::new(schemars::schema_for!(T).to_value());
        compile_schema(&generated, &generated, &mut Some(HashSet::new()), 0)
    }
}

fn parse_integer(input: &Input, signed: bool, bits: u8) -> Result<Value, ParseFailure> {
    if !matches!(bits, 8 | 16 | 32 | 64) {
        return Err(ValidationError::custom("Integer width must be 8, 16, 32 or 64 bits").into());
    }
    let Input::Scalar(Value::Number(number)) = input else {
        return Err(invalid_type("number", input, &[]).into());
    };
    let value = number
        .as_f64()
        .ok_or_else(|| invalid_type("number", input, &[]))?;
    if value.fract() != 0.0 {
        return Err(ValidationError {
            issues: vec![
                json!({"expected":"int", "format":"safeint", "code":"invalid_type", "path":[], "message":"Invalid input: expected int, received number"}),
            ],
        }.into());
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
        return Err(ParseFailure::continuable(ValidationError {
            issues: vec![Value::Object(issue)],
        }));
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
    root: &Arc<Value>,
    references: &mut Option<HashSet<String>>,
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
                | "prefixItems"
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
        let pointer = reference
            .strip_prefix('#')
            .ok_or_else(|| unsupported("external reference"))?;
        let target = root
            .pointer(pointer)
            .ok_or_else(|| unsupported(format!("unresolved reference {reference}")))?;
        if references
            .as_mut()
            .is_some_and(|seen| seen.insert(reference.into()))
        {
            compile_schema(target, root, references, depth + 1)?;
        }
        return Ok(Schema::Lazy(GeneratedReference {
            root: Arc::clone(root),
            reference: reference.into(),
        }));
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
            Some("array") => {
                if let Some(prefix) = object.get("prefixItems") {
                    let elements = prefix
                        .as_array()
                        .ok_or_else(|| unsupported("non-array prefixItems"))?;
                    if object.get("minItems").and_then(Value::as_u64) != Some(elements.len() as u64)
                        || object.get("maxItems").and_then(Value::as_u64)
                            != Some(elements.len() as u64)
                        || object
                            .get("items")
                            .is_some_and(|v| v != &Value::Bool(false))
                    {
                        return Err(unsupported("variable-length tuple"));
                    }
                    Schema::Tuple(
                        elements
                            .iter()
                            .map(|v| compile_schema(v, root, references, depth + 1))
                            .collect::<Result<_, _>>()?,
                    )
                } else {
                    Schema::Array(Box::new(compile_schema(
                        object.get("items").unwrap_or(&Value::Bool(true)),
                        root,
                        references,
                        depth + 1,
                    )?))
                }
            }
            Some("object") => {
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
                match object.get("additionalProperties") {
                    Some(Value::Bool(false)) => Schema::StrictObject(fields),
                    None => Schema::Object(fields),
                    Some(value) => {
                        let element = Box::new(compile_schema(value, root, references, depth + 1)?);
                        if fields.is_empty() {
                            Schema::Record(element)
                        } else {
                            Schema::OpenObject(fields, element)
                        }
                    }
                }
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
        if object.contains_key("prefixItems") && matches!(key, "minItems" | "maxItems") {
            continue;
        }
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
    fn hard_exhaustion_stops_collections_and_union_attempts() {
        let count = 8_192;
        let array = Input::Array(vec![Input::Scalar(json!(0)); count]);
        let fields = (0..count)
            .map(|index| (index.to_string(), Schema::Number))
            .collect::<IndexMap<_, _>>();
        let object = Input::Object(
            fields
                .keys()
                .map(|key| (key.clone(), Input::Scalar(json!(0))))
                .collect(),
        );
        for (schema, input) in [
            (Schema::Array(Box::new(Schema::Number)), &array),
            (Schema::Tuple(vec![Schema::Number; count]), &array),
            (Schema::Object(fields.clone()), &object),
            (
                Schema::OpenObject(IndexMap::new(), Box::new(Schema::Number)),
                &object,
            ),
            (Schema::Record(Box::new(Schema::Number)), &object),
            (Schema::Union(vec![Schema::String; count]), &array),
        ] {
            let budget = ValidationBudget {
                limits: ValidationLimits {
                    max_depth: 128,
                    max_nodes: 2,
                },
                remaining: Cell::new(2),
                failure: RefCell::new(None),
            };
            let error = schema.parse_at(input, &[], 0, &budget).err().unwrap();
            let expected = budget.failure.into_inner().unwrap();
            assert_eq!(error.issues.len(), 1, "{schema:?}");
            assert_eq!(error.issues, expected.issues);
        }
    }

    #[test]
    fn hard_exhaustion_never_reaches_callbacks_after_catch_or_optional() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for inner in [
            Schema::Catch(Box::new(Schema::Number), json!(0)),
            Schema::Optional(Box::new(Schema::Default(
                Box::new(Schema::Number),
                json!(0),
            ))),
        ] {
            let calls = Arc::new(AtomicUsize::new(0));
            let observed = calls.clone();
            let schema = inner.refine(move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });
            let error = schema
                .parse_with_limits(
                    None,
                    ValidationLimits {
                        max_depth: 128,
                        max_nodes: 2,
                    },
                )
                .unwrap_err();
            assert_eq!(
                error.issues[0]["message"],
                "Schema validation exceeds 2 nodes"
            );
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn catch_inherits_inner_output_optionality_for_absent_object_fields() {
        let required = object([(
            "value",
            Schema::Catch(Box::new(Schema::Number), json!(0))
                .refine(|_| Err(ValidationError::custom("Rejected"))),
        )]);
        assert_eq!(
            required.parse_value(json!({})).unwrap_err().issues,
            vec![json!({"code":"custom","path":["value"],"message":"Rejected"})]
        );
        let optional = object([(
            "value",
            Schema::Catch(
                Box::new(Schema::Optional(Box::new(Schema::Number))),
                json!(0),
            )
            .refine(|_| Err(ValidationError::custom("Rejected"))),
        )]);
        assert_eq!(optional.parse_value(json!({})).unwrap(), json!({}));
        assert_eq!(
            optional.parse_value(json!({"value":1})).unwrap_err().issues,
            vec![json!({"code":"custom","path":["value"],"message":"Rejected"})]
        );
    }

    #[derive(schemars::JsonSchema, Serialize, Deserialize, Debug, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct StrictBody {
        value: String,
    }

    #[test]
    fn generated_strict_objects_reject_unknown_fields_before_typed_decoding() {
        let strict = Schema::from_type::<StrictBody>().unwrap();
        assert_eq!(
            typed_roundtrip::<StrictBody>(json!({"value":"ok"})),
            json!({"value":"ok"})
        );
        assert_eq!(
            strict
                .parse_value(json!({"value":"ok","unexpected":true,"extra":0}))
                .unwrap_err()
                .issues,
            vec![
                json!({"code":"unrecognized_keys","keys":["unexpected","extra"],"path":[],
                "message":"Unrecognized keys: \"unexpected\", \"extra\""})
            ]
        );
        assert_eq!(
            object([("value", Schema::String)])
                .parse_value(json!({"value":"ok","unexpected":true}))
                .unwrap(),
            json!({"value":"ok"})
        );
        assert_eq!(
            Schema::from_type::<TypedNested>()
                .unwrap()
                .parse_value(json!({"flag":true,"unexpected":true}))
                .unwrap(),
            json!({"flag":true})
        );
        let nested = Schema::Array(Box::new(strict));
        assert_eq!(
            nested
                .parse_value(json!([{"value":"ok","extra":0}]))
                .unwrap_err()
                .issues[0]["path"],
            json!([0])
        );
    }

    #[test]
    fn tuple_arity_failures_abort_union_branches() {
        let schema = Schema::Union(vec![Schema::Tuple(vec![Schema::String]), Schema::String]);
        for (input, code, bound, message) in [
            (
                json!([]),
                "too_small",
                "minimum",
                "Too small: expected array to have >=1 items",
            ),
            (
                json!(["ok", "extra"]),
                "too_big",
                "maximum",
                "Too big: expected array to have <=1 items",
            ),
        ] {
            assert_eq!(
                schema.parse_value(input).unwrap_err().issues,
                vec![json!({"code":"invalid_union","errors":[[
                    {"code":code,bound:1,"inclusive":true,"origin":"array","path":[],"message":message}
                ],[{
                    "expected":"string","code":"invalid_type","path":[],
                    "message":"Invalid input: expected string, received array"
                }]],"path":[],"message":"Invalid input"})]
            );
        }
        let nested = Schema::Union(vec![
            object([("pair", Schema::Tuple(vec![Schema::String]))]),
            Schema::String,
        ]);
        assert_eq!(
            nested.parse_value(json!({"pair":[]})).unwrap_err().issues[0]["code"],
            "invalid_union"
        );
    }

    #[test]
    fn oversized_tuples_report_length_then_declared_element_errors() {
        let schema = Schema::Tuple(vec![
            Schema::Array(Box::new(Schema::Tuple(vec![
                Schema::Boolean,
                Schema::String,
            ]))),
            Schema::Nullable(Box::new(Schema::Number)),
        ]);
        let error = schema.parse_value(json!(["a", false, 2])).unwrap_err();
        assert_eq!(
            error.issues,
            vec![
                json!({"code":"too_big","maximum":2,"inclusive":true,"origin":"array","path":[],"message":"Too big: expected array to have <=2 items"}),
                json!({"expected":"array","code":"invalid_type","path":[0],"message":"Invalid input: expected array, received string"}),
                json!({"expected":"number","code":"invalid_type","path":[1],"message":"Invalid input: expected number, received boolean"}),
            ]
        );
    }

    #[test]
    fn tuples_preserve_trailing_optional_absence_and_apply_defaults() {
        let optional = Schema::Tuple(vec![Schema::Optional(Box::new(Schema::String))]);
        assert_eq!(optional.parse_value(json!([])).unwrap(), json!([]));
        let defaults = Schema::Tuple(vec![
            Schema::Optional(Box::new(Schema::String)),
            Schema::Default(Box::new(Schema::Number), json!(2)),
        ]);
        assert_eq!(defaults.parse_value(json!([])).unwrap(), json!([null, 2]));
        let required = Schema::Tuple(vec![
            Schema::String,
            Schema::Optional(Box::new(Schema::Number)),
        ]);
        assert_eq!(
            required.parse_value(json!([])).unwrap_err().issues,
            vec![
                json!({"code":"too_small","minimum":1,"inclusive":true,"origin":"array","path":[],"message":"Too small: expected array to have >=1 items"})
            ]
        );
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
    fn uploaded_files_keep_blob_validation_identity() {
        assert_eq!(
            Schema::File.parse_input(&Input::Blob).unwrap(),
            Some(json!({}))
        );
        assert_eq!(
            Schema::String.parse_input(&Input::Blob).unwrap_err().issues[0]["message"],
            "Invalid input: expected string, received Blob"
        );
        assert_eq!(
            Schema::Object(IndexMap::new())
                .parse_input(&Input::Blob)
                .unwrap(),
            Some(json!({}))
        );
        assert_eq!(
            Schema::File.parse_value(json!({})).unwrap_err().issues[0]["message"],
            "Invalid input: expected file, received object"
        );
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

    fn typed_roundtrip<T>(input: Value) -> Value
    where
        T: schemars::JsonSchema + serde::de::DeserializeOwned + Serialize,
    {
        let schema = Schema::from_type::<T>().unwrap();
        let parsed = schema.parse_value(input).unwrap();
        let typed: T = serde_json::from_value(parsed.clone()).unwrap();
        assert_eq!(serde_json::to_value(typed).unwrap(), parsed);
        parsed
    }

    #[derive(schemars::JsonSchema, Serialize, Deserialize)]
    struct MapValue {
        count: i32,
        pairs: Vec<(String, bool)>,
    }

    #[test]
    fn derived_string_key_maps_retain_keys_and_validate_nested_values() {
        type Map = std::collections::BTreeMap<String, MapValue>;
        let expected =
            json!({"first":{"count":3,"pairs":[["a",true]]},"second":{"count":0,"pairs":[]}});
        assert_eq!(
            typed_roundtrip::<Map>(json!({
                "first":{"count":3.0,"pairs":[["a",true]],"stripped":1},
                "second":{"count":0,"pairs":[]}
            })),
            expected
        );
        assert_eq!(
            typed_roundtrip::<std::collections::HashMap<String, String>>(json!({})),
            json!({})
        );
        let schema = Schema::from_type::<Map>().unwrap();
        let error = schema
            .parse_value(json!({"first":{"count":"3","pairs":[]}}))
            .unwrap_err();
        assert_eq!(error.issues[0]["path"], json!(["first", "count"]));
        assert_eq!(error.issues[0]["expected"], "number");
        let record = Schema::Record(Box::new(Schema::Number));
        assert_eq!(
            record.parse_value(json!([1])).unwrap_err().issues,
            vec![json!({
                "expected":"record","code":"invalid_type","path":[],
                "message":"Invalid input: expected record, received array"
            })]
        );
    }

    #[derive(schemars::JsonSchema, Serialize, Deserialize)]
    struct OpenFields {
        known: String,
        #[serde(flatten)]
        extra: std::collections::BTreeMap<String, i32>,
    }

    #[test]
    fn derived_open_objects_preserve_and_validate_catchall_fields() {
        assert_eq!(
            typed_roundtrip::<OpenFields>(json!({"known":"a","x":2.0,"y":3})),
            json!({"known":"a","x":2,"y":3})
        );
        let schema = Schema::from_type::<OpenFields>().unwrap();
        assert_eq!(
            schema
                .parse_value(json!({"known":"a","x":"2"}))
                .unwrap_err()
                .issues[0]["path"],
            json!(["x"])
        );
        assert_eq!(
            schema.parse_value(json!({"x":2})).unwrap_err().issues[0]["path"],
            json!(["known"])
        );
        assert_eq!(
            object([("known", Schema::String)])
                .parse_value(json!({"known":"a","x":2}))
                .unwrap(),
            json!({"known":"a"})
        );
        assert_eq!(
            typed_roundtrip::<std::collections::BTreeMap<String, Value>>(
                json!({"x":{"a":[1,true,null]}})
            ),
            json!({"x":{"a":[1,true,null]}})
        );
    }

    #[test]
    fn derived_heterogeneous_tuples_roundtrip_and_emit_exact_zod_errors() {
        assert_eq!(
            typed_roundtrip::<(String, bool, i32)>(json!(["a", false, 2.0])),
            json!(["a", false, 2])
        );
        assert_eq!(
            typed_roundtrip::<(Vec<(bool, String)>, Option<i32>)>(json!([[[true, "a"]], null])),
            json!([[[true, "a"]], null])
        );
        let schema = Schema::from_type::<(String, bool)>().unwrap();
        for input in [json!([]), json!(["a"])] {
            assert_eq!(
                schema.parse_value(input).unwrap_err().issues,
                vec![json!({
                    "code":"too_small","minimum":2,"inclusive":true,"origin":"array","path":[],
                    "message":"Too small: expected array to have >=2 items"
                })]
            );
        }
        assert_eq!(
            schema
                .parse_value(json!(["a", true, 0]))
                .unwrap_err()
                .issues,
            vec![json!({
                "code":"too_big","maximum":2,"inclusive":true,"origin":"array","path":[],
                "message":"Too big: expected array to have <=2 items"
            })]
        );
        assert_eq!(
            schema.parse_value(json!(["a", "true"])).unwrap_err().issues,
            vec![json!({
                "expected":"boolean","code":"invalid_type","path":[1],
                "message":"Invalid input: expected boolean, received string"
            })]
        );
        assert_eq!(
            schema.parse_value(json!({})).unwrap_err().issues[0]["expected"],
            "tuple"
        );
        assert_eq!(typed_roundtrip::<(String,)>(json!(["a"])), json!(["a"]));
    }

    #[derive(schemars::JsonSchema, Serialize, Deserialize)]
    struct Tree {
        name: String,
        children: Vec<Tree>,
        next: Option<Box<Tree>>,
    }

    #[test]
    fn recursive_box_vec_structs_validate_and_strip_at_every_level() {
        let input = json!({"name":"root","children":[{"name":"child","children":[],"next":null,"extra":1}],
            "next":{"name":"next","children":[],"next":null,"extra":2},"extra":3});
        let expected = json!({"name":"root","children":[{"name":"child","children":[],"next":null}],
            "next":{"name":"next","children":[],"next":null}});
        assert_eq!(typed_roundtrip::<Tree>(input), expected);
        let schema = Schema::from_type::<Tree>().unwrap();
        schema.validate_params().unwrap();
        let error = schema
            .parse_value(json!({"name":"root","children":[{"name":2,"children":[]}]}))
            .unwrap_err();
        assert_eq!(error.issues[0]["path"], json!(["children", 0, "name"]));
        assert_eq!(error.issues[0]["expected"], "string");
        assert_eq!(
            Schema::from_type::<Recursive>()
                .unwrap()
                .parse_value(json!({"next":{"next":null}}))
                .unwrap(),
            json!({"next":{"next":null}})
        );
    }

    #[derive(schemars::JsonSchema, Serialize, Deserialize)]
    enum Expression {
        Number(i32),
        Pair(Box<Expression>, Box<Expression>),
        List(Vec<Expression>),
    }

    #[derive(schemars::JsonSchema, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "camelCase")]
    enum TaggedTree {
        Leaf { value: i32 },
        Branch { children: Vec<TaggedTree> },
    }

    #[test]
    fn recursive_enums_and_tuple_variants_validate_finite_data() {
        let expression = json!({"Pair":[{"Number":3.0},{"List":[{"Number":4}]}]});
        assert_eq!(
            typed_roundtrip::<Expression>(expression),
            json!({"Pair":[{"Number":3},{"List":[{"Number":4}]}]})
        );
        let tagged = json!({"kind":"branch","children":[{"kind":"leaf","value":2}]});
        assert_eq!(typed_roundtrip::<TaggedTree>(tagged.clone()), tagged);
        Schema::from_type::<Expression>()
            .unwrap()
            .validate_params()
            .unwrap();
        Schema::from_type::<TaggedTree>()
            .unwrap()
            .validate_params()
            .unwrap();
        assert!(
            Schema::from_type::<Expression>()
                .unwrap()
                .parse_value(json!({"List":[{"Number":"3"}]}))
                .is_err()
        );
    }

    #[derive(schemars::JsonSchema, Serialize, Deserialize)]
    struct MutualA {
        b: Option<Box<MutualB>>,
    }
    #[derive(schemars::JsonSchema, Serialize, Deserialize)]
    struct MutualB {
        a: Vec<MutualA>,
        count: i32,
    }

    #[test]
    fn mutually_recursive_definitions_validate_and_terminate() {
        let input = json!({"b":{"a":[{"b":null}],"count":2}});
        assert_eq!(typed_roundtrip::<MutualA>(input.clone()), input);
        Schema::from_type::<MutualA>()
            .unwrap()
            .validate_params()
            .unwrap();
    }

    #[derive(schemars::JsonSchema)]
    #[serde(untagged)]
    #[allow(dead_code)]
    enum StringOrNumber {
        Text(String),
        Number(i32),
    }
    #[derive(schemars::JsonSchema)]
    #[allow(dead_code)]
    struct AmbiguousTree {
        next: Option<Box<AmbiguousTree>>,
        value: StringOrNumber,
    }

    #[test]
    fn recursive_ambiguity_walk_checks_reachable_fields_after_back_edges() {
        let schema = Schema::from_type::<AmbiguousTree>().unwrap();
        let error = schema.validate_params().unwrap_err();
        assert!(error.0.contains("ambiguous union"), "{error}");
        assert!(error.0.contains("value"), "{error}");
        for schema in [
            Schema::Record(Box::new(Schema::Union(vec![
                Schema::Number,
                Schema::String,
            ]))),
            Schema::Tuple(vec![Schema::Union(vec![Schema::Boolean, Schema::String])]),
        ] {
            assert!(
                schema
                    .validate_params()
                    .unwrap_err()
                    .0
                    .contains("ambiguous union")
            );
        }
        // Independently derived roots may reuse the same local definition name.
        let first = Arc::new(json!({"$ref":"#/$defs/X","$defs":{"X":{"type":"string"}}}));
        let second = Arc::new(
            json!({"$ref":"#/$defs/X","$defs":{"X":{"anyOf":[{"type":"string"},{"type":"number"}]}}}),
        );
        let compiled = [first, second]
            .map(|root| compile_schema(&root, &root, &mut Some(HashSet::new()), 0).unwrap());
        assert!(
            Schema::Tuple(compiled.into())
                .validate_params()
                .unwrap_err()
                .0
                .contains("ambiguous union")
        );
    }

    fn has_issue_message(error: &ValidationError, message: &str) -> bool {
        fn contains(value: &Value, message: &str) -> bool {
            match value {
                Value::Object(fields) => {
                    fields.get("message").and_then(Value::as_str) == Some(message)
                        || fields.values().any(|v| contains(v, message))
                }
                Value::Array(items) => items.iter().any(|v| contains(v, message)),
                _ => false,
            }
        }
        error.issues.iter().any(|v| contains(v, message))
    }

    #[test]
    fn validation_bounds_recursive_depth_total_work_and_reference_only_cycles() {
        let schema = Schema::from_type::<Recursive>().unwrap();
        let mut value = json!({"next":null});
        for _ in 0..100 {
            value = json!({"next":value});
        }
        let error = schema.parse_value(value).unwrap_err();
        assert!(has_issue_message(
            &error,
            "Schema nesting exceeds 128 levels"
        ));
        let wide = Schema::Array(Box::new(Schema::Number));
        let error = wide.parse_value(json!(vec![0; 65_536])).unwrap_err();
        assert_eq!(
            error.issues.last().unwrap()["message"],
            "Schema validation exceeds 65536 nodes"
        );
        assert_eq!(error.issues.last().unwrap()["path"], json!([65_535]));
        let cyclic = Arc::new(json!({"$ref":"#"}));
        let cyclic = compile_schema(&cyclic, &cyclic, &mut Some(HashSet::new()), 0).unwrap();
        cyclic.validate_params().unwrap();
        assert!(has_issue_message(
            &cyclic.parse_value(json!(null)).unwrap_err(),
            "Schema nesting exceeds 128 levels"
        ));
    }

    #[test]
    fn validation_limits_are_per_parse_and_allow_intentional_larger_flat_data() {
        let schema = Schema::Array(Box::new(Schema::Number));
        let limits = ValidationLimits {
            max_depth: 1,
            max_nodes: 3,
        };
        assert_eq!(
            schema
                .parse_value_with_limits(json!([1, 2]), limits)
                .unwrap(),
            json!([1, 2])
        );
        assert_eq!(
            schema
                .parse_value_with_limits(json!([1, 2, 3]), limits)
                .unwrap_err()
                .issues[0]["message"],
            "Schema validation exceeds 3 nodes"
        );
        assert_eq!(
            schema.parse_value(json!([1, 2, 3])).unwrap(),
            json!([1, 2, 3])
        );
        let larger = ValidationLimits {
            max_depth: 1,
            max_nodes: 65_538,
        };
        assert_eq!(
            schema
                .parse_value_with_limits(json!(vec![0; 65_537]), larger)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            65_537
        );
        assert_eq!(
            Schema::Optional(Box::new(Schema::Number))
                .parse_with_limits(None, limits)
                .unwrap(),
            None
        );
        let shallow = ValidationLimits {
            max_depth: 0,
            max_nodes: 3,
        };
        assert_eq!(
            schema
                .parse_value_with_limits(json!([1]), shallow)
                .unwrap_err()
                .issues[0]["message"],
            "Schema nesting exceeds 0 levels"
        );
    }

    #[test]
    fn catch_optional_and_union_cannot_hide_validation_limit_exhaustion() {
        let limits = ValidationLimits {
            max_depth: 128,
            max_nodes: 1,
        };
        for schema in [
            Schema::Catch(Box::new(Schema::Number), json!(0)),
            Schema::Optional(Box::new(Schema::Default(
                Box::new(Schema::Number),
                json!(0),
            ))),
            Schema::Union(vec![Schema::Number, Schema::Unknown]),
        ] {
            assert_eq!(
                schema.parse_with_limits(None, limits).unwrap_err().issues[0]["message"],
                "Schema validation exceeds 1 nodes"
            );
        }
    }

    #[test]
    fn unsupported_constructs_behind_recursive_references_fail_during_construction() {
        let root = Arc::new(
            json!({"$ref":"#/$defs/Node", "$defs":{"Node":{"type":"object","properties":{
                "next":{"$ref":"#/$defs/Node"}, "value":{"type":"string","pattern":"^x"}
            }}}}),
        );
        assert!(
            compile_schema(&root, &root, &mut Some(HashSet::new()), 0)
                .unwrap_err()
                .0
                .contains("keyword pattern")
        );
    }

    #[test]
    fn unsupported_typed_constructs_fail_explicitly() {
        for value in [
            json!({"type":"string","pattern":"a"}),
            json!({"type":"string","format":"email"}),
            json!({"$ref":"https://example.com"}),
            json!({"type":"array","uniqueItems":true}),
        ] {
            assert!(
                compile_schema(
                    &value,
                    &Arc::new(value.clone()),
                    &mut Some(HashSet::new()),
                    0
                )
                .unwrap_err()
                .0
                .starts_with("Unsupported generated JSON schema:")
            );
        }
    }

    #[test]
    fn generated_numeric_and_utf16_length_constraints_emit_zod_issues() {
        let number = json!({"type":"number","minimum":2});
        let schema = compile_schema(
            &number,
            &Arc::new(number.clone()),
            &mut Some(HashSet::new()),
            0,
        )
        .unwrap();
        assert_eq!(
            schema.parse_value(json!(1)).unwrap_err().issues,
            vec![
                json!({"origin":"number","code":"too_small","minimum":2,"inclusive":true,"path":[],"message":"Too small: expected number to be >=2"})
            ]
        );
        let string = json!({"type":"string","maxLength":1});
        let schema = compile_schema(
            &string,
            &Arc::new(string.clone()),
            &mut Some(HashSet::new()),
            0,
        )
        .unwrap();
        assert_eq!(
            schema.parse_value(json!("😀")).unwrap_err().issues[0]["message"],
            "Too big: expected string to have <=1 characters"
        );
    }
}
