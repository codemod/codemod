//! Streaming UTF-8 decoding for child-process output: chunks may split a
//! multi-byte sequence, which is carried over to the next chunk; invalid
//! sequences become U+FFFD (lossy, like Node's `setEncoding("utf8")`).

#[derive(Default)]
pub struct StreamDecoder {
    carry: Vec<u8>,
}

impl StreamDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> String {
        self.carry.extend_from_slice(bytes);
        let mut out = String::new();
        let mut rest: &[u8] = &self.carry;
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    out.push_str(text);
                    rest = &[];
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    out.push_str(std::str::from_utf8(&rest[..valid]).unwrap_or_default());
                    match error.error_len() {
                        Some(len) => {
                            out.push('\u{FFFD}');
                            rest = &rest[valid + len..];
                        }
                        None => {
                            rest = &rest[valid..];
                            break;
                        }
                    }
                }
            }
        }
        self.carry = rest.to_vec();
        out
    }

    /// Whatever is left at end of stream (an incomplete sequence) as U+FFFD.
    pub fn finish(&mut self) -> String {
        if self.carry.is_empty() {
            String::new()
        } else {
            self.carry.clear();
            "\u{FFFD}".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StreamDecoder;

    #[test]
    fn carries_split_sequences() {
        let bytes = "aé€😀b".as_bytes();
        for split in 0..bytes.len() {
            let mut decoder = StreamDecoder::default();
            let mut text = decoder.push(&bytes[..split]);
            text.push_str(&decoder.push(&bytes[split..]));
            text.push_str(&decoder.finish());
            assert_eq!(text, "aé€😀b", "split at {split}");
        }
    }

    #[test]
    fn replaces_invalid_bytes() {
        let mut decoder = StreamDecoder::default();
        assert_eq!(decoder.push(b"a\xffb"), "a\u{FFFD}b");
        assert_eq!(decoder.push(b"\xe2\x82"), "");
        assert_eq!(decoder.finish(), "\u{FFFD}");
    }
}
