use std::path::PathBuf;
use tokio::sync::mpsc;

/// File write operation for batched async I/O
#[derive(Debug)]
pub struct FileWriteOperation {
    path: PathBuf,
    content: String,
    sender: tokio::sync::oneshot::Sender<std::io::Result<()>>,
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
