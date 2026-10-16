use serde_json::Value;

/// Matches JSON.stringify for JSON values, including f64 spelling and index-key order.
pub fn stringify(value: &Value) -> Result<String, serde_json::Error> {
    let mut output = String::new();
    write(value, &mut output)?;
    Ok(output)
}

fn array_index(key: &str) -> Option<u32> {
    let index = key.parse::<u32>().ok()?;
    (index != u32::MAX && index.to_string() == key).then_some(index)
}

fn write(value: &Value, output: &mut String) -> Result<(), serde_json::Error> {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(true) => output.push_str("true"),
        Value::Bool(false) => output.push_str("false"),
        Value::String(value) => output.push_str(&serde_json::to_string(value)?),
        Value::Number(number) => {
            // JavaScript JSON.parse and JSON.stringify use IEEE-754 numbers.
            let number = number
                .as_f64()
                .expect("a JSON number has a finite f64 representation");
            output.push_str(ryu_js::Buffer::new().format(number));
        }
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write(value, output)?;
            }
            output.push(']');
        }
        Value::Object(values) => {
            output.push('{');
            let mut numeric: Vec<_> = values
                .iter()
                .filter_map(|(key, value)| array_index(key).map(|index| (index, key, value)))
                .collect();
            numeric.sort_unstable_by_key(|(index, _, _)| *index);
            let ordered = numeric
                .into_iter()
                .map(|(_, key, value)| (key, value))
                .chain(values.iter().filter(|(key, _)| array_index(key).is_none()));
            for (index, (key, value)) in ordered.enumerate() {
                if index != 0 {
                    output.push(',');
                }
                output.push_str(&serde_json::to_string(key)?);
                output.push(':');
                write(value, output)?;
            }
            output.push('}');
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_use_ecmascript_formatting() {
        let value: Value =
            serde_json::from_str("[1.0,-0.0,1e-7,1e-6,1e20,1e21,9007199254740993]").unwrap();
        assert_eq!(
            stringify(&value).unwrap(),
            "[1,0,1e-7,0.000001,100000000000000000000,1e+21,9007199254740992]"
        );
    }

    #[test]
    fn object_index_keys_sort_before_insertion_order_strings() {
        let value: Value =
            serde_json::from_str(r#"{"z":1,"10":2,"2":3,"01":4,"0":5,"4294967295":6,"a":7}"#)
                .unwrap();
        assert_eq!(
            stringify(&value).unwrap(),
            r#"{"0":5,"2":3,"10":2,"z":1,"01":4,"4294967295":6,"a":7}"#
        );
    }

    #[test]
    fn nested_values_and_strings_preserve_json_types() {
        let value = json!({"text": "\"\\\n\u{2028}", "values": [null, true, false, {"0":"<tag>"}]});
        let text = stringify(&value).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), value);
    }
}
