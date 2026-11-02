//! Reading the API's JSON the forgiving way the Ruby tool does: a missing
//! key is nil, nil prints as nothing, and only nil and false are false.

use serde_json::Value;

static NULL: Value = Value::Null;

/// value[key], or null.
pub fn at<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&NULL)
}

/// value[a][b]..., or null.
pub fn dig<'a>(value: &'a Value, keys: &[&str]) -> &'a Value {
    keys.iter().fold(value, |found, key| at(found, key))
}

/// Ruby's to_s: nil is "", numbers and booleans as written.
pub fn s(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Ruby's truthiness: only nil and false are false.
pub fn truthy(value: &Value) -> bool {
    !matches!(value, Value::Null | Value::Bool(false))
}

/// Array(value): an array's items, nothing for nil, else the value alone.
pub fn list(value: &Value) -> Vec<Value> {
    match value {
        Value::Array(items) => items.clone(),
        Value::Null => vec![],
        other => vec![other.clone()],
    }
}

/// to_i: numbers as whole numbers, "12" as 12, anything else 0.
pub fn int(value: &Value) -> i64 {
    match value {
        Value::Number(n) => n.as_i64().unwrap_or_else(|| n.as_f64().map_or(0, |f| f as i64)),
        Value::String(text) => text.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// None for nil.
pub fn present(value: &Value) -> Option<&Value> {
    (!value.is_null()).then_some(value)
}

/// A JSON object from pairs, leaving out the nil ones (Ruby's compact).
pub fn object(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().filter(|(_, value)| !value.is_null()).map(|(key, value)| (key.to_string(), value)).collect())
}
