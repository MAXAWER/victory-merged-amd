//! Just enough JSON to read a glTF file's header: values, with no schema.

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Value {
    Null,
    Bool(bool),
    Number(f64),
    Text(String),
    List(Vec<Value>),
    Map(Vec<(String, Value)>),
}

impl Value {
    pub(super) fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub(super) fn at(&self, index: usize) -> Option<&Value> {
        match self {
            Value::List(items) => items.get(index),
            _ => None,
        }
    }

    pub(super) fn list(&self) -> &[Value] {
        match self {
            Value::List(items) => items,
            _ => &[],
        }
    }

    pub(super) fn number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub(super) fn index(&self) -> Option<usize> {
        self.number().filter(|n| *n >= 0.0 && n.fract() == 0.0).map(|n| n as usize)
    }

    pub(super) fn text(&self) -> Option<&str> {
        match self {
            Value::Text(t) => Some(t),
            _ => None,
        }
    }

    /// The numbers of a list of numbers; `None` if it is anything else.
    pub(super) fn numbers(&self) -> Option<Vec<f32>> {
        self.list().iter().map(|v| v.number().map(|n| n as f32)).collect::<Option<Vec<_>>>().filter(|_| matches!(self, Value::List(_)))
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn space(&mut self) {
        while self.bytes.get(self.at).is_some_and(|b| b.is_ascii_whitespace()) {
            self.at += 1;
        }
    }

    fn take(&mut self, byte: u8) -> Option<()> {
        self.space();
        (self.bytes.get(self.at) == Some(&byte)).then(|| self.at += 1)
    }

    fn word(&mut self, word: &str, value: Value) -> Option<Value> {
        self.bytes[self.at..].starts_with(word.as_bytes()).then(|| {
            self.at += word.len();
            value
        })
    }

    fn string(&mut self) -> Option<String> {
        self.take(b'"')?;
        let mut out = Vec::new();
        loop {
            let b = *self.bytes.get(self.at)?;
            self.at += 1;
            match b {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let e = *self.bytes.get(self.at)?;
                    self.at += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let hex = std::str::from_utf8(self.bytes.get(self.at..self.at + 4)?).ok()?;
                            self.at += 4;
                            let c = char::from_u32(u32::from_str_radix(hex, 16).ok()?).unwrap_or('?');
                            out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
                        }
                        other => out.push(other),
                    }
                }
                other => out.push(other),
            }
        }
    }

    fn value(&mut self, depth: u32) -> Option<Value> {
        if depth > 64 {
            return None;
        }
        self.space();
        match *self.bytes.get(self.at)? {
            b'{' => {
                self.at += 1;
                let mut pairs = Vec::new();
                if self.take(b'}').is_some() {
                    return Some(Value::Map(pairs));
                }
                loop {
                    let key = self.string()?;
                    self.take(b':')?;
                    pairs.push((key, self.value(depth + 1)?));
                    if self.take(b',').is_none() {
                        self.take(b'}')?;
                        return Some(Value::Map(pairs));
                    }
                }
            }
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                if self.take(b']').is_some() {
                    return Some(Value::List(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    if self.take(b',').is_none() {
                        self.take(b']')?;
                        return Some(Value::List(items));
                    }
                }
            }
            b'"' => self.string().map(Value::Text),
            b't' => self.word("true", Value::Bool(true)),
            b'f' => self.word("false", Value::Bool(false)),
            b'n' => self.word("null", Value::Null),
            _ => {
                let from = self.at;
                while self.bytes.get(self.at).is_some_and(|b| matches!(b, b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')) {
                    self.at += 1;
                }
                std::str::from_utf8(&self.bytes[from..self.at]).ok()?.parse::<f64>().ok().map(Value::Number)
            }
        }
    }
}

/// The value `bytes` spell, if they spell one and nothing else.
pub(super) fn parse(bytes: &[u8]) -> Option<Value> {
    let mut reader = Reader { bytes, at: 0 };
    let value = reader.value(0)?;
    reader.space();
    (reader.at == bytes.len()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_read() {
        let v = parse(br#" {"a": [1, -2.5e1, "x\"yA"], "b": {"c": true, "d": null}, "e": []} "#).unwrap();
        assert_eq!(v.get("a").and_then(|a| a.at(1)).and_then(Value::number), Some(-25.0));
        assert_eq!(v.get("a").and_then(|a| a.at(2)).and_then(Value::text), Some("x\"yA"));
        assert_eq!(v.get("b").and_then(|b| b.get("c")), Some(&Value::Bool(true)));
        assert_eq!(v.get("e").map(Value::list).map(<[Value]>::len), Some(0));
        assert_eq!(parse(b"[1, 2").is_none() && parse(b"{} x").is_none(), true);
        assert_eq!(parse(b"[1, 2.5]").unwrap().numbers(), Some(vec![1.0, 2.5]));
        assert_eq!(parse(b"[3]").unwrap().at(0).and_then(Value::index), Some(3));
    }
}
