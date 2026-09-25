//! A reader for the JavaScript literals Codex's code mode writes into `exec` inputs (R26): JSON plus unquoted keys,
//! single-quoted and template strings, trailing commas, comments and `undefined`.

use serde_json::{Map, Number, Value};

const MAX_DEPTH: usize = 64;

/// The literal starting at byte `start` of `src` and the byte offset just past it; `None` for anything that is not a literal.
pub(crate) fn parse_literal(src: &str, start: usize) -> Option<(Value, usize)> {
    let mut reader = Reader {
        src: src.as_bytes(),
        text: src,
        pos: start,
    };
    let value = reader.value(0)?;
    Some((value, reader.pos))
}

struct Reader<'a> {
    src: &'a [u8],
    text: &'a str,
    pos: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn skip_space(&mut self) {
        while let Some(b) = self.peek() {
            if b.is_ascii_whitespace() {
                self.pos += 1;
            } else if self.src[self.pos..].starts_with(b"//") {
                while self.peek().is_some_and(|b| b != b'\n') {
                    self.pos += 1;
                }
            } else if self.src[self.pos..].starts_with(b"/*") {
                let end = self.text[self.pos + 2..].find("*/");
                self.pos = end.map_or(self.src.len(), |e| self.pos + 2 + e + 2);
            } else {
                break;
            }
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        self.skip_space();
        if self.peek() == Some(byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.skip_space();
        match self.peek()? {
            b'{' => self.object(depth),
            b'[' => self.array(depth),
            b'"' | b'\'' | b'`' => self.string().map(Value::String),
            b'-' | b'+' | b'.' | b'0'..=b'9' => self.number(),
            _ => match self.ident()? {
                "true" => Some(Value::Bool(true)),
                "false" => Some(Value::Bool(false)),
                "null" | "undefined" => Some(Value::Null),
                _ => None,
            },
        }
    }

    fn object(&mut self, depth: usize) -> Option<Value> {
        self.pos += 1;
        let mut map = Map::new();
        loop {
            if self.eat(b'}') {
                return Some(Value::Object(map));
            }
            self.skip_space();
            let key = match self.peek()? {
                b'"' | b'\'' | b'`' => self.string()?,
                b'0'..=b'9' => self.number()?.to_string(),
                _ => self.ident()?.to_owned(),
            };
            if !self.eat(b':') {
                return None;
            }
            map.insert(key, self.value(depth + 1)?);
            if !self.eat(b',') {
                return self.eat(b'}').then_some(Value::Object(map));
            }
        }
    }

    fn array(&mut self, depth: usize) -> Option<Value> {
        self.pos += 1;
        let mut items = Vec::new();
        loop {
            if self.eat(b']') {
                return Some(Value::Array(items));
            }
            items.push(self.value(depth + 1)?);
            if !self.eat(b',') {
                return self.eat(b']').then_some(Value::Array(items));
            }
        }
    }

    fn ident(&mut self) -> Option<&str> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$')
        {
            self.pos += 1;
        }
        let ident = &self.text[start..self.pos];
        (!ident.is_empty() && !ident.as_bytes()[0].is_ascii_digit()).then_some(ident)
    }

    fn number(&mut self) -> Option<Value> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-' | b'_'))
        {
            self.pos += 1;
        }
        let raw = self.text[start..self.pos].replace('_', "");
        let raw = raw.strip_prefix('+').unwrap_or(&raw);
        if let Ok(n) = raw.parse::<i64>() {
            return Some(Value::Number(n.into()));
        }
        Number::from_f64(raw.parse::<f64>().ok()?).map(Value::Number)
    }

    fn string(&mut self) -> Option<String> {
        let quote = self.peek()?;
        self.pos += 1;
        let mut out = String::new();
        loop {
            let c = self.text[self.pos..].chars().next()?;
            self.pos += c.len_utf8();
            match c {
                '\\' => self.escape(&mut out)?,
                '$' if quote == b'`' && self.peek() == Some(b'{') => return None,
                '\n' | '\r' if quote != b'`' => return None,
                c if c as u32 == u32::from(quote) => return Some(out),
                c => out.push(c),
            }
        }
    }

    fn escape(&mut self, out: &mut String) -> Option<()> {
        let c = self.text[self.pos..].chars().next()?;
        self.pos += c.len_utf8();
        match c {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'v' => out.push('\u{b}'),
            '0' => out.push('\0'),
            '\n' => {}
            'x' => out.push(self.hex(2)?),
            'u' if self.peek() == Some(b'{') => {
                let end = self.text[self.pos..].find('}')?;
                let code =
                    u32::from_str_radix(&self.text[self.pos + 1..self.pos + end], 16).ok()?;
                self.pos += end + 1;
                out.push(char::from_u32(code)?);
            }
            'u' => {
                let high = self.hex_u16()?;
                let code = if (0xD800..0xDC00).contains(&high)
                    && self.text[self.pos..].starts_with("\\u")
                {
                    self.pos += 2;
                    let low = self.hex_u16()?;
                    0x10000
                        + ((u32::from(high) - 0xD800) << 10)
                        + (u32::from(low).checked_sub(0xDC00)?)
                } else {
                    u32::from(high)
                };
                out.push(char::from_u32(code)?);
            }
            c => out.push(c),
        }
        Some(())
    }

    fn hex_u16(&mut self) -> Option<u16> {
        let digits = self.text.get(self.pos..self.pos + 4)?;
        let value = u16::from_str_radix(digits, 16).ok()?;
        self.pos += 4;
        Some(value)
    }

    fn hex(&mut self, len: usize) -> Option<char> {
        let digits = self.text.get(self.pos..self.pos + len)?;
        let value = u32::from_str_radix(digits, 16).ok()?;
        self.pos += len;
        char::from_u32(value)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parse(src: &str) -> Option<Value> {
        parse_literal(src, 0).map(|(v, _)| v)
    }

    #[test]
    fn reads_the_code_mode_shape() {
        let src =
            r#"{plan:[{step:"add README",status:"in_progress"},{step:"commit",status:"pending"}]}"#;
        assert_eq!(
            parse(src).unwrap(),
            json!({"plan": [
                {"step": "add README", "status": "in_progress"},
                {"step": "commit", "status": "pending"}
            ]})
        );
    }

    #[test]
    fn reads_js_extras() {
        let src = "{ 'a': `tick`, b: [1, -2.5, true, null, undefined,], /* c */ c: \"\\u00e9\\n\", // tail\n }";
        assert_eq!(
            parse(src).unwrap(),
            json!({"a": "tick", "b": [1, -2.5, true, null, null], "c": "é\n"})
        );
        assert_eq!(parse("\"\\ud83d\\ude00\"").unwrap(), json!("\u{1F600}"));
        assert_eq!(parse(r#"'\u{1F600}'"#).unwrap(), json!("\u{1F600}"));
    }

    #[test]
    fn rejects_non_literals() {
        for src in [
            "plan", "{plan}", "{a: b}", "`${x}`", "{a: 1", "[1 2]", "'open", "{a: f()}",
        ] {
            assert!(parse(src).is_none(), "{src:?}");
        }
    }

    #[test]
    fn stops_at_the_end_of_the_literal() {
        let src = "({a: 1}); text(r);";
        let (value, end) = parse_literal(src, 1).unwrap();
        assert_eq!(value, json!({"a": 1}));
        assert_eq!(&src[end..], "); text(r);");
    }

    #[test]
    fn nesting_is_bounded() {
        let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        assert!(parse(&deep).is_none());
    }
}
