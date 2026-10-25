use encoding_rs::{CoderResult, Encoding, UTF_8};

/// Stateful terminal decoder plus outbound encoder. Keeping the decoder alive
/// across SSH packets is important because a multibyte character may be split
/// between two channel data messages.
pub(crate) struct TerminalEncoding {
    encoding: &'static Encoding,
    decoder: encoding_rs::Decoder,
}

impl TerminalEncoding {
    pub(crate) fn new(label: &str) -> Self {
        let encoding = Encoding::for_label(label.trim().as_bytes()).unwrap_or(UTF_8);
        Self {
            encoding,
            decoder: encoding.new_decoder_without_bom_handling(),
        }
    }

    pub(crate) fn decode(&mut self, input: &[u8]) -> String {
        let mut output = String::with_capacity(input.len().saturating_mul(2).max(16));
        let mut offset = 0;
        loop {
            let (result, read, _) =
                self.decoder
                    .decode_to_string(&input[offset..], &mut output, false);
            offset += read;
            match result {
                CoderResult::InputEmpty => break,
                CoderResult::OutputFull => output.reserve(input.len().max(16)),
            }
        }
        output
    }

    pub(crate) fn encode(&self, input: &[u8]) -> Vec<u8> {
        if self.encoding == UTF_8 {
            return input.to_vec();
        }
        let text = String::from_utf8_lossy(input);
        let (encoded, _, _) = self.encoding.encode(&text);
        encoded.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gbk_round_trip_and_split_decode() {
        let encoder = TerminalEncoding::new("gbk");
        let bytes = encoder.encode("中文测试".as_bytes());
        let mut decoder = TerminalEncoding::new("gbk");
        let split = 3;
        let mut decoded = decoder.decode(&bytes[..split]);
        decoded.push_str(&decoder.decode(&bytes[split..]));
        assert_eq!(decoded, "中文测试");
    }

    #[test]
    fn unknown_label_falls_back_to_utf8() {
        let mut codec = TerminalEncoding::new("not-a-real-charset");
        assert_eq!(codec.decode("你好".as_bytes()), "你好");
    }

    #[test]
    fn utf8_remote_console_output_survives_every_packet_boundary() {
        // Entirely synthetic remote output, not a captured user's session.
        // Windows ConPTY sends UTF-8 even when the hosted application uses a
        // different code page. Exercise both decoding and the terminal parser.
        let original = concat!(
            "Windows 测试终端\r\n",
            "\x1b[32mdemo@测试主机\x1b[0m C:\\fixture> 😀"
        );
        let bytes = original.as_bytes();
        for split in 0..=bytes.len() {
            let mut codec = TerminalEncoding::new("UTF-8");
            let mut terminal = crate::terminal::TermBuffer::new(5, 80);
            let mut decoded = String::new();
            for packet in [&bytes[..split], &bytes[split..]] {
                let chunk = codec.decode(packet);
                terminal.ingest(chunk.as_bytes());
                decoded.push_str(&chunk);
            }
            assert_eq!(decoded, original, "split={split}");
            assert_eq!(terminal.parser.screen().contents(), "Windows 测试终端\ndemo@测试主机 C:\\fixture> 😀", "split={split}");
        }
        let mut codec = TerminalEncoding::new("UTF-8");
        let mut terminal = crate::terminal::TermBuffer::new(5, 80);
        let decoded: String = bytes.chunks(1).map(|packet| {
            let chunk = codec.decode(packet);
            terminal.ingest(chunk.as_bytes());
            chunk
        }).collect();
        assert_eq!(decoded, original);
        assert_eq!(terminal.parser.screen().contents(), "Windows 测试终端\ndemo@测试主机 C:\\fixture> 😀");
    }

    #[test]
    fn explicitly_configured_legacy_output_preserves_raw_fixture_and_split_characters() {
        // The same Chinese glyphs are GB18030 here. Choosing a legacy decoder
        // is an explicit session setting, not an inference from remote OS.
        let raw = b"\x1b[32m\xd6\xd0\xce\xc4\x1b[0m\r\ndemo@fixture> ";
        let expected = "\x1b[32m中文\x1b[0m\r\ndemo@fixture> ";
        for split in 0..=raw.len() {
            let mut codec = TerminalEncoding::new("GB18030");
            let mut decoded = codec.decode(&raw[..split]);
            decoded.push_str(&codec.decode(&raw[split..]));
            assert_eq!(decoded, expected, "split={split}");
            assert_eq!(codec.encode(expected.as_bytes()), raw);
        }
    }

    #[test]
    fn decoder_does_not_guess_a_charset_or_repair_replacements_sent_by_the_peer() {
        let original = "demo@\u{fffd}\u{fffd} C:\\fixture> ";
        let mut codec = TerminalEncoding::new("UTF-8");
        assert_eq!(codec.decode(original.as_bytes()), original);

        // A screenshot cannot tell malformed transport bytes from valid UTF-8
        // replacement characters already emitted by the remote application.
        let mut codec = TerminalEncoding::new("UTF-8");
        let decoded = codec.decode(b"\xd6\xd0\xce\xc4");
        assert!(decoded.contains('\u{fffd}'));
        assert_ne!(decoded, "中文", "UTF-8 must not silently become GB18030");
    }
}
