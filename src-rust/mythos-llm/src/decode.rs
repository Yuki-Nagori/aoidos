//! 增量 UTF-8 解码：按任意字节边界喂入网络分片，扣留不完整的多字节尾部。
//!
//! 异常 EOF 时扣留的尾部不得合成为 U+FFFD 交付（设计见 ai-docs/architecture/llm.md 跨分片算法），
//! [`IncrementalUtf8::finish`] 对残留尾部报 [`InvalidUtf8`]，由上层归入 `llm.bad-response`。

/// 字节序列不是合法 UTF-8；截断的最后字符不会以替换符交付。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidUtf8;

impl std::fmt::Display for InvalidUtf8 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "byte stream is not valid utf-8")
    }
}

impl std::error::Error for InvalidUtf8 {}

fn decode_prefix(bytes: &[u8]) -> String {
    // 调用方只传入 from_utf8 已确认合法的前缀。
    std::str::from_utf8(bytes)
        .expect("validated prefix")
        .to_owned()
}

/// 流式 UTF-8 解码器。每次 [`feed`](IncrementalUtf8::feed) 返回本轮可完整解码的文本；
/// 输入按 [`str::from_utf8`] 的规则校验，多字节序列被网络包切断时留在内部缓冲。
#[derive(Debug, Default)]
pub struct IncrementalUtf8 {
    pending: Vec<u8>,
}

impl IncrementalUtf8 {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一段网络字节，返回已完整解码的文本（可为空）。
    ///
    /// # Errors
    /// 遇到确定的非法字节序列时返回 [`InvalidUtf8`]；此后解码器状态不可再用，调用方应中止本次流。
    pub fn feed(&mut self, chunk: &[u8]) -> Result<String, InvalidUtf8> {
        if self.pending.is_empty() {
            let valid = match std::str::from_utf8(chunk) {
                Ok(text) => return Ok(text.to_owned()),
                Err(error) => error,
            };
            let valid_up_to = valid.valid_up_to();
            match valid.error_len() {
                None => {
                    // 尾部是不完整的多字节前缀，留给下一分片。
                    self.pending.extend_from_slice(&chunk[valid_up_to..]);
                    Ok(decode_prefix(&chunk[..valid_up_to]))
                }
                Some(_) => Err(InvalidUtf8),
            }
        } else {
            self.pending.extend_from_slice(chunk);
            let valid = match std::str::from_utf8(&self.pending) {
                Ok(_) => {
                    let taken = std::mem::take(&mut self.pending);
                    return Ok(String::from_utf8(taken).expect("pending validated as utf-8"));
                }
                Err(error) => error,
            };
            let valid_up_to = valid.valid_up_to();
            let decoded = decode_prefix(&self.pending[..valid_up_to]);
            self.pending.drain(..valid_up_to);
            match valid.error_len() {
                None => Ok(decoded),
                Some(_) => Err(InvalidUtf8),
            }
        }
    }

    /// 流结束：正常结束时不得有扣留的字节。
    ///
    /// # Errors
    /// 仍有不完整的多字节尾部时返回 [`InvalidUtf8`]（异常 EOF，尾部不交付）。
    pub fn finish(&mut self) -> Result<(), InvalidUtf8> {
        if self.pending.is_empty() {
            Ok(())
        } else {
            Err(InvalidUtf8)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_passes_through() {
        let mut decoder = IncrementalUtf8::new();
        assert_eq!(decoder.feed(b"hello").unwrap(), "hello");
        assert!(decoder.finish().is_ok());
    }

    #[test]
    fn empty_chunk_is_legal() {
        let mut decoder = IncrementalUtf8::new();
        assert_eq!(decoder.feed(b"").unwrap(), "");
        assert!(decoder.finish().is_ok());
    }

    #[test]
    fn multibyte_split_at_every_byte_position_yields_same_text() {
        // 中文 3 字节 + emoji 4 字节 + ASCII，覆盖各类切割点。
        let whole = "风渡🌉end".as_bytes();
        for cut in 0..=whole.len() {
            let mut decoder = IncrementalUtf8::new();
            let first = decoder.feed(&whole[..cut]).unwrap();
            let second = decoder.feed(&whole[cut..]).unwrap();
            assert_eq!(format!("{first}{second}"), "风渡🌉end", "cut at {cut}");
            assert!(decoder.finish().is_ok(), "cut at {cut}");
        }
    }

    #[test]
    fn three_way_split_accumulates_pending() {
        let whole = " 夜航🌟".as_bytes();
        for a in 0..=whole.len() {
            for b in a..=whole.len() {
                let mut decoder = IncrementalUtf8::new();
                let joined = format!(
                    "{}{}{}",
                    decoder.feed(&whole[..a]).unwrap(),
                    decoder.feed(&whole[a..b]).unwrap(),
                    decoder.feed(&whole[b..]).unwrap(),
                );
                assert_eq!(joined, " 夜航🌟", "cut at {a}/{b}");
            }
        }
    }

    #[test]
    fn invalid_sequence_is_rejected() {
        let mut decoder = IncrementalUtf8::new();
        assert_eq!(decoder.feed(b"ok").unwrap(), "ok");
        assert_eq!(decoder.feed(b"\xff").unwrap_err(), InvalidUtf8);
    }

    #[test]
    fn invalid_sequence_after_pending_is_rejected() {
        let mut decoder = IncrementalUtf8::new();
        // 先扣留多字节前缀，再喂入非法续字节。
        assert_eq!(decoder.feed("船".as_bytes()[..2].into()).unwrap(), "");
        assert_eq!(decoder.feed(b"\x28").unwrap_err(), InvalidUtf8);
    }

    #[test]
    fn truncated_continuation_is_invalid() {
        let mut decoder = IncrementalUtf8::new();
        // 合法两字节序列被换成非法续字节。
        assert_eq!(decoder.feed(b"\xc3\x28").unwrap_err(), InvalidUtf8);
    }

    #[test]
    fn pending_tail_at_eof_is_an_error_not_replacement() {
        let mut decoder = IncrementalUtf8::new();
        assert_eq!(decoder.feed("船".as_bytes()[..2].into()).unwrap(), "");
        // 扣留的多字节前缀在 EOF 不得转换成 U+FFFD 交付。
        assert_eq!(decoder.finish().unwrap_err(), InvalidUtf8);
    }

    #[test]
    fn complete_sequence_flushes_pending() {
        let bytes = "灯".as_bytes();
        let mut decoder = IncrementalUtf8::new();
        assert_eq!(decoder.feed(&bytes[..1]).unwrap(), "");
        assert_eq!(decoder.feed(&bytes[1..]).unwrap(), "灯");
        assert!(decoder.finish().is_ok());
    }

    #[test]
    fn display_and_error_trait_are_implemented() {
        let error: &dyn std::error::Error = &InvalidUtf8;
        assert!(!error.to_string().is_empty());
    }
}
