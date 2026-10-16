use crate::{
    DefinitionError, HttpError,
    schema::{Input, Schema, ValidationError},
};
use indexmap::IndexMap;
use serde_json::{Value, json};
use url::Url;

// Bound both each sparse index and the sum of allocations, including nested sparse arrays.
const MAX_QUERY_INDEX: usize = 4095;
const MAX_QUERY_NODES: usize = 65_536;
const MAX_QUERY_DEPTH: usize = 64;

fn segments(path: &str) -> Vec<&str> {
    path.strip_suffix('/').unwrap_or(path).split('/').collect()
}

fn path_params(path: &str) -> Result<Vec<(&str, bool)>, DefinitionError> {
    let mut optional_seen = false;
    let mut params = Vec::new();
    for segment in segments(path) {
        let optional = segment.ends_with('?');
        if optional {
            optional_seen = true;
        } else if optional_seen {
            return Err(DefinitionError(
                "Cannot have a non optional parameter after an optional parameter".into(),
            ));
        }
        if let Some(name) = segment.strip_prefix(':') {
            params.push((name.strip_suffix('?').unwrap_or(name), optional));
        }
    }
    Ok(params)
}

pub fn auto_schema(path: &str) -> Result<Schema, DefinitionError> {
    let params = path_params(path)?;
    let mut fields = IndexMap::new();
    // The reference groups mandatory IDs, mandatory strings, optional IDs, optional strings.
    for optional in [false, true] {
        for numeric in [true, false] {
            for &(name, is_optional) in &params {
                let is_numeric = name.ends_with("Id") || name.ends_with("_id");
                if optional == is_optional && numeric == is_numeric {
                    let schema = if numeric {
                        Schema::Number
                    } else {
                        Schema::String
                    };
                    fields.insert(
                        name.into(),
                        if optional {
                            Schema::Optional(Box::new(schema))
                        } else {
                            schema
                        },
                    );
                }
            }
        }
    }
    Ok(Schema::Object(fields))
}

pub fn validate_path(
    path: &str,
    schema: &Schema,
    prefixes: &[String],
) -> Result<(), DefinitionError> {
    let params = path_params(path)?;
    if !params.is_empty() {
        let Schema::Object(fields) = schema else {
            return Err(DefinitionError(format!(
                "Route \"{path}\" has path params [{}] but paramsValidation is not a ZodObject",
                params
                    .iter()
                    .map(|(n, _)| *n)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        };
        let missing = params
            .iter()
            .filter(|(n, _)| !fields.contains_key(*n))
            .map(|(n, _)| *n)
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(DefinitionError(format!(
                "Route \"{path}\" has path params [{}] missing from paramsValidation schema",
                missing.join(", ")
            )));
        }
    }
    schema.validate_params()?;
    for prefix in prefixes {
        if prefix.starts_with('/') {
            return Err(DefinitionError(format!(
                "Prefix \"{prefix}\" must not start with a slash"
            )));
        }
        if prefix.contains(':') {
            return Err(DefinitionError(format!(
                "Prefix \"{prefix}\" must not contain path params"
            )));
        }
    }
    Ok(())
}

pub fn parse_url(url: &Url, path: &str, schema: &Schema) -> Result<Value, ValidationError> {
    let mut fields = IndexMap::new();
    let mut budget = MAX_QUERY_NODES;
    for (key, value) in url.query_pairs() {
        spend(&mut budget, 1)?;
        let incoming = Input::Scalar(Value::String(value.into_owned()));
        match fields.entry(key.into_owned()) {
            indexmap::map::Entry::Vacant(entry) => {
                entry.insert(incoming);
            }
            indexmap::map::Entry::Occupied(mut entry) => match entry.get_mut() {
                Input::Array(items) => items.push(incoming),
                previous => {
                    *previous = Input::Array(vec![previous.clone(), incoming]);
                }
            },
        }
    }
    let actual = segments(url.path());
    for (index, segment) in segments(path).iter().enumerate() {
        if let Some(name) = segment.strip_prefix(':') {
            let name = name.strip_suffix('?').unwrap_or(name);
            let value = actual
                .get(index)
                .map(|s| Input::Scalar(Value::String((*s).into())))
                .unwrap_or(Input::Absent);
            // An absent path value shadows even a present query value.
            fields.insert(name.into(), value);
        }
    }
    let mut nested = Input::Object(IndexMap::new());
    for (key, value) in &fields {
        let parts = key.split('.').collect::<Vec<_>>();
        if parts.len() > MAX_QUERY_DEPTH {
            return Err(ValidationError::custom(
                "Query key nesting exceeds 64 levels",
            ));
        }
        let field = expand_field(&parts, value.clone(), &mut budget)?;
        nested = merge(nested, field)?;
    }
    let input = merge(Input::Object(fields), nested)?;
    parse_coerced(schema, input)
}

fn spend(budget: &mut usize, amount: usize) -> Result<(), ValidationError> {
    *budget = budget
        .checked_sub(amount)
        .ok_or_else(|| ValidationError::custom("Query structure exceeds 65536 nodes"))?;
    Ok(())
}

fn numeric_key(key: &str) -> bool {
    !key.is_empty() && key.bytes().all(|b| b.is_ascii_digit())
}

fn expand_field(
    parts: &[&str],
    value: Input,
    budget: &mut usize,
) -> Result<Input, ValidationError> {
    spend(budget, 1)?;
    let Some((&key, rest)) = parts.split_first() else {
        return Ok(value);
    };
    // parseFieldName treats an empty next component as the end of the key.
    let child = if rest.first().is_none_or(|next| next.is_empty()) {
        value
    } else if numeric_key(rest[0]) {
        expand_array(rest, value, budget)?
    } else {
        expand_field(rest, value, budget)?
    };
    Ok(Input::Object(IndexMap::from([(key.into(), child)])))
}

fn expand_array(
    parts: &[&str],
    value: Input,
    budget: &mut usize,
) -> Result<Input, ValidationError> {
    let key = parts[0];
    let index = key
        .parse::<usize>()
        .map_err(|_| ValidationError::custom("Query array index exceeds 4095"))?;
    if index > MAX_QUERY_INDEX {
        return Err(ValidationError::custom("Query array index exceeds 4095"));
    }
    // JavaScript treats noncanonical integer names such as 01 as array properties, not elements.
    if index.to_string() != key {
        return Ok(Input::Array(Vec::new()));
    }
    spend(budget, index + 1)?;
    let rest = &parts[1..];
    let child = if rest.first().is_none_or(|next| next.is_empty()) {
        value
    } else if numeric_key(rest[0]) {
        expand_array(rest, value, budget)?
    } else {
        expand_field(rest, value, budget)?
    };
    let mut array = vec![Input::Absent; index + 1];
    array[index] = child;
    Ok(Input::Array(array))
}

fn merge(base: Input, replacement: Input) -> Result<Input, ValidationError> {
    match (base, replacement) {
        (Input::Object(mut base), Input::Object(replacement)) => {
            for (key, value) in replacement {
                let previous = base.get(&key).cloned();
                let next = match (previous, value) {
                    (Some(previous), next @ (Input::Object(_) | Input::Array(_))) => {
                        merge(previous, next)?
                    }
                    (_, next) => next,
                };
                base.insert(key, next);
            }
            Ok(Input::Object(base))
        }
        (Input::Array(mut base), Input::Array(replacement)) => {
            if base.len() < replacement.len() {
                base.resize(replacement.len(), Input::Absent);
            }
            for (index, value) in replacement.into_iter().enumerate() {
                if matches!(value, Input::Absent | Input::Scalar(Value::Null)) {
                    continue;
                }
                let previous = std::mem::replace(&mut base[index], Input::Absent);
                base[index] = match (&previous, &value) {
                    (Input::Object(_), Input::Object(_)) | (Input::Array(_), Input::Array(_)) => {
                        merge(previous, value)?
                    }
                    _ => value,
                };
            }
            Ok(Input::Array(base))
        }
        (Input::Absent, replacement) => Ok(replacement),
        (_, Input::Absent) => Ok(Input::Absent),
        (_, Input::Object(_) | Input::Array(_)) => Err(ValidationError::custom(
            "Conflicting scalar and nested query keys",
        )),
        (_, replacement) => Ok(replacement),
    }
}

#[derive(Clone, Copy)]
enum Conversion {
    Number,
    Boolean,
}

// Only optional and readonly are unwrapped by the reference coercion pass.
fn coercion_schema(schema: &Schema) -> &Schema {
    match schema {
        Schema::Optional(inner) | Schema::Readonly(inner) | Schema::Refinement(inner, _) => {
            coercion_schema(inner)
        }
        _ => schema,
    }
}

fn js_number(value: &str) -> Option<Input> {
    if value.is_empty() {
        return None;
    }
    let trimmed = value.trim_matches(|c: char| matches!(c, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'));
    if trimmed.is_empty() {
        return Some(Input::Scalar(json!(0)));
    }
    let radix = if trimmed.starts_with("0x") || trimmed.starts_with("0X") {
        Some(16)
    } else if trimmed.starts_with("0b") || trimmed.starts_with("0B") {
        Some(2)
    } else if trimmed.starts_with("0o") || trimmed.starts_with("0O") {
        Some(8)
    } else {
        None
    };
    let number = if let Some(radix) = radix {
        if trimmed.len() == 2 {
            return None;
        }
        let mut number = 0.0;
        for ch in trimmed[2..].chars() {
            number = number * f64::from(radix) + f64::from(ch.to_digit(radix)?);
        }
        number
    } else {
        // Rust accepts inf, which JavaScript's Number does not.
        if trimmed.to_ascii_lowercase().contains("inf")
            && !matches!(trimmed, "Infinity" | "+Infinity" | "-Infinity")
        {
            return None;
        }
        trimmed.parse::<f64>().ok()?
    };
    if number.is_infinite() {
        return Some(Input::NonFinite {
            original: value.into(),
        });
    }
    if number.fract() == 0.0 && number.abs() < 9_007_199_254_740_992.0 {
        Some(Input::Scalar(json!(number as i64)))
    } else {
        serde_json::Number::from_f64(number).map(|n| Input::Scalar(Value::Number(n)))
    }
}

fn converted(schema: &Schema, input: &Input, conversion: Conversion) -> Option<Input> {
    let schema = coercion_schema(schema);
    if matches!(input, Input::Absent) {
        return None;
    }
    if let Schema::Union(branches) = schema {
        let mut result = None;
        for branch in branches {
            if let Some(value) = converted(branch, input, conversion) {
                // unionHandler drops scalar false and zero because it tests JavaScript truthiness.
                if matches!(&value, Input::Scalar(Value::Bool(false)))
                    || matches!(&value, Input::Scalar(Value::Number(n)) if n.as_f64() == Some(0.0))
                {
                    continue;
                }
                result = Some(match result {
                    Some(previous) => overlay_conversion(previous, value, true),
                    None => value,
                });
            }
        }
        return result;
    }
    match (schema, input) {
        (Schema::Number | Schema::Integer { .. }, Input::Scalar(Value::String(value)))
            if matches!(conversion, Conversion::Number) =>
        {
            js_number(value)
        }
        (Schema::Boolean, Input::Scalar(Value::String(value)))
            if matches!(conversion, Conversion::Boolean) =>
        {
            match value.as_str() {
                "true" => Some(Input::Scalar(Value::Bool(true))),
                "false" => Some(Input::Scalar(Value::Bool(false))),
                _ => None,
            }
        }
        (Schema::Array(element), Input::Array(items)) => {
            // The Bun reference filters unconverted entries before merging them back by index.
            let values = items
                .iter()
                .filter_map(|item| converted(element, item, conversion))
                .collect::<Vec<_>>();
            if values.is_empty() {
                None
            } else {
                Some(Input::Array(values))
            }
        }
        (Schema::Object(shape), Input::Object(fields)) => {
            let values = shape
                .iter()
                .filter_map(|(key, schema)| {
                    fields
                        .get(key)
                        .and_then(|value| converted(schema, value, conversion))
                        .map(|v| (key.clone(), v))
                })
                .collect::<IndexMap<_, _>>();
            if values.is_empty() {
                None
            } else {
                Some(Input::Object(values))
            }
        }
        _ => None,
    }
}

fn overlay_conversion(base: Input, replacement: Input, replace_arrays: bool) -> Input {
    match (base, replacement) {
        (Input::Object(mut base), Input::Object(replacement)) => {
            for (key, value) in replacement {
                let next = if let Some(previous) = base.get(&key) {
                    overlay_conversion(previous.clone(), value, replace_arrays)
                } else {
                    value
                };
                base.insert(key, next);
            }
            Input::Object(base)
        }
        (Input::Array(mut base), Input::Array(replacement)) if !replace_arrays => {
            if base.len() < replacement.len() {
                base.resize(replacement.len(), Input::Absent);
            }
            for (index, value) in replacement.into_iter().enumerate() {
                if matches!(value, Input::Absent | Input::Scalar(Value::Null)) {
                    continue;
                }
                let previous = std::mem::replace(&mut base[index], Input::Absent);
                base[index] = overlay_conversion(previous, value, replace_arrays);
            }
            Input::Array(base)
        }
        (previous, Input::Absent) => previous,
        (_, replacement) => replacement,
    }
}

fn coerce_input(schema: &Schema, input: Input) -> Input {
    let mut output = input;
    for conversion in [Conversion::Number, Conversion::Boolean] {
        // Body/query parsing merges only object and array conversion results, never root scalars.
        if let Some(value @ (Input::Object(_) | Input::Array(_))) =
            converted(schema, &output, conversion)
        {
            output = overlay_conversion(output, value, false);
        }
    }
    output
}

fn parse_coerced(schema: &Schema, input: Input) -> Result<Value, ValidationError> {
    schema
        .parse_input(&coerce_input(schema, input))?
        .ok_or_else(|| ValidationError::custom("Params schema returned undefined"))
}

pub(crate) fn parse_form_input(schema: &Schema, input: Input) -> Result<Value, ValidationError> {
    parse_coerced(schema, input)
}

pub fn parse_form(schema: &Schema, value: Value) -> Result<Value, ValidationError> {
    parse_coerced(schema, Input::from_value(value))
}

// JSON cannot carry non-finite numbers. HTTP validation must use parse_form, not this lossy utility.
pub fn coerce_form(schema: &Schema, value: Value) -> Value {
    fn restore_nonfinite(input: Input) -> Input {
        match input {
            Input::NonFinite { original, .. } => Input::Scalar(Value::String(original)),
            Input::Object(fields) => Input::Object(
                fields
                    .into_iter()
                    .map(|(k, v)| (k, restore_nonfinite(v)))
                    .collect(),
            ),
            Input::Array(items) => Input::Array(items.into_iter().map(restore_nonfinite).collect()),
            other => other,
        }
    }
    restore_nonfinite(coerce_input(schema, Input::from_value(value)))
        .to_value()
        .unwrap_or(Value::Null)
}

fn js_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(number) => {
            if let Some(number) = number.as_f64() {
                if number == 0.0 {
                    return "0".into();
                }
                let text = number.to_string();
                if number.abs() >= 1e21 || number.abs() < 1e-6 {
                    let scientific = format!("{number:e}");
                    if let Some((mantissa, exponent)) = scientific.split_once('e') {
                        return format!(
                            "{mantissa}e{}{exponent}",
                            if exponent.starts_with('-') { "" } else { "+" }
                        );
                    }
                }
                return text;
            }
            number.to_string()
        }
        Value::Array(items) => items
            .iter()
            .map(|v| {
                if v.is_null() {
                    String::new()
                } else {
                    js_string(v)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

fn encode_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'(' | b')')
        {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(encoded, "%{byte:02X}").expect("writing to a String cannot fail");
        }
    }
    encoded
}

fn stringify_object(
    parent: &str,
    value: &Value,
    query: &mut url::form_urlencoded::Serializer<'_, String>,
) {
    let fields = match value {
        Value::Object(fields) => fields
            .iter()
            .map(|(k, v)| (k.clone(), v))
            .collect::<Vec<_>>(),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(i, v)| (i.to_string(), v))
            .collect(),
        _ => return,
    };
    for (key, value) in fields {
        if value.is_object() || value.is_array() {
            stringify_object(
                &format!("{}.{}", encode_component(parent), encode_component(&key)),
                value,
                query,
            );
        } else {
            query.append_pair(&format!("{parent}.{key}"), &js_string(value));
        }
    }
}

pub fn def_to_url(path: &str, schema: &Schema, params: Value) -> Result<String, HttpError> {
    let route_params = path_params(path).map_err(|e| HttpError::new(400, e.to_string()))?;
    let parsed = schema
        .parse_value(params)
        .map_err(|e| HttpError::new(400, e.to_string()))?;
    let fields = parsed.as_object().ok_or_else(|| {
        HttpError::new(400, "Invalid params - only object should be used as params")
    })?;
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in fields {
        if route_params.iter().any(|(name, _)| *name == key) {
            continue;
        }
        if value.is_object() || value.is_array() {
            stringify_object(key, value, &mut query);
        } else {
            query.append_pair(key, &js_string(value));
        }
    }
    let mut result = path.strip_suffix('/').unwrap_or(path).to_owned();
    let mut missing = false;
    for (name, optional) in route_params {
        let replacement = if missing { None } else { fields.get(name) };
        if replacement.is_none() {
            missing = true;
        }
        let falsy = replacement.is_none_or(|v| {
            v.is_null()
                || v == &Value::Bool(false)
                || v.as_str() == Some("")
                || v.as_f64() == Some(0.0)
        });
        let token = format!(
            "{}:{name}{}",
            if optional && falsy { "/" } else { "" },
            if optional { "?" } else { "" }
        );
        let encoded = replacement
            .filter(|v| !v.is_null())
            .map(|v| encode_component(&js_string(v)))
            .unwrap_or_default();
        result = result.replacen(&token, &encoded, 1);
    }
    let query = query.finish();
    if !query.is_empty() {
        result.push('?');
        result.push_str(&query);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(fields: impl IntoIterator<Item = (&'static str, Schema)>) -> Schema {
        Schema::Object(fields.into_iter().map(|(k, s)| (k.into(), s)).collect())
    }
    fn optional(schema: Schema) -> Schema {
        Schema::Optional(Box::new(schema))
    }
    fn array(schema: Schema) -> Schema {
        Schema::Array(Box::new(schema))
    }
    fn parse(path: &str, schema: &Schema, url: &str) -> Result<Value, ValidationError> {
        parse_url(&Url::parse(url).unwrap(), path, schema)
    }

    #[test]
    fn auto_schema_groups_both_id_conventions_and_optional_fields_in_reference_order() {
        let schema =
            auto_schema("/items/:name/:itemId/:user_id/:slug?/:pageId?/:other_id?/").unwrap();
        let Schema::Object(fields) = &schema else {
            panic!("expected object");
        };
        assert_eq!(
            fields.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["itemId", "user_id", "name", "pageId", "other_id", "slug"]
        );
        assert_eq!(
            schema
                .parse_value(json!({"itemId":1,"user_id":2,"name":"a"}))
                .unwrap(),
            json!({"itemId":1,"user_id":2,"name":"a"})
        );
        assert!(
            schema
                .parse_value(json!({"itemId":"1","user_id":2,"name":"a"}))
                .is_err()
        );
        assert_eq!(
            auto_schema("/")
                .unwrap()
                .parse_value(json!({"ignored":1}))
                .unwrap(),
            json!({})
        );
    }

    #[test]
    fn path_validation_reports_optional_order_missing_keys_and_nonobjects() {
        assert_eq!(
            auto_schema("/:a?/:b").unwrap_err().0,
            "Cannot have a non optional parameter after an optional parameter"
        );
        assert!(auto_schema("/:a?/static").is_err());
        assert_eq!(
            validate_path("/:a/:b?", &object([("a", Schema::String)]), &[])
                .unwrap_err()
                .0,
            "Route \"/:a/:b?\" has path params [b] missing from paramsValidation schema"
        );
        assert_eq!(
            validate_path("/:a", &Schema::String, &[]).unwrap_err().0,
            "Route \"/:a\" has path params [a] but paramsValidation is not a ZodObject"
        );
        assert!(validate_path("/:a", &optional(object([("a", Schema::String)])), &[]).is_err());
        validate_path("/", &Schema::Unknown, &[]).unwrap();
    }

    #[test]
    fn path_validation_checks_ambiguity_and_prefixes() {
        let ambiguous = object([("a", Schema::Union(vec![Schema::String, Schema::Number]))]);
        assert!(
            validate_path("/", &ambiguous, &[])
                .unwrap_err()
                .0
                .contains("ambiguous union")
        );
        assert_eq!(
            validate_path("/", &object([]), &["/api".into()])
                .unwrap_err()
                .0,
            "Prefix \"/api\" must not start with a slash"
        );
        assert_eq!(
            validate_path("/", &object([]), &["api/:version".into()])
                .unwrap_err()
                .0,
            "Prefix \"api/:version\" must not contain path params"
        );
        validate_path("/", &object([]), &["api".into(), "v1".into()]).unwrap();
    }

    #[test]
    fn path_precedence_includes_absent_mandatory_and_optional_segments() {
        let schema = auto_schema("/items/:itemId/:slug?").unwrap();
        assert_eq!(
            parse(
                "/items/:itemId/:slug?",
                &schema,
                "http://x/items/3/path?itemId=9&slug=query"
            )
            .unwrap(),
            json!({"itemId":3,"slug":"path"})
        );
        assert_eq!(
            parse(
                "/items/:itemId/:slug?",
                &schema,
                "http://x/items/3?slug=query"
            )
            .unwrap(),
            json!({"itemId":3})
        );
        let error = parse("/items/:itemId/:slug?", &schema, "http://x/items?itemId=9").unwrap_err();
        assert_eq!(
            error.issues[0]["message"],
            "Invalid input: expected number, received undefined"
        );
        assert_eq!(error.issues[0]["path"], json!(["itemId"]));
    }

    #[test]
    fn path_segments_remain_percent_encoded_as_in_reference() {
        let schema = auto_schema("/:name").unwrap();
        assert_eq!(
            parse("/:name", &schema, "http://x/a%20b%2Fc?name=other").unwrap(),
            json!({"name":"a%20b%2Fc"})
        );
        assert_eq!(
            parse("/:name/", &schema, "http://x/value/").unwrap(),
            json!({"name":"value"})
        );
    }

    #[test]
    fn repeated_query_values_nested_objects_and_indexed_arrays() {
        let schema = object([
            ("tags", array(Schema::String)),
            (
                "filters",
                array(object([
                    ("field", Schema::String),
                    ("all_of", array(Schema::String)),
                ])),
            ),
            ("matrix", array(array(Schema::Number))),
        ]);
        let result = parse("/", &schema, "http://x/?tags=one&tags=two&filters.0.field=current_job&filters.0.all_of.0=METAL+work&matrix.0.0=1&matrix.0.1=2&matrix.1.0=3").unwrap();
        assert_eq!(
            result,
            json!({"tags":["one","two"],"filters":[{"field":"current_job","all_of":["METAL work"]}],"matrix":[[1,2],[3]]})
        );
    }

    #[test]
    fn repeated_nested_query_keys_preserve_values_and_empty_strings() {
        let schema = object([("nested", object([("tags", array(Schema::String))]))]);
        assert_eq!(
            parse(
                "/",
                &schema,
                "http://x/?nested.tags=a&nested.tags=&nested.tags=b"
            )
            .unwrap(),
            json!({"nested":{"tags":["a","","b"]}})
        );
        assert!(
            parse(
                "/",
                &object([("tag", array(Schema::String))]),
                "http://x/?tag=one"
            )
            .is_err()
        );
    }

    #[test]
    fn dot_expansion_retains_flat_keys_for_unknown_schemas() {
        let result = parse("/", &Schema::Unknown, "http://x/?a.b=c").unwrap();
        assert_eq!(result, json!({"a.b":"c","a":{"b":"c"}}));
        assert_eq!(
            parse(
                "/",
                &object([("a", Schema::String)]),
                "http://x/?a..ignored=c"
            )
            .unwrap(),
            json!({"a":"c"})
        );
    }

    #[test]
    fn sparse_string_arrays_distinguish_holes_from_null() {
        let schema = object([("a", array(Schema::String))]);
        let error = parse("/", &schema, "http://x/?a.1=value").unwrap_err();
        assert_eq!(
            error.issues,
            vec![
                json!({"expected":"string","code":"invalid_type","path":["a",0],"message":"Invalid input: expected string, received undefined"})
            ]
        );
        let optional_items = object([("a", array(optional(Schema::String)))]);
        assert_eq!(
            parse("/", &optional_items, "http://x/?a.1=value").unwrap(),
            json!({"a":[null,"value"]})
        );
        assert_eq!(
            parse("/", &schema, "http://x/?a.01=value").unwrap(),
            json!({"a":[]})
        );
    }

    #[test]
    fn hostile_sparse_indices_depth_and_total_expansion_are_bounded() {
        let schema = object([("a", array(Schema::Unknown))]);
        for index in ["4096", "4294967294", "999999999999999999999999999999"] {
            let error = parse("/", &schema, &format!("http://x/?a.{index}=x")).unwrap_err();
            assert_eq!(error.issues[0]["message"], "Query array index exceeds 4095");
        }
        let deep = format!("http://x/?{}=v", vec!["a"; 65].join("."));
        assert_eq!(
            parse("/", &schema, &deep).unwrap_err().issues[0]["message"],
            "Query key nesting exceeds 64 levels"
        );
        let wide = format!(
            "http://x/?{}",
            (0..17)
                .map(|i| format!("a{i}.4095=x"))
                .collect::<Vec<_>>()
                .join("&")
        );
        assert_eq!(
            parse("/", &Schema::Unknown, &wide).unwrap_err().issues[0]["message"],
            "Query structure exceeds 65536 nodes"
        );
    }

    #[test]
    fn scalar_nested_collisions_are_explicit_errors_and_prototype_names_are_data() {
        assert_eq!(
            parse("/", &Schema::Unknown, "http://x/?a=scalar&a.b=nested")
                .unwrap_err()
                .issues[0]["message"],
            "Conflicting scalar and nested query keys"
        );
        let schema = object([("__proto__", object([("value", Schema::String)]))]);
        assert_eq!(
            parse("/", &schema, "http://x/?__proto__.value=safe").unwrap(),
            json!({"__proto__":{"value":"safe"}})
        );
    }

    #[test]
    fn recursive_number_boolean_coercion_preserves_other_fields() {
        let schema = object([
            ("id", Schema::Number),
            (
                "rows",
                array(object([
                    ("numbers", array(Schema::Number)),
                    ("flag", optional(Schema::Boolean)),
                ])),
            ),
        ]);
        assert_eq!(
            coerce_form(
                &schema,
                json!({"id":"1","rows":[{"numbers":["2","3.5"],"flag":"false"}],"ignored":"4"})
            ),
            json!({"id":1,"rows":[{"numbers":[2,3.5],"flag":false}],"ignored":"4"})
        );
    }

    #[test]
    fn number_coercion_matches_javascript_decimal_radix_and_whitespace_rules() {
        let schema = object([("n", Schema::Number)]);
        for (input, expected) in [
            ("123", json!(123)),
            ("123.45", json!(123.45)),
            ("  ", json!(0)),
            ("0xFF", json!(255)),
            ("0b11", json!(3)),
            ("0o10", json!(8)),
            ("1e2", json!(100)),
            ("+.5", json!(0.5)),
            ("-0", json!(0)),
        ] {
            assert_eq!(
                coerce_form(&schema, json!({"n":input})),
                json!({"n":expected}),
                "{input}"
            );
        }
        for input in ["", "abc", "1_2", "NaN", "0x", "+0x1", "inf"] {
            assert_eq!(
                coerce_form(&schema, json!({"n":input})),
                json!({"n":input}),
                "{input}"
            );
        }
    }

    #[test]
    fn number_coercion_uses_ecmascript_whitespace_not_unicode_whitespace() {
        let schema = object([("n", Schema::Number)]);
        for input in ["\u{0085}", "\u{0085}2\u{0085}", "\u{180e}"] {
            assert_eq!(coerce_form(&schema, json!({"n":input})), json!({"n":input}));
        }
        for input in ["\u{feff}", "\u{2000}2\u{2029}", "\u{00a0}2\u{3000}"] {
            assert!(parse_form(&schema, json!({"n":input})).is_ok());
        }
    }

    #[test]
    fn json_decimal_input_retains_the_same_float_as_javascript() {
        let schema = object([("n", Schema::Number)]);
        let input: Value = serde_json::from_str("{\"n\":-1.738134420912916e96}").unwrap();
        assert_eq!(
            def_to_url("/", &schema, input).unwrap(),
            "?n=-1.738134420912916e%2B96"
        );
    }

    #[test]
    fn url_number_formatting_uses_shortest_decimal_and_ecmascript_exponent_thresholds() {
        let schema = object([("n", Schema::Number)]);
        for (number, expected) in [
            (
                json!(1_000_000_000_000_000_128_u64),
                "?n=1000000000000000100",
            ),
            (json!(1e20), "?n=100000000000000000000"),
            (json!(1e21), "?n=1e%2B21"),
            (json!(1e-6), "?n=0.000001"),
            (json!(1e-7), "?n=1e-7"),
        ] {
            assert_eq!(
                def_to_url("/", &schema, json!({"n":number})).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn boolean_coercion_accepts_only_exact_lowercase_strings() {
        let schema = object([("flag", Schema::Boolean)]);
        for (input, expected) in [
            ("true", json!(true)),
            ("false", json!(false)),
            ("TRUE", json!("TRUE")),
            (" true", json!(" true")),
            ("0", json!("0")),
            ("", json!("")),
        ] {
            assert_eq!(
                coerce_form(&schema, json!({"flag":input})),
                json!({"flag":expected})
            );
        }
    }

    #[test]
    fn coercion_unwraps_only_reference_supported_wrappers() {
        let schema = object([
            ("optional", optional(Schema::Number)),
            ("readonly", Schema::Readonly(Box::new(Schema::Boolean))),
            (
                "default",
                Schema::Default(Box::new(Schema::Number), json!(2)),
            ),
            ("nullable", Schema::Nullable(Box::new(Schema::Number))),
            (
                "catch",
                Schema::Catch(Box::new(Schema::Boolean), json!(false)),
            ),
            ("transform", Schema::Number.transform(Ok)),
        ]);
        let input = json!({"optional":"1","readonly":"false","default":"3","nullable":"4","catch":"true","transform":"5"});
        assert_eq!(
            coerce_form(&schema, input),
            json!({"optional":1,"readonly":false,"default":"3","nullable":"4","catch":"true","transform":"5"})
        );
    }

    #[test]
    fn mixed_array_conversion_retains_reference_filtered_overlay_behavior() {
        assert_eq!(
            coerce_form(
                &object([("a", array(Schema::Number))]),
                json!({"a":["bad","2","3"]})
            ),
            json!({"a":[2,3,"3"]})
        );
        assert_eq!(
            coerce_form(
                &object([("a", array(Schema::Boolean))]),
                json!({"a":["bad","false","true"]})
            ),
            json!({"a":[false,true,"true"]})
        );
        let error = parse(
            "/",
            &object([("a", array(Schema::Number))]),
            "http://x/?a.1=2",
        )
        .unwrap_err();
        assert_eq!(error.issues[0]["path"], json!(["a", 1]));
        assert_eq!(
            error.issues[0]["message"],
            "Invalid input: expected number, received string"
        );
    }

    #[test]
    fn union_coercion_preserves_reference_zero_and_false_truthiness_edge() {
        let number = object([(
            "v",
            Schema::Union(vec![Schema::Number, object([("x", Schema::String)])]),
        )]);
        assert_eq!(coerce_form(&number, json!({"v":"2"})), json!({"v":2}));
        assert_eq!(coerce_form(&number, json!({"v":"0"})), json!({"v":"0"}));
        assert!(parse("/", &number, "http://x/?v=0").is_err());
        let boolean = object([(
            "v",
            Schema::Union(vec![Schema::Boolean, object([("x", Schema::String)])]),
        )]);
        assert_eq!(
            coerce_form(&boolean, json!({"v":"true"})),
            json!({"v":true})
        );
        assert_eq!(
            coerce_form(&boolean, json!({"v":"false"})),
            json!({"v":"false"})
        );
    }

    #[test]
    fn checked_form_and_url_parsing_preserve_nonfinite_number_errors() {
        let schema = object([(
            "rows",
            array(object([("number", optional(Schema::Number))])),
        )]);
        for value in ["Infinity", "+Infinity", "-Infinity", "1e999", "-1e999"] {
            let error = parse_form(&schema, json!({"rows":[{"number":value}]})).unwrap_err();
            assert_eq!(
                error.issues,
                vec![
                    json!({"expected":"number","code":"invalid_type","received":"Infinity","path":["rows",0,"number"],"message":"Invalid input: expected number, received number"})
                ]
            );
            let url = format!("http://x/?rows.0.number={}", value.replace('+', "%2B"));
            assert_eq!(parse("/", &schema, &url).unwrap_err(), error);
        }
        let union = object([(
            "n",
            Schema::Union(vec![Schema::Number, Schema::Literal(Value::Null)]),
        )]);
        let error = parse_form(&union, json!({"n":"-Infinity"})).unwrap_err();
        assert_eq!(error.issues[0]["errors"][0][0]["received"], "Infinity");
        assert_eq!(error.issues[0]["errors"][1][0]["code"], "invalid_value");
        let lossy = object([("n", Schema::Number)]);
        assert_eq!(
            coerce_form(&lossy, json!({"n":"Infinity"})),
            json!({"n":"Infinity"})
        );
        assert_eq!(
            parse_form(&lossy, json!({"n": {"type":"Infinity"}}))
                .unwrap_err()
                .issues[0]["message"],
            "Invalid input: expected number, received object"
        );
    }

    #[test]
    fn flat_form_coercion_does_not_expand_dots_or_convert_root_scalars() {
        assert_eq!(coerce_form(&Schema::Number, json!("3")), json!("3"));
        let schema = object([("a", object([("n", Schema::Number)]))]);
        assert_eq!(coerce_form(&schema, json!({"a.n":"3"})), json!({"a.n":"3"}));
        assert_eq!(
            coerce_form(&array(Schema::Number), json!(["1", "2"])),
            json!([1, 2])
        );
    }

    #[test]
    fn derived_schema_coercion_integrates_with_typed_deserialization() {
        #[derive(schemars::JsonSchema, serde::Deserialize, Debug, PartialEq)]
        struct Query {
            page: u32,
            ready: bool,
            tags: Vec<String>,
        }
        let schema = Schema::from_type::<Query>().unwrap();
        let value = parse("/", &schema, "http://x/?page=3&ready=false&tags=a&tags=b").unwrap();
        let typed: Query = serde_json::from_value(value).unwrap();
        assert_eq!(
            typed,
            Query {
                page: 3,
                ready: false,
                tags: vec!["a".into(), "b".into()]
            }
        );
    }

    #[test]
    fn url_generation_validates_strips_unknown_and_preserves_property_order() {
        let schema = object([("z", Schema::Number), ("a", Schema::String)]);
        assert_eq!(
            def_to_url(
                "/test/",
                &schema,
                json!({"a":"hello","unknown":true,"z":23})
            )
            .unwrap(),
            "/test?z=23&a=hello"
        );
        let error = def_to_url("/test", &schema, json!({"z":"23","a":"hello"})).unwrap_err();
        assert_eq!(error.status, 400);
        assert!(error.message.contains("expected number, received string"));
        assert_eq!(
            def_to_url("/", &Schema::String, json!("a"))
                .unwrap_err()
                .message,
            "Invalid params - only object should be used as params"
        );
    }

    #[test]
    fn url_generation_serializes_nested_objects_arrays_null_and_empty_arrays() {
        let schema = object([
            (
                "rows",
                array(object([("name", Schema::String), ("age", Schema::Number)])),
            ),
            ("nested", object([("array", array(Schema::String))])),
            ("empty", array(Schema::String)),
            ("null", Schema::Nullable(Box::new(Schema::String))),
            ("missing", optional(Schema::String)),
        ]);
        assert_eq!(def_to_url("/test", &schema, json!({"rows":[{"name":"John","age":30},{"name":"Jane","age":25}],"nested":{"array":["A","B"]},"empty":[],"null":null})).unwrap(), "/test?rows.0.name=John&rows.0.age=30&rows.1.name=Jane&rows.1.age=25&nested.array.0=A&nested.array.1=B&null=null");
    }

    #[test]
    fn url_optional_truncation_omits_later_values_after_absence() {
        let schema = object([
            ("id", Schema::Number),
            ("slug", optional(Schema::String)),
            ("page", optional(Schema::Number)),
            ("q", optional(Schema::String)),
        ]);
        let path = "/test/:id/:slug?/:page?/";
        for (input, expected) in [
            (
                json!({"id":5,"slug":"hello","page":99,"q":"test"}),
                "/test/5/hello/99?q=test",
            ),
            (json!({"id":6,"q":"test"}), "/test/6?q=test"),
            (json!({"id":6,"page":100,"q":"test"}), "/test/6?q=test"),
            (json!({"id":6,"slug":"","page":0}), "/test/60"),
        ] {
            assert_eq!(def_to_url(path, &schema, input).unwrap(), expected);
        }
    }

    #[test]
    fn url_path_and_query_encoding_match_uri_component_and_search_params() {
        let schema = object([
            ("id", Schema::Number),
            ("slug", optional(Schema::String)),
            ("q", Schema::String),
        ]);
        assert_eq!(
            def_to_url(
                "/test/:id/:slug?",
                &schema,
                json!({"id":8000000000001_u64,"slug":"don't /雪!~*()","q":"t;'`%! est"})
            )
            .unwrap(),
            "/test/8000000000001/don%27t%20%2F%E9%9B%AA!~*()?q=t%3B%27%60%25%21+est"
        );
    }

    #[test]
    fn url_nested_special_keys_preserve_reference_repeated_encoding() {
        let schema = object([(
            "spaced out",
            object([(
                "star+><=&array",
                array(object([("name", Schema::String), ("age", Schema::Number)])),
            )]),
        )]);
        assert_eq!(def_to_url("/testpath", &schema, json!({"spaced out":{"star+><=&array":[{"name":"Joh`n","age":30},{"name":"Jane","age":25}]}})).unwrap(), "/testpath?spaced%252520out.star%25252B%25253E%25253C%25253D%252526array.0.name=Joh%60n&spaced%252520out.star%25252B%25253E%25253C%25253D%252526array.0.age=30&spaced%252520out.star%25252B%25253E%25253C%25253D%252526array.1.name=Jane&spaced%252520out.star%25252B%25253E%25253C%25253D%252526array.1.age=25");
    }

    #[test]
    fn url_generation_applies_rust_transforms_and_union_branch_stripping() {
        let schema = object([(
            "q",
            Schema::String
                .transform(|v| Ok(v.and_then(|v| v.as_str().map(|s| json!(s.to_uppercase()))))),
        )]);
        assert_eq!(
            def_to_url("/", &schema, json!({"q":"hello"})).unwrap(),
            "?q=HELLO"
        );
        let union = Schema::Union(vec![
            object([("a", Schema::String)]),
            object([("b", Schema::Number)]),
        ]);
        assert_eq!(
            def_to_url("/test", &union, json!({"b":3,"ignored":true})).unwrap(),
            "/test?b=3"
        );
    }
}
