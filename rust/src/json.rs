use serde_json::Value;

/// Parses finite JSON using ECMAScript numeric values and object entry order.
/// Integral values in serde_json's i64/u64 ranges retain integer representations for
/// typed decoding. Negative zero retains its f64 sign. Serde's finite-number and
/// Unicode-scalar input boundaries remain enforced.
pub fn parse(text: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(text).map(normalize)
}

/// ECMAScript own-property order: array indices first, then insertion-order strings.
pub(crate) fn ordered_entries(values: &serde_json::Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut entries = values.iter().collect::<Vec<_>>();
    entries.sort_by_key(|(key, _)| array_index(key).map_or((1, 0), |index| (0, index)));
    entries
}

fn normalize(value: Value) -> Value {
    match value {
        Value::Number(number) => {
            let number = number.as_f64().expect("parsed JSON numbers are finite");
            // Keep negative zero observable; typed integer schemas normalize it on validation.
            if number.fract() == 0.0 && !(number == 0.0 && number.is_sign_negative()) {
                if (0.0..18_446_744_073_709_551_616.0).contains(&number) {
                    return Value::from(number as u64);
                }
                if (-9_223_372_036_854_775_808.0..0.0).contains(&number) {
                    return Value::from(number as i64);
                }
            }
            Value::Number(
                serde_json::Number::from_f64(number).expect("parsed JSON numbers are finite"),
            )
        }
        Value::Array(values) => Value::Array(values.into_iter().map(normalize).collect()),
        Value::Object(mut fields) => {
            let keys = ordered_entries(&fields)
                .into_iter()
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            Value::Object(
                keys.into_iter()
                    .map(|key| {
                        let value = fields
                            .remove(&key)
                            .expect("ordered keys belong to the parsed object");
                        (key, normalize(value))
                    })
                    .collect(),
            )
        }
        other => other,
    }
}

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
            for (index, (key, value)) in ordered_entries(values).into_iter().enumerate() {
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
    fn parse_rounds_numbers_before_callbacks_and_preserves_integral_decoding() {
        #[derive(serde::Deserialize, Debug, PartialEq)]
        struct Numbers {
            positive: u64,
            negative: i64,
            integral_float: i32,
            nested: Vec<u64>,
        }
        let value = parse(r#"{"positive":9007199254740993,"negative":-9007199254740993,"integral_float":1.0,"nested":[9007199254740995]}"#).unwrap();
        assert_eq!(value["positive"], json!(9_007_199_254_740_992_u64));
        assert_eq!(value["negative"], json!(-9_007_199_254_740_992_i64));
        assert_eq!(value["nested"][0], json!(9_007_199_254_740_996_u64));
        assert_eq!(
            serde_json::from_value::<Numbers>(value).unwrap(),
            Numbers {
                positive: 9_007_199_254_740_992,
                negative: -9_007_199_254_740_992,
                integral_float: 1,
                nested: vec![9_007_199_254_740_996],
            }
        );
    }

    #[test]
    fn parse_uses_rounded_integer_ranges_without_saturating_casts() {
        let value = parse("[-9223372036854775807,9223372036854775807,18446744073709549568,18446744073709551615,-9223372036854777856]").unwrap();
        assert_eq!(value[0].as_i64(), Some(i64::MIN));
        assert_eq!(value[1].as_u64(), Some(9_223_372_036_854_775_808));
        assert_eq!(value[2].as_u64(), Some(18_446_744_073_709_549_568));
        assert_eq!(value[3].as_u64(), None);
        assert_eq!(value[3].as_f64(), Some(18_446_744_073_709_551_616.0));
        assert_eq!(value[4].as_i64(), None);
        assert_eq!(value[4].as_f64(), Some(-9_223_372_036_854_777_856.0));
    }

    #[test]
    fn parse_preserves_fractional_values_negative_zero_and_finite_boundaries() {
        let value =
            parse("[-0,-0.0,0,1.25,5e-324,1.7976931348623157e308,true,null,\"😀\"]").unwrap();
        for index in [0, 1] {
            assert_eq!(value[index].as_f64(), Some(-0.0));
            assert!(value[index].as_f64().unwrap().is_sign_negative());
        }
        assert_eq!(value[2].as_u64(), Some(0));
        assert_eq!(value[3].as_f64(), Some(1.25));
        assert_eq!(value[4].as_f64(), Some(f64::from_bits(1)));
        assert_eq!(value[5].as_f64(), Some(f64::MAX));
        assert_eq!(value[6], true);
        assert_eq!(value[7], Value::Null);
        assert_eq!(value[8], "😀");
        for text in ["1e400", "-1e400", "NaN", "[1,]", "{", r#""\ud800""#] {
            assert_eq!(
                parse(text).unwrap_err().to_string(),
                serde_json::from_str::<Value>(text).unwrap_err().to_string()
            );
        }
    }

    #[test]
    fn ordered_entries_obey_array_index_boundaries_and_stable_string_order() {
        let value: Value = serde_json::from_str(r#"{"z":0,"4294967295":1,"10":2,"2":3,"01":4,"0":5,"4294967294":6,"-0":7,"1.0":8,"a":9}"#).unwrap();
        let keys = ordered_entries(value.as_object().unwrap())
            .into_iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            vec![
                "0",
                "2",
                "10",
                "4294967294",
                "z",
                "4294967295",
                "01",
                "-0",
                "1.0",
                "a"
            ]
        );
    }

    #[test]
    fn parse_orders_nested_objects_and_retains_first_duplicate_key_position() {
        let value = parse(r#"{"z":0,"10":1,"2":{"b":0,"1":1,"0":2,"a":3},"a":1,"z":4}"#).unwrap();
        assert_eq!(
            value
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["2", "10", "z", "a"]
        );
        assert_eq!(
            value["2"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["0", "1", "b", "a"]
        );
        assert_eq!(value["z"], 4);
    }

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
