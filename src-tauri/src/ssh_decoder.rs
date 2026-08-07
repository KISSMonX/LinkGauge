//! SSH 远端输出解码器：UTF-8 跨块拼接 + ANSI 转义序列剔除。
//!
//! 从 ssh.rs 拆分出的独立模块，Decoder 零外部依赖，可单测。
//! PTY 输出的颜色、光标定位等控制序列由 strip 逐字节剔除，
//! 只保留正文与 \r \n \b \t，前端的行缓冲负责回车覆盖与退格。

#[derive(Default, Clone, Copy, PartialEq)]
pub(crate) enum EscState {
    #[default]
    Text,
    /// 刚读到 ESC，等待后续类型字节
    Esc,
    /// CSI（ESC [ …）：终止于 0x40..=0x7E
    Csi,
    /// OSC / DCS / APC 等字符串型序列：终止于 BEL 或 ESC \
    Str,
    /// 字符串型序列中读到 ESC，等待 \
    StrEsc,
}

/// 远端字节流 → 可显示的纯文本。
///
/// 状态跨调用保留，序列被拆到多个块也能正确解析。
#[derive(Default)]
pub(crate) struct Decoder {
    /// 上一块结尾不完整的 UTF-8 字节
    pending: Vec<u8>,
    state: EscState,
}

impl Decoder {
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> String {
        self.pending.extend_from_slice(chunk);
        let mut decoded = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(text) => {
                    decoded.push_str(text);
                    self.pending.clear();
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    decoded.push_str(&String::from_utf8_lossy(&self.pending[..valid]));
                    match error.error_len() {
                        Some(len) => {
                            decoded.push('\u{FFFD}');
                            self.pending.drain(..valid + len);
                        }
                        None => {
                            self.pending.drain(..valid);
                            break;
                        }
                    }
                }
            }
        }
        self.strip(&decoded)
    }

    fn strip(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for ch in text.chars() {
            self.state = match self.state {
                EscState::Text => {
                    if ch == '\u{1b}' {
                        EscState::Esc
                    } else {
                        if ch == '\n' || ch == '\r' || ch == '\t' || ch == '\u{8}' || ch >= ' ' {
                            out.push(ch);
                        }
                        EscState::Text
                    }
                }
                EscState::Esc => match ch {
                    '[' => EscState::Csi,
                    ']' | 'P' | 'X' | '^' | '_' => EscState::Str,
                    _ => EscState::Text,
                },
                EscState::Csi => {
                    if ('\u{40}'..='\u{7e}').contains(&ch) {
                        EscState::Text
                    } else {
                        EscState::Csi
                    }
                }
                EscState::Str => match ch {
                    '\u{7}' => EscState::Text,
                    '\u{1b}' => EscState::StrEsc,
                    _ => EscState::Str,
                },
                EscState::StrEsc => {
                    if ch == '\\' {
                        EscState::Text
                    } else {
                        EscState::Str
                    }
                }
            };
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::Decoder;

    #[test]
    fn keeps_plain_text_and_line_control_chars() {
        let mut decoder = Decoder::default();
        assert_eq!(decoder.feed(b"iperf3\r\n- - - -\n"), "iperf3\r\n- - - -\n");
        assert_eq!(decoder.feed(b"a\x07b"), "ab");
    }

    #[test]
    fn strips_ansi_sequences() {
        let mut decoder = Decoder::default();
        assert_eq!(decoder.feed(b"\x1b[32mgreen\x1b[0m done"), "green done");
        assert_eq!(decoder.feed(b"\x1b]0;user@host\x07$ "), "$ ");
        assert_eq!(decoder.feed(b"\x1b]0;t\x1b\\ok"), "ok");
    }

    #[test]
    fn resumes_sequences_split_across_chunks() {
        let mut decoder = Decoder::default();
        assert_eq!(decoder.feed(b"\x1b[3"), "");
        assert_eq!(decoder.feed(b"2mok"), "ok");
    }

    #[test]
    fn resumes_utf8_split_across_chunks() {
        let mut decoder = Decoder::default();
        assert_eq!(decoder.feed(&[0xe4, 0xb8]), "");
        assert_eq!(decoder.feed(&[0xad]), "中");
    }

    #[test]
    fn replaces_invalid_utf8_and_continues() {
        let mut decoder = Decoder::default();
        assert_eq!(decoder.feed(&[b'a', 0xff, b'b']), "a\u{fffd}b");
    }
}
