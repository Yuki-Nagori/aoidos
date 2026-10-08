//! 共用编码与规范化入口；错误不携带输入，普通编码保持 serde 字段顺序。

use crate::Error;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

/// 对已经解析的值做类型转换；无法重新检查原文本中的重复键。
/// # Errors
/// 字段类型 / schema 不合法为 InvalidShape。
pub fn from_value<T: DeserializeOwned>(value: Value) -> Result<T, Error> {
    serde_json::from_value(value).map_err(Error::invalid_shape)
}
/// 将结构转换为 JSON 值；字段协议仍由调用方的 Serialize 实现定义。
/// # Errors
/// 不合法的对象键或序列化器失败为 InvalidEncoding。
pub fn to_value<T: Serialize>(value: T) -> Result<Value, Error> {
    serde_json::to_value(value).map_err(Error::invalid_encoding)
}
/// 借用平台已经解析的 JSON 值，避免为了 DTO 解码复制完整参数。
/// # Errors
/// 字段类型 / schema 不合法为 InvalidShape；重复键须在原文本解码时检查。
pub fn decode_value<T: DeserializeOwned>(value: &Value) -> Result<T, Error> {
    serde::Deserialize::deserialize(value).map_err(Error::invalid_shape)
}
/// 紧凑编码，不重排字段、不附加换行；JSONL 和摘要协议由调用方处理。
/// # Errors
/// 序列化器失败为 InvalidEncoding。
pub fn to_string<T: Serialize + ?Sized>(value: &T) -> Result<String, Error> {
    serde_json::to_string(value).map_err(Error::invalid_encoding)
}
/// 直接生成 UTF-8 字节，供请求或原子写使用；不负责文件 / 网络操作。
/// # Errors
/// 序列化器失败为 InvalidEncoding。
pub fn to_vec<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, Error> {
    serde_json::to_vec(value).map_err(Error::invalid_encoding)
}
/// 递归按对象键排序后紧凑编码；数组顺序和字符串原文保持不变。
/// 不作为已有物理记录的重编码或导出规则，也不自行添加 LF / 计算 hash。
/// # Errors
/// 结构转换或序列化失败为 InvalidEncoding。
pub fn canonical_string<T: Serialize + ?Sized>(value: &T) -> Result<String, Error> {
    let mut value = to_value(value)?;
    value.sort_all_objects();
    to_string(&value)
}

#[cfg(test)]
mod tests;
