// SPDX-License-Identifier: MIT
use crate::Result;
use std::collections::BTreeMap;
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}
impl Value {
    pub fn get(&self, key: &str) -> Result<&Value> {
        match self {
            Self::Object(m) => m.get(key).ok_or_else(|| format!("missing {key}")),
            _ => Err("expected object".into()),
        }
    }
    pub fn string(&self) -> Result<&str> {
        if let Self::Str(s) = self {
            Ok(s)
        } else {
            Err("expected string".into())
        }
    }
    pub fn integer(&self) -> Result<i64> {
        if let Self::Int(n) = self {
            Ok(*n)
        } else {
            Err("expected integer".into())
        }
    }
    pub fn boolean(&self) -> Result<bool> {
        if let Self::Bool(n) = self {
            Ok(*n)
        } else {
            Err("expected boolean".into())
        }
    }
    pub fn array(&self) -> Result<&[Value]> {
        if let Self::Array(a) = self {
            Ok(a)
        } else {
            Err("expected array".into())
        }
    }
    pub fn encode(&self) -> String {
        match self {
            Self::Null => "null".into(),
            Self::Bool(b) => b.to_string(),
            Self::Int(n) => n.to_string(),
            Self::Str(s) => quote(s),
            Self::Array(a) => format!(
                "[{}]",
                a.iter().map(Value::encode).collect::<Vec<_>>().join(",")
            ),
            Self::Object(m) => format!(
                "{{{}}}",
                m.iter()
                    .map(|(k, v)| format!("{}:{}", quote(k), v.encode()))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }
}
pub fn object(fields: &[(&str, Value)]) -> Value {
    Value::Object(
        fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}
pub fn quote(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if c < ' ' => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}
pub fn parse(s: &str) -> Result<Value> {
    let mut p = Parser { s, at: 0 };
    let v = p.value(0)?;
    p.space();
    if p.at != s.len() {
        return Err("trailing JSON data".into());
    }
    Ok(v)
}
struct Parser<'a> {
    s: &'a str,
    at: usize,
}
impl Parser<'_> {
    fn space(&mut self) {
        while self
            .s
            .as_bytes()
            .get(self.at)
            .is_some_and(|b| b.is_ascii_whitespace())
        {
            self.at += 1;
        }
    }
    fn byte(&mut self) -> Result<u8> {
        let b = *self.s.as_bytes().get(self.at).ok_or("truncated JSON")?;
        self.at += 1;
        Ok(b)
    }
    fn expect(&mut self, b: u8) -> Result<()> {
        if self.byte()? == b {
            Ok(())
        } else {
            Err("invalid JSON delimiter".into())
        }
    }
    fn hex(&mut self) -> Result<u32> {
        let end = self.at.checked_add(4).ok_or("invalid escape")?;
        let s = self.s.get(self.at..end).ok_or("invalid unicode escape")?;
        let n = u32::from_str_radix(s, 16).map_err(|_| "invalid unicode escape")?;
        self.at = end;
        Ok(n)
    }
    fn string(&mut self) -> Result<String> {
        self.expect(b'"')?;
        let mut s = String::new();
        loop {
            match self.byte()? {
                b'"' => return Ok(s),
                b'\\' => match self.byte()? {
                    b'"' => s.push('"'),
                    b'\\' => s.push('\\'),
                    b'/' => s.push('/'),
                    b'b' => s.push('\x08'),
                    b'f' => s.push('\x0c'),
                    b'n' => s.push('\n'),
                    b'r' => s.push('\r'),
                    b't' => s.push('\t'),
                    b'u' => {
                        let mut n = self.hex()?;
                        if (0xd800..=0xdbff).contains(&n) {
                            self.expect(b'\\')?;
                            self.expect(b'u')?;
                            let low = self.hex()?;
                            if !(0xdc00..=0xdfff).contains(&low) {
                                return Err("invalid surrogate".into());
                            }
                            n = 0x10000 + ((n - 0xd800) << 10) + (low - 0xdc00);
                        }
                        s.push(char::from_u32(n).ok_or("invalid unicode scalar")?);
                    }
                    _ => return Err("invalid escape".into()),
                },
                b if b < 32 => return Err("JSON control character".into()),
                _ => {
                    self.at -= 1;
                    let c = self.s[self.at..].chars().next().ok_or("truncated string")?;
                    self.at += c.len_utf8();
                    s.push(c);
                }
            }
        }
    }
    fn value(&mut self, depth: usize) -> Result<Value> {
        if depth > 24 {
            return Err("JSON nesting limit".into());
        }
        self.space();
        match self
            .s
            .as_bytes()
            .get(self.at)
            .copied()
            .ok_or("empty JSON")?
        {
            b'"' => Ok(Value::Str(self.string()?)),
            b'[' => {
                self.at += 1;
                let mut a = vec![];
                self.space();
                if self.s.as_bytes().get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Ok(Value::Array(a));
                }
                loop {
                    a.push(self.value(depth + 1)?);
                    self.space();
                    match self.byte()? {
                        b']' => break,
                        b',' => {}
                        _ => return Err("invalid array".into()),
                    }
                }
                Ok(Value::Array(a))
            }
            b'{' => {
                self.at += 1;
                let mut m = BTreeMap::new();
                self.space();
                if self.s.as_bytes().get(self.at) == Some(&b'}') {
                    self.at += 1;
                    return Ok(Value::Object(m));
                }
                loop {
                    self.space();
                    let k = self.string()?;
                    self.space();
                    self.expect(b':')?;
                    let v = self.value(depth + 1)?;
                    if m.insert(k, v).is_some() {
                        return Err("duplicate JSON key".into());
                    }
                    self.space();
                    match self.byte()? {
                        b'}' => break,
                        b',' => {}
                        _ => return Err("invalid object".into()),
                    }
                }
                Ok(Value::Object(m))
            }
            b't' | b'f' | b'n' => {
                for (word, value) in [
                    ("true", Value::Bool(true)),
                    ("false", Value::Bool(false)),
                    ("null", Value::Null),
                ] {
                    if self.s[self.at..].starts_with(word) {
                        self.at += word.len();
                        return Ok(value);
                    }
                }
                Err("invalid JSON literal".into())
            }
            b'-' | b'0'..=b'9' => {
                let start = self.at;
                if self.s.as_bytes()[self.at] == b'-' {
                    self.at += 1;
                }
                let first = self.byte()?;
                if !first.is_ascii_digit() {
                    return Err("invalid number".into());
                }
                if first != b'0' {
                    while self
                        .s
                        .as_bytes()
                        .get(self.at)
                        .is_some_and(u8::is_ascii_digit)
                    {
                        self.at += 1;
                    }
                }
                self.s[start..self.at]
                    .parse()
                    .map(Value::Int)
                    .map_err(|_| "integer out of range".into())
            }
            _ => Err("invalid JSON value".into()),
        }
    }
}
#[cfg(test)]
#[path = "json_tests.rs"]
mod tests;
