use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

/// File write operation for batched async I/O
#[derive(Debug)]
pub struct FileWriteOperation {
    path: PathBuf,
    content: String,
    sender: tokio::sync::oneshot::Sender<std::io::Result<()>>,
}

/// Result of reading source text, including whether legacy decoding was required.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedSource {
    pub content: String,
    /// `None` when the on-disk bytes were already valid UTF-8.
    pub decoded_from: Option<&'static str>,
}

/// Read a source file as Unicode text.
///
/// Valid UTF-8 is returned as-is. Otherwise we honor UTF-16 BOMs, then use
/// `chardetng` + `encoding_rs` so legacy-encoded sources can still be parsed.
pub fn read_source_file(path: &Path) -> std::io::Result<DecodedSource> {
    let bytes = std::fs::read(path)?;
    Ok(decode_source_bytes(&bytes))
}

/// Decode source bytes to Unicode for Tree-sitter / JSSG analysis.
pub fn decode_source_bytes(bytes: &[u8]) -> DecodedSource {
    if let Ok(content) = std::str::from_utf8(bytes) {
        return DecodedSource {
            content: content.to_owned(),
            decoded_from: None,
        };
    }

    // Prefer explicit BOMs before statistical detection.
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let (content, _, _) = encoding_rs::UTF_16LE.decode(bytes);
        return DecodedSource {
            content: content.into_owned(),
            decoded_from: Some("UTF-16LE"),
        };
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let (content, _, _) = encoding_rs::UTF_16BE.decode(bytes);
        return DecodedSource {
            content: content.into_owned(),
            decoded_from: Some("UTF-16BE"),
        };
    }

    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(bytes, true);
    let encoding = detector.guess(None, true);
    let (content, used, _) = encoding.decode(bytes);
    DecodedSource {
        content: content.into_owned(),
        decoded_from: Some(used.name()),
    }
}

/// Async file writer that batches writes to reduce I/O contention
pub struct AsyncFileWriter {
    sender: mpsc::UnboundedSender<FileWriteOperation>,
}

impl Default for AsyncFileWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl AsyncFileWriter {
    pub fn new() -> Self {
        let (sender, mut receiver) = mpsc::unbounded_channel::<FileWriteOperation>();

        tokio::spawn(async move {
            while let Some(operation) = receiver.recv().await {
                let result = tokio::fs::write(&operation.path, &operation.content).await;
                let _ = operation.sender.send(result);
            }
        });

        Self { sender }
    }

    pub async fn write_file(&self, path: PathBuf, content: String) -> std::io::Result<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let operation = FileWriteOperation {
            path,
            content,
            sender: tx,
        };

        if self.sender.send(operation).is_err() {
            return Err(std::io::Error::other("File writer channel closed"));
        }

        rx.await
            .map_err(|_| std::io::Error::other("File write operation canceled"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_source_bytes_keeps_utf8() {
        let decoded = decode_source_bytes(b"var x = 1;\n");
        assert_eq!(decoded.content, "var x = 1;\n");
        assert_eq!(decoded.decoded_from, None);
    }

    #[test]
    fn decode_source_bytes_handles_windows_1252() {
        // `café` with Windows-1252 é (0xE9)
        let decoded = decode_source_bytes(&[b'c', b'a', b'f', 0xE9]);
        assert_eq!(decoded.content, "café");
        assert!(decoded.decoded_from.is_some());
    }
}
