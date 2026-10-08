//! Token 估算器及版本登记；仅估算冻结输入，不修改正文或运行期系数。

use mythos_llm::provider::ProviderInput;

/// 估算版本显式登记，旧记录与已冻结请求不随默认值变化。
#[derive(Debug, Clone)]
pub enum EstimatorRevision {
    BaselineV1,
    ConservativeV2,
}
impl EstimatorRevision {
    pub fn version(&self) -> u32 {
        match self {
            Self::BaselineV1 => 1,
            Self::ConservativeV2 => 2,
        }
    }
    pub(crate) fn scale(&self, value: u32) -> u32 {
        match self {
            Self::BaselineV1 => value,
            Self::ConservativeV2 => (u64::from(value) * 125)
                .div_ceil(100)
                .min(u64::from(u32::MAX)) as u32,
        }
    }
}
/// v1 字符分类器：基础 / 扩展汉字按 Unicode 标量统计，封装一并估算。
pub fn estimate(text: &str, overhead: u32) -> u32 {
    let mut units = u64::from(overhead) * 10;
    for c in text.chars() {
        units += if c.is_ascii() {
            3
        } else if matches!(u32::from(c),0x3400..=0x4dbf|0x4e00..=0x9fff|0x20000..=0x323af) {
            6
        } else {
            10
        };
    }
    // 1.15 余量用有理整数避免平台浮点舍入差异。
    ((units * 115).div_ceil(1000)).min(u64::from(u32::MAX)) as u32
}
pub fn estimate_input(input: &ProviderInput) -> u32 {
    match input {
        ProviderInput::Completion(input) => estimate(&input.prompt, 0),
        ProviderInput::Chat(input) => estimate(
            &format!(
                "{}{}",
                input
                    .messages
                    .iter()
                    .map(|m| m.content.as_str())
                    .collect::<String>(),
                input.assistant_prefix.as_deref().unwrap_or("")
            ),
            12 + input.messages.len() as u32 * 8,
        ),
    }
}
