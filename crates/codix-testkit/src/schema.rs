//! Telling a breaking schema change from an additive one, for the JSON
//! output snapshots.
//!
//! Breaking: a property or definition removed, a type narrowed, an allowed
//! value removed; for requests, a property newly required; for responses, a
//! property no longer always present. Everything else, such as a new optional
//! request property or a new response field, is additive.

use serde_json::Value;

/// Which side of the protocol a schema describes: what makes a change
/// breaking differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// What clients send: a newly required property breaks them.
    Request,
    /// What clients read: a property that may now be missing breaks them.
    Response,
}

/// The breaking changes from `old` to `new`, each described with its JSON
/// path; empty when the change is additive or there is none.
pub fn breaking_changes(old: &Value, new: &Value, side: Side) -> Vec<String> {
    let mut found = Vec::new();
    compare(old, new, side, "$", &mut found);
    let none = serde_json::Map::new();
    let new_defs = new.get("$defs").and_then(Value::as_object).unwrap_or(&none);
    for (name, old_def) in old.get("$defs").and_then(Value::as_object).unwrap_or(&none) {
        match new_defs.get(name) {
            None => found.push(format!("$defs.{name}: removed")),
            Some(new_def) => compare(old_def, new_def, side, &format!("$defs.{name}"), &mut found),
        }
    }
    found
}

fn compare(old: &Value, new: &Value, side: Side, path: &str, found: &mut Vec<String>) {
    let old_types = types(old);
    let new_types = types(new);
    if !old_types.is_empty() && old_types.iter().any(|t| !new_types.contains(t)) {
        found.push(format!("{path}: type {old_types:?} became {new_types:?}"));
    }
    if old.get("$ref") != new.get("$ref") && old.get("$ref").is_some() {
        found.push(format!(
            "{path}: refers to {} instead of {}",
            new.get("$ref").unwrap_or(&Value::Null),
            old.get("$ref").unwrap_or(&Value::Null)
        ));
    }

    let old_values = allowed_values(old);
    let new_values = allowed_values(new);
    if !old_values.is_empty() && !new_values.is_empty() {
        for value in &old_values {
            if !new_values.contains(value) {
                found.push(format!("{path}: value {value} no longer allowed"));
            }
        }
    }

    let old_required = required(old);
    let new_required = required(new);
    match side {
        Side::Request => {
            for name in new_required.iter().filter(|n| !old_required.contains(n)) {
                found.push(format!("{path}.{name}: now required"));
            }
        }
        Side::Response => {
            for name in old_required.iter().filter(|n| !new_required.contains(n)) {
                found.push(format!("{path}.{name}: no longer always present"));
            }
        }
    }

    if let Some(old_properties) = old.get("properties").and_then(Value::as_object) {
        let new_properties = new.get("properties").and_then(Value::as_object);
        for (name, old_property) in old_properties {
            match new_properties.and_then(|p| p.get(name)) {
                None => found.push(format!("{path}.{name}: removed")),
                Some(new_property) => {
                    compare(
                        old_property,
                        new_property,
                        side,
                        &format!("{path}.{name}"),
                        found,
                    );
                }
            }
        }
    }
    if let (Some(old_items), Some(new_items)) = (old.get("items"), new.get("items")) {
        compare(old_items, new_items, side, &format!("{path}[]"), found);
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let (Some(old_list), Some(new_list)) = (
            old.get(key).and_then(Value::as_array),
            new.get(key).and_then(Value::as_array),
        ) && old_list.len() == new_list.len()
        {
            for (i, (o, n)) in old_list.iter().zip(new_list).enumerate() {
                compare(o, n, side, &format!("{path}.{key}[{i}]"), found);
            }
        }
    }
}

fn types(schema: &Value) -> Vec<String> {
    match schema.get("type") {
        Some(Value::String(t)) => vec![t.clone()],
        Some(Value::Array(list)) => list
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// The values an enumeration allows: `enum`, or the `const` of each
/// alternative.
fn allowed_values(schema: &Value) -> Vec<Value> {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        return values.clone();
    }
    ["oneOf", "anyOf"]
        .iter()
        .filter_map(|key| schema.get(*key).and_then(Value::as_array))
        .flatten()
        .filter_map(|alternative| alternative.get("const").cloned())
        .collect()
}

fn required(schema: &Value) -> Vec<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn object(properties: &Value, required: &[&str]) -> Value {
        json!({ "type": "object", "properties": properties, "required": required })
    }

    #[test]
    fn identical_schemas_have_no_breaking_change() {
        let schema = object(&json!({ "a": { "type": "string" } }), &["a"]);
        assert_eq!(
            breaking_changes(&schema, &schema, Side::Response),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_new_optional_property_is_additive() {
        let old = object(&json!({ "a": { "type": "string" } }), &["a"]);
        let new = object(
            &json!({ "a": { "type": "string" }, "b": { "type": "integer" } }),
            &["a"],
        );
        assert_eq!(
            breaking_changes(&old, &new, Side::Request),
            Vec::<String>::new()
        );
        assert_eq!(
            breaking_changes(&old, &new, Side::Response),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_new_field_always_present_is_additive_in_a_response_only() {
        let old = object(&json!({ "a": { "type": "string" } }), &["a"]);
        let new = object(
            &json!({ "a": { "type": "string" }, "b": { "type": "integer" } }),
            &["a", "b"],
        );
        assert_eq!(
            breaking_changes(&old, &new, Side::Response),
            Vec::<String>::new()
        );
        assert_eq!(
            breaking_changes(&old, &new, Side::Request),
            ["$.b: now required"]
        );
    }

    #[test]
    fn a_removed_property_breaks() {
        let old = object(&json!({ "a": { "type": "string" }, "b": {} }), &[]);
        let new = object(&json!({ "a": { "type": "string" } }), &[]);
        assert_eq!(
            breaking_changes(&old, &new, Side::Response),
            ["$.b: removed"]
        );
    }

    #[test]
    fn a_changed_type_breaks() {
        let old = object(&json!({ "a": { "type": "string" } }), &[]);
        let new = object(&json!({ "a": { "type": "integer" } }), &[]);
        assert_eq!(breaking_changes(&old, &new, Side::Request).len(), 1);
        // Widening is additive.
        let nullable = object(&json!({ "a": { "type": ["string", "null"] } }), &[]);
        assert_eq!(
            breaking_changes(&old, &nullable, Side::Request),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_field_that_may_now_be_missing_breaks_a_response() {
        let old = object(&json!({ "a": { "type": "string" } }), &["a"]);
        let new = object(&json!({ "a": { "type": "string" } }), &[]);
        assert_eq!(
            breaking_changes(&old, &new, Side::Response),
            ["$.a: no longer always present"]
        );
    }

    #[test]
    fn a_removed_enum_value_breaks_and_a_new_one_does_not() {
        let mode = |values: &[&str]| json!({ "oneOf": values.iter().map(|v| json!({ "const": v, "type": "string" })).collect::<Vec<_>>() });
        let old = mode(&["exact", "prefix"]);
        assert_eq!(
            breaking_changes(&old, &mode(&["exact", "prefix", "glob"]), Side::Request),
            Vec::<String>::new()
        );
        assert_eq!(
            breaking_changes(&old, &mode(&["exact"]), Side::Request),
            ["$: value \"prefix\" no longer allowed"]
        );
    }

    #[test]
    fn definitions_are_compared_by_name() {
        let old = json!({ "$defs": { "Kind": { "type": "string" }, "Gone": {} } });
        let new = json!({ "$defs": { "Kind": { "type": "integer" } } });
        let found = breaking_changes(&old, &new, Side::Response);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.contains(&"$defs.Gone: removed".to_owned()));
    }
}
