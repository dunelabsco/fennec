use anyhow::Result;
use async_trait::async_trait;

use super::openai::OpenAIProvider;
use super::traits::{ChatRequest, ChatResponse, Provider, StreamEvent};

/// OpenRouter provider that wraps OpenAIProvider with OpenRouter-specific
/// base URL and headers.
pub struct OpenRouterProvider {
    inner: OpenAIProvider,
}

impl OpenRouterProvider {
    /// Create a new OpenRouter provider.
    ///
    /// Uses the OpenAI-compatible API at `https://openrouter.ai/api/v1`
    /// with the required `HTTP-Referer` and `X-Title` headers.
    pub fn new(
        api_key: String,
        model: Option<String>,
        context_window: Option<usize>,
    ) -> Self {
        let inner = OpenAIProvider::new(
            api_key,
            model,
            Some("https://openrouter.ai/api/v1".to_string()),
            context_window,
        )
        .with_extra_headers(vec![
            ("HTTP-Referer".to_string(), "https://fennec.dev".to_string()),
            ("X-Title".to_string(), "Fennec".to_string()),
        ]);

        Self { inner }
    }
}

#[async_trait]
impl Provider for OpenRouterProvider {
    fn name(&self) -> &str {
        "openrouter"
    }

    async fn chat(&self, request: ChatRequest<'_>) -> Result<ChatResponse> {
        self.inner.chat(request).await
    }

    fn supports_tool_calling(&self) -> bool {
        self.inner.supports_tool_calling()
    }

    fn context_window(&self) -> usize {
        self.inner.context_window()
    }

    fn supports_streaming(&self) -> bool {
        self.inner.supports_streaming()
    }

    async fn chat_stream(
        &self,
        request: ChatRequest<'_>,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        self.inner.chat_stream(request).await
    }
}
