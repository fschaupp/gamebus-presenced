//! Python's `repr()`, for the handful of values the schema check quotes.
//!
//! The lint's messages are a contract: `.scripts/gamedb-lint-fixtures/expected.txt`
//! is compared byte for byte against both implementations, and the Python one
//! formats offending values with `{v!r}`. Reproducing that is the difference
//! between the two linters being interchangeable and merely similar.

/// `repr()` of a Python `str`.
///
/// Single quotes, unless the string contains one and no double quote - which
/// is exactly CPython's rule. Escapes are the ones CPython emits for the
/// characters that can appear in a data set: backslash, the quote in use,
/// and the three whitespace escapes. Anything else non-printable falls back
/// to `\xNN` / `\uXXXX`, again as CPython writes it.
pub fn string(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `repr()` of a parsed TOML value, for the value the schema check rejected.
pub fn toml_value(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => string(s),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => float(*f),
        toml::Value::Boolean(b) => {
            if *b {
                "True".into()
            } else {
                "False".into()
            }
        }
        // tomllib hands these back as datetime objects; their repr is a
        // constructor call. A schema that reaches here has already reported
        // the type mismatch, so this only has to be recognisable.
        toml::Value::Datetime(d) => format!("datetime({d})"),
        toml::Value::Array(a) => {
            let items: Vec<String> = a.iter().map(toml_value).collect();
            format!("[{}]", items.join(", "))
        }
        toml::Value::Table(t) => {
            let items: Vec<String> = t
                .iter()
                .map(|(k, v)| format!("{}: {}", string(k), toml_value(v)))
                .collect();
            format!("{{{}}}", items.join(", "))
        }
    }
}

/// `str()` of a Python list of JSON values, which for a list is its `repr()`.
///
/// Used for the `enum` in `'x' not in ['high', 'medium', 'low']`.
pub fn json_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "None".into(),
        serde_json::Value::Bool(true) => "True".into(),
        serde_json::Value::Bool(false) => "False".into(),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => i.to_string(),
            None => float(n.as_f64().unwrap_or(f64::NAN)),
        },
        serde_json::Value::String(s) => string(s),
        serde_json::Value::Array(a) => {
            let items: Vec<String> = a.iter().map(json_value).collect();
            format!("[{}]", items.join(", "))
        }
        serde_json::Value::Object(o) => {
            let items: Vec<String> = o
                .iter()
                .map(|(k, v)| format!("{}: {}", string(k), json_value(v)))
                .collect();
            format!("{{{}}}", items.join(", "))
        }
    }
}

/// `repr()` of a Python `float`: shortest round-tripping form, always with a
/// decimal point so it cannot be read back as an `int`.
fn float(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let s = format!("{f}");
    if s.contains(['.', 'e', 'E']) {
        s
    } else {
        format!("{s}.0")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_get_python_quoting() {
        assert_eq!(string("plain"), "'plain'");
        assert_eq!(string("it's"), "\"it's\"");
        assert_eq!(string("both ' and \""), "'both \\' and \"'");
        assert_eq!(string("a\nb"), "'a\\nb'");
        assert_eq!(string("back\\slash"), "'back\\\\slash'");
        assert_eq!(string("\u{1}"), "'\\x01'");
    }

    #[test]
    fn enum_lists_read_like_python_lists() {
        let v: serde_json::Value = serde_json::json!(["high", "medium", "low"]);
        assert_eq!(json_value(&v), "['high', 'medium', 'low']");
    }

    #[test]
    fn scalars_match_their_python_spelling() {
        assert_eq!(toml_value(&toml::Value::Integer(870780)), "870780");
        assert_eq!(toml_value(&toml::Value::Boolean(true)), "True");
        assert_eq!(toml_value(&toml::Value::Float(1.0)), "1.0");
    }
}
