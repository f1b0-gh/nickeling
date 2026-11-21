//! Bounded JSON with duplicate-key rejection; used for state and untrusted transport.
use crate::error::{Error, Result};
use serde::de::{DeserializeSeed, Deserializer, Error as DeError, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
struct Seed(usize, usize);
impl<'de> DeserializeSeed<'de> for Seed {
    type Value = Value;
    fn deserialize<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Value, D::Error> {
        if self.0 > 64 {
            return Err(D::Error::custom("JSON nesting limit exceeded"));
        }
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Seed {
    type Value = Value;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("bounded JSON without duplicate keys")
    }
    fn visit_bool<E: DeError>(self, v: bool) -> std::result::Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E: DeError>(self, v: i64) -> std::result::Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_u64<E: DeError>(self, v: u64) -> std::result::Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_f64<E: DeError>(self, v: f64) -> std::result::Result<Value, E> {
        Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite number"))
    }
    fn visit_str<E: DeError>(self, v: &str) -> std::result::Result<Value, E> {
        Ok(Value::String(v.into()))
    }
    fn visit_string<E: DeError>(self, v: String) -> std::result::Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_unit<E: DeError>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(Seed(self.0 + 1, self.1))? {
            if values.len() >= self.1 {
                return Err(A::Error::custom("array item limit exceeded"));
            }
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.len() >= 10000 {
                return Err(A::Error::custom("object field limit exceeded"));
            }
            if values.contains_key(&key) {
                return Err(A::Error::custom("duplicate JSON key"));
            }
            let value = map.next_value_seed(Seed(self.0 + 1, self.1))?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}
pub fn parse(bytes: &[u8]) -> Result<Value> {
    parse_with_limits(bytes, 32 * 1024 * 1024, 10_000)
}

/// Parse with caller-selected byte and array budgets. Duplicate keys, nesting
/// beyond 64 levels and objects beyond 10,000 fields are always refused.
/// Existing callers retain the conservative defaults through `parse`.
pub fn parse_with_limits(bytes: &[u8], max_bytes: usize, max_array_items: usize) -> Result<Value> {
    if bytes.len() > max_bytes {
        return Err(Error::InvalidInput {
            message: format!("JSON exceeds {max_bytes} bytes"),
        });
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = Seed(0, max_array_items).deserialize(&mut decoder)?;
    decoder.end()?;
    Ok(value)
}
