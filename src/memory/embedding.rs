use anyhow::{Context, Result};
use async_trait::async_trait;

/// Async trait for computing text embeddings.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Human-readable name of this embedding provider.
    fn name(&self) -> &str;

    /// The dimensionality of vectors this provider produces.
    fn dimensions(&self) -> usize;

    /// Embed a single text string into a vector.
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// Embed a batch of text strings. Default implementation loops over `embed`.
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }
}

// ---------------------------------------------------------------------------
// NoopEmbedding
// ---------------------------------------------------------------------------

/// A no-op embedding provider that returns zero vectors.
///
/// Used when no embedding API key is available; the system falls back to
/// keyword-only search.
pub struct NoopEmbedding {
    dims: usize,
}

impl NoopEmbedding {
    pub fn new(dims: usize) -> Self {
        Self { dims }
    }
}

#[async_trait]
impl EmbeddingProvider for NoopEmbedding {
    fn name(&self) -> &str {
        "noop"
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Ok(vec![0.0; self.dims])
    }
}

// ---------------------------------------------------------------------------
// OpenAIEmbedding
// ---------------------------------------------------------------------------

/// Embedding provider that calls the OpenAI-compatible embeddings API.
pub struct OpenAIEmbedding {
    api_key: String,
    client: reqwest::Client,
    model: String,
    base_url: String,
    dims: usize,
}

impl OpenAIEmbedding {
    /// Create a new OpenAI embedding provider.
    ///
    /// Defaults: model `"text-embedding-3-small"`, base_url `"https://api.openai.com/v1"`,
    /// dimensions `1536`.
    pub fn new(
        api_key: String,
        model: Option<String>,
        base_url: Option<String>,
        dims: Option<usize>,
    ) -> Self {
        Self {
            api_key,
            client: reqwest::Client::new(),
            model: model.unwrap_or_else(|| "text-embedding-3-small".to_string()),
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            dims: dims.unwrap_or(1536),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAIEmbedding {
    fn name(&self) -> &str {
        "openai"
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let body = serde_json::json!({
            "model": self.model,
            "input": text,
        });

        let response = self
            .client
            .post(format!("{}/embeddings", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("sending request to embeddings API")?;

        let status = response.status();
        let response_body: serde_json::Value = response
            .json()
            .await
            .context("parsing embeddings API response")?;

        if !status.is_success() {
            let error_msg = response_body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("Embeddings API error ({}): {}", status, error_msg);
        }

        // Parse data[0].embedding
        let embedding = response_body
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("embedding"))
            .and_then(|e| e.as_array())
            .context("missing data[0].embedding in response")?;

        let vec: Vec<f32> = embedding
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect();

        Ok(vec)
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
        });

        let response = self
            .client
            .post(format!("{}/embeddings", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("sending batch request to embeddings API")?;

        let status = response.status();
        let response_body: serde_json::Value = response
            .json()
            .await
            .context("parsing embeddings API batch response")?;

        if !status.is_success() {
            let error_msg = response_body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("Embeddings API error ({}): {}", status, error_msg);
        }

        let data = response_body
            .get("data")
            .and_then(|d| d.as_array())
            .context("missing data array in batch response")?;

        let mut results = Vec::with_capacity(data.len());
        for item in data {
            let embedding = item
                .get("embedding")
                .and_then(|e| e.as_array())
                .context("missing embedding in batch response item")?;

            let vec: Vec<f32> = embedding
                .iter()
                .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                .collect();
            results.push(vec);
        }

        Ok(results)
    }
}
