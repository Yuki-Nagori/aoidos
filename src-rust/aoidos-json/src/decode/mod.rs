//! 有界、拒绝重复键的 JSON 解码；文件行规则、字段协议与域错误由调用方负责。

use crate::Error;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::BTreeMap;

/// 消费一个完整 JSON 值，递归拒绝重复键；上限按原始 UTF-8 字节计。
/// 不要求末尾 LF，不追加或修正输入；递归深度沿用 serde_json 的默认限制。
/// # Errors
/// 零预算 / 超限为 SizeLimit；语法、重复键或尾随非空白为 InvalidJson。
pub fn parse(text: &str, max_bytes: usize) -> Result<Value, Error> {
    if max_bytes == 0 || text.len() > max_bytes {
        return Err(Error::SizeLimit);
    }
    let mut decoder = serde_json::Deserializer::from_str(text);
    let value = Unique::deserialize(&mut decoder)
        .map_err(Error::invalid_json)?
        .0;
    decoder.end().map_err(Error::invalid_json)?;
    Ok(value)
}

/// 严格解码后交给领域 DTO；是否允许未知字段由 DTO 自己声明。
/// # Errors
/// 除 parse 的错误外，字段类型 / schema 不符为 InvalidShape。
pub fn decode<T: DeserializeOwned>(text: &str, max_bytes: usize) -> Result<T, Error> {
    crate::from_value(parse(text, max_bytes)?)
}
/// 从有界 UTF-8 字节解码；不进行有损替换或文件读取。
/// # Errors
/// 除 decode 的错误外，非法 UTF-8 为 InvalidUtf8；超限先于编码校验拒绝。
pub fn decode_bytes<T: DeserializeOwned>(bytes: &[u8], max_bytes: usize) -> Result<T, Error> {
    if max_bytes == 0 || bytes.len() > max_bytes {
        return Err(Error::SizeLimit);
    }
    let text = std::str::from_utf8(bytes).map_err(Error::invalid_utf8)?;
    decode(text, max_bytes)
}

struct Unique(Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Unique;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON without duplicate keys")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Unique, A::Error> {
                let mut fields = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, Unique>()? {
                    if fields.insert(key, value.0).is_some() {
                        return Err(serde::de::Error::custom("duplicate key"));
                    }
                }
                Ok(Unique(Value::Object(fields.into_iter().collect())))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Unique, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<Unique>()? {
                    values.push(value.0);
                }
                Ok(Unique(Value::Array(values)))
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::Bool(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> std::result::Result<Unique, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| Unique(Value::Number(n)))
                    .ok_or_else(|| E::custom("nonfinite"))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
        }
        decoder.deserialize_any(Visitor)
    }
}

#[cfg(test)]
mod tests;
