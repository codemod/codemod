//! Semantic retrieval primitives backed by Rig embeddings and in-memory vector store.

use rig::client::embeddings::EmbeddingsClient;
use rig::embeddings::EmbeddingsBuilder;
use rig::vector_store::{in_memory_store::InMemoryVectorStore, TopNResults, VectorStoreIndexDyn};
use rig::wasm_compat::WasmBoxedFuture;
use rig::Embed;

use crate::memory::MemoryError;

const MAX_VECTOR_INDEX_DOCS: usize = 256;

#[derive(rig::Embed, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct SemanticDocument {
    pub id: String,
    #[embed]
    pub text: String,
}

pub struct DynamicContextIndex {
    inner: Box<dyn VectorStoreIndexDyn + Send + Sync>,
}

impl DynamicContextIndex {
    pub fn new(inner: Box<dyn VectorStoreIndexDyn + Send + Sync>) -> Self {
        Self { inner }
    }
}

impl VectorStoreIndexDyn for DynamicContextIndex {
    fn top_n<'a>(
        &'a self,
        req: rig::vector_store::VectorSearchRequest<
            rig::vector_store::request::Filter<serde_json::Value>,
        >,
    ) -> WasmBoxedFuture<'a, TopNResults> {
        self.inner.top_n(req)
    }

    fn top_n_ids<'a>(
        &'a self,
        req: rig::vector_store::VectorSearchRequest<
            rig::vector_store::request::Filter<serde_json::Value>,
        >,
    ) -> WasmBoxedFuture<
        'a,
        std::result::Result<Vec<(f64, String)>, rig::vector_store::VectorStoreError>,
    > {
        self.inner.top_n_ids(req)
    }
}

pub async fn build_dynamic_context_index<C>(
    client: &C,
    embedding_model: &str,
    docs: &[SemanticDocument],
) -> std::result::Result<Option<DynamicContextIndex>, MemoryError>
where
    C: EmbeddingsClient,
    C::EmbeddingModel: Clone + 'static,
{
    if docs.is_empty() {
        return Ok(None);
    }

    let bounded_docs = docs
        .iter()
        .take(MAX_VECTOR_INDEX_DOCS)
        .cloned()
        .collect::<Vec<_>>();

    let model = client.embedding_model(embedding_model.to_string());
    let embeddings = EmbeddingsBuilder::new(model.clone())
        .documents(bounded_docs)
        .map_err(|e| MemoryError::Compaction(format!("Failed to stage semantic docs: {}", e)))?
        .build()
        .await
        .map_err(|e| {
            MemoryError::Compaction(format!("Failed to build semantic embeddings: {}", e))
        })?;

    let store = InMemoryVectorStore::from_documents(embeddings);
    let index = store.index(model);
    Ok(Some(DynamicContextIndex::new(Box::new(index))))
}
