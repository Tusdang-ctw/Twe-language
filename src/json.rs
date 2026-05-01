//! Minimal JSON parser + emitter, no dependencies.
//!
//! The LSP wire format is JSON-RPC over stdio — the server has to
//! parse arbitrary client-side requests and emit replies. We keep
//! things zero-dep (matches `ast_json.rs`'s hand-rolled emitter)
//! and ship just enough surface for the LSP's actual message
//! shapes: objects, arrays, strings (with `\\`/`\"`/`\n`/etc.
//! escapes + `\uXXXX`), numbers (i64 and f64), bools, null.
//!
//! The parser is single-pass + non-validating in places — it
//! trusts the LSP client's framing. Bad JSON returns an Err with
//! a byte offset + message; LSP-side, that becomes a hard error
//! and we abort. For an editor doing `didChange` thousands of
//! times that's fine; correctness over polish.

use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub offset: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "json: at byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Parse a JSON text into a `Value`. Trailing whitespace is allowed.
pub fn parse(src: &str) -> Result<Value, ParseError> {
    let bytes = src.as_bytes();
    let mut p = Parser { bytes, pos: 0 };
    p.skip_ws();
    let value = p.parse_value()?;
    p.skip_ws();
    if p.pos != bytes.len() {
        return Err(p.err("unexpected trailing data"));
    }
    Ok(value)
}

/// Emit a `Value` as compact JSON — no extra whitespace.
pub fn to_string(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value);
    out
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError {
            offset: self.pos,
            message: msg.into(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn expect_byte(&mut self, b: u8) -> Result<(), ParseError> {
        match self.peek() {
            Some(c) if c == b => {
                self.pos += 1;
                Ok(())
            }
            Some(c) => Err(self.err(format!("expected '{}', got '{}'", b as char, c as char))),
            None => Err(self.err(format!("expected '{}', got EOF", b as char))),
        }
    }

    fn parse_value(&mut self) -> Result<Value, ParseError> {
        self.skip_ws();
        let b = self
            .peek()
            .ok_or_else(|| self.err("expected a value, got EOF"))?;
        match b {
            b'{' => self.parse_object(),
            b'[' => self.parse_array(),
            b'"' => self.parse_string().map(Value::Str),
            b't' | b'f' => self.parse_bool(),
            b'n' => self.parse_null(),
            b'-' | b'0'..=b'9' => self.parse_number(),
            other => Err(self.err(format!("unexpected byte 0x{other:02x}"))),
        }
    }

    fn parse_object(&mut self) -> Result<Value, ParseError> {
        self.expect_byte(b'{')?;
        let mut map = BTreeMap::new();
        self.skip_ws();
        if matches!(self.peek(), Some(b'}')) {
            self.pos += 1;
            return Ok(Value::Object(map));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect_byte(b':')?;
            self.skip_ws();
            let value = self.parse_value()?;
            map.insert(key, value);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Object(map));
                }
                _ => return Err(self.err("expected ',' or '}' in object")),
            }
        }
    }

    fn parse_array(&mut self) -> Result<Value, ParseError> {
        self.expect_byte(b'[')?;
        let mut elems = Vec::new();
        self.skip_ws();
        if matches!(self.peek(), Some(b']')) {
            self.pos += 1;
            return Ok(Value::Array(elems));
        }
        loop {
            self.skip_ws();
            elems.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Array(elems));
                }
                _ => return Err(self.err("expected ',' or ']' in array")),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, ParseError> {
        self.expect_byte(b'"')?;
        let mut out = String::new();
        loop {
            let b = self.peek().ok_or_else(|| self.err("unterminated string"))?;
            match b {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let esc = self.peek().ok_or_else(|| self.err("unterminated escape"))?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{08}'),
                        b'f' => out.push('\u{0c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            // \uXXXX. Surrogate pairs not handled —
                            // valid for the BMP-only LSP messages we
                            // see in practice.
                            if self.pos + 4 > self.bytes.len() {
                                return Err(self.err("truncated \\uXXXX escape"));
                            }
                            let mut n: u32 = 0;
                            for _ in 0..4 {
                                let h = self.bytes[self.pos];
                                self.pos += 1;
                                let v = match h {
                                    b'0'..=b'9' => (h - b'0') as u32,
                                    b'a'..=b'f' => (h - b'a' + 10) as u32,
                                    b'A'..=b'F' => (h - b'A' + 10) as u32,
                                    _ => return Err(self.err("bad hex in \\u escape")),
                                };
                                n = n * 16 + v;
                            }
                            if let Some(c) = char::from_u32(n) {
                                out.push(c);
                            } else {
                                out.push('\u{fffd}');
                            }
                        }
                        other => {
                            return Err(self.err(format!("unknown escape '\\{}'", other as char)));
                        }
                    }
                }
                _ => {
                    // Read until next " or \. Push raw bytes which
                    // are valid UTF-8 in the source.
                    let start = self.pos;
                    while let Some(c) = self.peek() {
                        if c == b'"' || c == b'\\' {
                            break;
                        }
                        self.pos += 1;
                    }
                    out.push_str(std::str::from_utf8(&self.bytes[start..self.pos]).map_err(
                        |_| ParseError {
                            offset: start,
                            message: "string contains invalid UTF-8".to_string(),
                        },
                    )?);
                }
            }
        }
    }

    fn parse_bool(&mut self) -> Result<Value, ParseError> {
        if self.bytes[self.pos..].starts_with(b"true") {
            self.pos += 4;
            Ok(Value::Bool(true))
        } else if self.bytes[self.pos..].starts_with(b"false") {
            self.pos += 5;
            Ok(Value::Bool(false))
        } else {
            Err(self.err("expected `true` or `false`"))
        }
    }

    fn parse_null(&mut self) -> Result<Value, ParseError> {
        if self.bytes[self.pos..].starts_with(b"null") {
            self.pos += 4;
            Ok(Value::Null)
        } else {
            Err(self.err("expected `null`"))
        }
    }

    fn parse_number(&mut self) -> Result<Value, ParseError> {
        let start = self.pos;
        if matches!(self.peek(), Some(b'-')) {
            self.pos += 1;
        }
        while let Some(b) = self.peek() {
            if b.is_ascii_digit() {
                self.pos += 1;
            } else {
                break;
            }
        }
        let mut is_float = false;
        if matches!(self.peek(), Some(b'.')) {
            is_float = true;
            self.pos += 1;
            while let Some(b) = self.peek() {
                if b.is_ascii_digit() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.pos += 1;
            }
            while let Some(b) = self.peek() {
                if b.is_ascii_digit() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        let raw = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| self.err("invalid utf-8 in number"))?;
        if is_float {
            raw.parse::<f64>()
                .map(Value::Float)
                .map_err(|_| self.err("invalid float"))
        } else {
            raw.parse::<i64>()
                .map(Value::Int)
                .map_err(|_| self.err("invalid integer"))
        }
    }
}

fn write_value(out: &mut String, v: &Value) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(n) => {
            let _ = write!(out, "{n}");
        }
        Value::Float(x) => {
            // Use {:?} for the shortest round-trip; LSP accepts.
            let _ = write!(out, "{x:?}");
        }
        Value::Str(s) => write_string(out, s),
        Value::Array(elems) => {
            out.push('[');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(out, e);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(out, k);
                out.push(':');
                write_value(out, v);
            }
            out.push('}');
        }
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

// --- ergonomic accessors used by the LSP ---

impl Value {
    /// Lookup `key` in an object value; returns `None` for non-objects.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(map) => map.get(key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
}

// --- builder ergonomics for emit-side ---

/// Build a JSON object from a sequence of `(key, value)` pairs.
pub fn obj<I>(pairs: I) -> Value
where
    I: IntoIterator<Item = (&'static str, Value)>,
{
    Value::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(src: &str) {
        let v = parse(src).expect("parse");
        let out = to_string(&v);
        let v2 = parse(&out).expect("re-parse");
        assert_eq!(v, v2, "round-trip changed value: src={src}, out={out}");
    }

    #[test]
    fn parses_null_bool_int_float() {
        assert_eq!(parse("null").unwrap(), Value::Null);
        assert_eq!(parse("true").unwrap(), Value::Bool(true));
        assert_eq!(parse("false").unwrap(), Value::Bool(false));
        assert_eq!(parse("42").unwrap(), Value::Int(42));
        assert_eq!(parse("-7").unwrap(), Value::Int(-7));
        assert_eq!(parse("1.5").unwrap(), Value::Float(1.5));
        assert_eq!(parse("1e3").unwrap(), Value::Float(1000.0));
        assert_eq!(parse("-2.5e-3").unwrap(), Value::Float(-0.0025));
    }

    #[test]
    fn parses_strings_with_escapes() {
        assert_eq!(parse(r#""hi""#).unwrap(), Value::Str("hi".into()));
        assert_eq!(
            parse(r#""\"\\\n\t""#).unwrap(),
            Value::Str("\"\\\n\t".into()),
        );
        assert_eq!(parse(r#""é""#).unwrap(), Value::Str("é".into()));
    }

    #[test]
    fn parses_arrays_and_objects() {
        assert_eq!(parse("[]").unwrap(), Value::Array(vec![]));
        assert_eq!(
            parse("[1, 2, 3]").unwrap(),
            Value::Array(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
        );
        let obj_v = parse(r#"{"a": 1, "b": "hi"}"#).unwrap();
        assert_eq!(obj_v.get("a"), Some(&Value::Int(1)));
        assert_eq!(obj_v.get("b").and_then(|v| v.as_str()), Some("hi"));
    }

    #[test]
    fn round_trips_nested() {
        round_trip(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#,
        );
        round_trip(r#"{"a":[1,2,{"b":null}]}"#);
        round_trip(r#""line\nbreak\twith\\backslash""#);
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert!(parse("123 trailing").is_err());
    }

    #[test]
    fn rejects_unterminated_string() {
        assert!(parse(r#""no end"#).is_err());
    }

    #[test]
    fn obj_helper_builds_objects() {
        let v = obj([("a", Value::Int(1)), ("b", Value::Bool(true))]);
        let s = to_string(&v);
        // BTreeMap orders alphabetically.
        assert_eq!(s, r#"{"a":1,"b":true}"#);
    }
}
