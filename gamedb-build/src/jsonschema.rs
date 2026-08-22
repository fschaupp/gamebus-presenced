//! The subset of JSON Schema the gamedb schemas actually use, applied to
//! parsed TOML.
//!
//! This is a deliberate port of `check()` in `.scripts/gamedb-lint.py`, not a
//! general validator, and it is faithful to that function including where the
//! function is loose:
//!
//! * `type` is enforced only for `object`, `array` and `string`. An `integer`
//!   declaration is documentation; nothing checks it. So is `minimum`.
//! * `pattern` is a *search*, not a match, so a pattern without anchors can
//!   hit anywhere in the value. Every pattern in `gamedb/schema/` anchors
//!   itself, which is why this has never mattered.
//! * `anyOf` is not implemented. `game.schema.json` carries one (a page needs
//!   `stores` or `exe`); the lint enforces that rule directly instead, with a
//!   message a contributor can act on.
//! * `$ref` resolves only at the top of a schema object, and only against the
//!   root document.
//!
//! Keeping the looseness is the point: the two linters have to produce the
//! same report, and a stricter validator here would fail pages that merge
//! today. Anything worth tightening belongs in both, in the same change.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value as Json;

use crate::pyrepr;

/// A validator over one schema document, caching compiled patterns.
///
/// The schema is borrowed throughout rather than cloned at every node: this
/// runs once per page, and the data set is meant to grow to thousands.
pub struct Validator {
    root: Json,
    patterns: Patterns,
}

impl Validator {
    /// Load a schema document.
    pub fn new(root: Json) -> Self {
        Self {
            root,
            patterns: Patterns::new(),
        }
    }

    /// Validate a parsed TOML document, appending findings to `errs`.
    ///
    /// `path` is the prefix every message carries; the Python lint passes the
    /// file's name, so messages read `control.toml: ...`.
    pub fn check(&mut self, value: &toml::Value, path: &str, errs: &mut Vec<String>) {
        check_against(
            &self.root,
            &mut self.patterns,
            &self.root,
            value,
            path,
            errs,
        );
    }
}

type Patterns = HashMap<String, Regex>;

fn check_against(
    root: &Json,
    patterns: &mut Patterns,
    schema: &Json,
    value: &toml::Value,
    path: &str,
    errs: &mut Vec<String>,
) {
    let schema = resolve(root, schema);

    match schema.get("type").and_then(Json::as_str) {
        Some("object") if !value.is_table() => {
            errs.push(format!("{path}: not an object"));
            return;
        }
        Some("array") if !value.is_array() => {
            errs.push(format!("{path}: not an array"));
            return;
        }
        Some("string") if !value.is_str() => {
            errs.push(format!("{path}: not a string"));
            return;
        }
        _ => {}
    }

    if let Some(allowed) = schema.get("enum") {
        let ok = allowed
            .as_array()
            .is_some_and(|a| a.iter().any(|want| equal(value, want)));
        if !ok {
            errs.push(format!(
                "{path}: {} not in {}",
                pyrepr::toml_value(value),
                pyrepr::json_value(allowed)
            ));
        }
    }

    if let Some(s) = value.as_str() {
        if let Some(pattern) = schema.get("pattern").and_then(Json::as_str) {
            if !matches(patterns, pattern, s) {
                errs.push(format!("{path}: {} fails /{pattern}/", pyrepr::string(s)));
            }
        }
        if s.chars().count() < usize_of(schema.get("minLength")) {
            errs.push(format!("{path}: too short"));
        }
    }

    if let Some(items) = value.as_array() {
        if items.len() < usize_of(schema.get("minItems")) {
            errs.push(format!("{path}: too few items"));
        }
        let item_schema = schema.get("items").unwrap_or_else(|| empty_schema());
        for (i, item) in items.iter().enumerate() {
            check_against(
                root,
                patterns,
                item_schema,
                item,
                &format!("{path}[{i}]"),
                errs,
            );
        }
    }

    if let Some(table) = value.as_table() {
        if table.len() < usize_of(schema.get("minProperties")) {
            errs.push(format!("{path}: too few properties"));
        }
        for required in schema
            .get("required")
            .and_then(Json::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            if let Some(name) = required.as_str() {
                if !table.contains_key(name) {
                    errs.push(format!("{path}: missing required {}", pyrepr::string(name)));
                }
            }
        }

        let props = schema.get("properties");
        let additional = schema.get("additionalProperties");
        let property_names = schema.get("propertyNames").map(|pn| resolve(root, pn));

        for (key, child) in table {
            // Python: `k not in resolve(pn).get("enum", [k])`. Without an enum
            // the default list contains the key, so nothing fires.
            if let Some(allowed) = property_names.and_then(|pn| pn.get("enum")) {
                let allowed = allowed.as_array().map(Vec::as_slice).unwrap_or_default();
                if !allowed.iter().any(|v| v.as_str() == Some(key.as_str())) {
                    errs.push(format!("{path}.{key}: store key not allowed"));
                }
            }

            let child_path = format!("{path}.{key}");
            match props.and_then(|p| p.get(key)) {
                Some(child_schema) => {
                    check_against(root, patterns, child_schema, child, &child_path, errs)
                }
                None => match additional {
                    Some(Json::Bool(false)) => errs.push(format!("{child_path}: unknown field")),
                    Some(other @ Json::Object(_)) => {
                        check_against(root, patterns, other, child, &child_path, errs)
                    }
                    _ => {}
                },
            }
        }
    }
}

/// Follow a `$ref` into the root document, once.
fn resolve<'a>(root: &'a Json, schema: &'a Json) -> &'a Json {
    let Some(reference) = schema.get("$ref").and_then(Json::as_str) else {
        return schema;
    };
    let mut node = root;
    for step in reference.trim_start_matches(['#', '/']).split('/') {
        match node.get(step) {
            Some(next) => node = next,
            // Python would raise KeyError here. A dangling $ref is a bug in
            // the schema, not in the data, and an empty schema keeps the run
            // going so the rest of the report still arrives.
            None => return empty_schema(),
        }
    }
    node
}

/// `re.search(pattern, value)`. Python's `$` also matches before a single
/// trailing newline where Rust's matches only at the end; no value in this
/// data set ends in one, and a schema that wants that should say `\n?$`.
fn matches(patterns: &mut Patterns, pattern: &str, value: &str) -> bool {
    patterns
        .entry(pattern.to_string())
        .or_insert_with(|| Regex::new(pattern).expect("schema carries an invalid pattern"))
        .is_match(value)
}

/// A schema that constrains nothing, for `items` left unsaid and for a `$ref`
/// that does not land.
fn empty_schema() -> &'static Json {
    static EMPTY: OnceLock<Json> = OnceLock::new();
    EMPTY.get_or_init(|| Json::Object(serde_json::Map::new()))
}

fn usize_of(v: Option<&Json>) -> usize {
    v.and_then(Json::as_u64).unwrap_or(0) as usize
}

/// Python's `==` between a parsed TOML value and a JSON literal.
fn equal(a: &toml::Value, b: &Json) -> bool {
    match (a, b) {
        (toml::Value::String(a), Json::String(b)) => a == b,
        (toml::Value::Integer(a), Json::Number(b)) => b.as_i64() == Some(*a),
        (toml::Value::Float(a), Json::Number(b)) => b.as_f64() == Some(*a),
        (toml::Value::Boolean(a), Json::Bool(b)) => a == b,
        (toml::Value::Array(a), Json::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equal(a, b))
        }
        (toml::Value::Table(a), Json::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, v)| b.get(k).is_some_and(|other| equal(v, other)))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(schema: serde_json::Value, doc: &str) -> Vec<String> {
        let mut errs = Vec::new();
        let value: toml::Value = doc.parse().expect("fixture parses");
        Validator::new(schema).check(&value, "doc.toml", &mut errs);
        errs
    }

    #[test]
    fn a_missing_required_field_is_named_the_way_python_names_it() {
        let errs = validate(
            serde_json::json!({"type": "object", "required": ["title"]}),
            "other = 1",
        );
        assert_eq!(errs, ["doc.toml: missing required 'title'"]);
    }

    #[test]
    fn an_enum_miss_prints_the_value_and_the_whole_enum() {
        let errs = validate(
            serde_json::json!({
                "type": "object",
                "properties": {"confidence": {"enum": ["high", "medium", "low"]}}
            }),
            r#"confidence = "certain""#,
        );
        assert_eq!(
            errs,
            ["doc.toml.confidence: 'certain' not in ['high', 'medium', 'low']"]
        );
    }

    #[test]
    fn a_pattern_miss_quotes_the_pattern() {
        let errs = validate(
            serde_json::json!({
                "type": "object",
                "properties": {"seen": {"type": "string", "pattern": "^[0-9]{4}$"}}
            }),
            r#"seen = "26""#,
        );
        assert_eq!(errs, ["doc.toml.seen: '26' fails /^[0-9]{4}$/"]);
    }

    #[test]
    fn additional_properties_false_rejects_unknown_fields() {
        let errs = validate(
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {"title": {"type": "string"}}
            }),
            "title = \"a\"\nstray = 1",
        );
        assert_eq!(errs, ["doc.toml.stray: unknown field"]);
    }

    #[test]
    fn property_names_gate_the_store_vocabulary() {
        let errs = validate(
            serde_json::json!({
                "type": "object",
                "propertyNames": {"enum": ["egs", "gog"]},
                "additionalProperties": true
            }),
            "itch = 1",
        );
        assert_eq!(errs, ["doc.toml.itch: store key not allowed"]);
    }

    #[test]
    fn a_ref_resolves_against_the_root() {
        let errs = validate(
            serde_json::json!({
                "type": "object",
                "properties": {"seen": {"$ref": "#/$defs/date"}},
                "$defs": {"date": {"type": "string", "pattern": "^[0-9]{4}-[0-9]{2}-[0-9]{2}$"}}
            }),
            r#"seen = "2026-8-22""#,
        );
        assert_eq!(
            errs,
            ["doc.toml.seen: '2026-8-22' fails /^[0-9]{4}-[0-9]{2}-[0-9]{2}$/"]
        );
    }

    #[test]
    fn counts_are_reported_the_way_python_reports_them() {
        let errs = validate(
            serde_json::json!({
                "type": "object",
                "minProperties": 2,
                "properties": {
                    "exe": {"type": "array", "minItems": 2, "items": {"type": "string", "minLength": 3}}
                }
            }),
            r#"exe = ["ab"]"#,
        );
        assert_eq!(
            errs,
            [
                "doc.toml: too few properties",
                "doc.toml.exe: too few items",
                "doc.toml.exe[0]: too short",
            ]
        );
    }
}
