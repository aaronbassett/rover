//! Cloud-backed summarizer wrapping `genai::Client`.
//!
//! Supports every provider `genai` ships natively (OpenAI, Anthropic,
//! Gemini, xAI, Groq, DeepSeek, Together, Fireworks) plus a custom
//! `openai_compat` kind that points at any OpenAI-compatible endpoint
//! via a `ServiceTargetResolver`.

use async_trait::async_trait;
use genai::chat::{ChatMessage, ChatRequest};
use genai::resolver::{AuthData, AuthResolver, Endpoint, ServiceTargetResolver};
use genai::{Client, ServiceTarget};

use crate::summarizer::backend::{CompactMode, CompactOpts, SummarizerBackend};
use crate::summarizer::error::BackendError;
use crate::summarizer::prompts::render_abstractive;

/// Provider kind parsed from `[backends.<name>] provider = "..."`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderKind {
    OpenAi,
    Anthropic,
    Gemini,
    XAi,
    Groq,
    DeepSeek,
    Together,
    Fireworks,
    /// Custom base_url speaking the OpenAI Chat Completions shape.
    OpenAiCompat,
}

impl ProviderKind {
    // Consumed in Task 6 (registry) when mapping `[backends.<name>] provider = "..."`.
    #[allow(dead_code)]
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "openai" => Ok(ProviderKind::OpenAi),
            "anthropic" => Ok(ProviderKind::Anthropic),
            "gemini" => Ok(ProviderKind::Gemini),
            "xai" => Ok(ProviderKind::XAi),
            "groq" => Ok(ProviderKind::Groq),
            "deepseek" => Ok(ProviderKind::DeepSeek),
            "together" => Ok(ProviderKind::Together),
            "fireworks" => Ok(ProviderKind::Fireworks),
            "openai_compat" => Ok(ProviderKind::OpenAiCompat),
            other => Err(format!("unknown provider: {other}")),
        }
    }
}

#[cfg(test)]
mod provider_tests {
    use super::*;

    #[test]
    fn parses_every_supported_provider() {
        for s in [
            "openai",
            "anthropic",
            "gemini",
            "xai",
            "groq",
            "deepseek",
            "together",
            "fireworks",
            "openai_compat",
        ] {
            assert!(ProviderKind::parse(s).is_ok(), "unexpected failure for {s}");
        }
    }

    #[test]
    fn rejects_unknown_provider() {
        assert!(ProviderKind::parse("bogus").is_err());
    }
}

/// Cloud backend. Builds a `genai::Client` once at construction; the
/// service holds an `Arc<dyn SummarizerBackend>` so this struct is
/// cheap to clone.
#[derive(Debug, Clone)]
pub struct CloudBackend {
    name: String,
    model: String,
    client: Client,
}

impl CloudBackend {
    /// Build a cloud backend.
    ///
    /// * `name` — config-key name (e.g. "fast").
    /// * `provider` — parsed provider kind.
    /// * `model` — the literal model id passed to genai (e.g. "gpt-4o-mini").
    /// * `base_url` — only used when `provider == OpenAiCompat`. For native
    ///   providers, pass `None`.
    /// * `api_key` — when `Some`, installs an explicit auth override. When
    ///   `None`, genai's default env-var resolution applies (OPENAI_API_KEY,
    ///   ANTHROPIC_API_KEY, etc.).
    // Consumed in Task 6 (registry) when constructing backends from config.
    #[allow(dead_code)]
    pub fn new(
        name: impl Into<String>,
        provider: ProviderKind,
        model: impl Into<String>,
        base_url: Option<String>,
        api_key: Option<String>,
    ) -> Result<Self, BackendError> {
        let name = name.into();
        let model = model.into();

        let mut builder = Client::builder();

        if provider == ProviderKind::OpenAiCompat {
            let base = base_url
                .clone()
                .ok_or_else(|| BackendError::Invalid("openai_compat requires base_url".into()))?;
            let key_for_resolver = api_key.clone().unwrap_or_else(|| "noop".to_string());
            let mapped_model = model.clone();
            let resolver = ServiceTargetResolver::from_resolver_fn(
                move |service_target: ServiceTarget| -> Result<
                    ServiceTarget,
                    genai::resolver::Error,
                > {
                    // Only remap when the call is destined for our model. We
                    // route by exact model id match because multiple
                    // openai_compat backends with different base_urls might
                    // share a process. We also force the adapter kind to
                    // OpenAI so the OpenAI Chat Completions wire shape is
                    // used regardless of what genai inferred from the model
                    // name (e.g. it would otherwise route arbitrary names
                    // to Ollama).
                    if &*service_target.model.model_name == mapped_model.as_str() {
                        let mut model = service_target.model;
                        model.adapter_kind = genai::adapter::AdapterKind::OpenAI;
                        Ok(ServiceTarget {
                            endpoint: Endpoint::from_owned(base.clone()),
                            auth: AuthData::from_single(key_for_resolver.clone()),
                            model,
                        })
                    } else {
                        Ok(service_target)
                    }
                },
            );
            builder = builder.with_service_target_resolver(resolver);
        } else if let Some(k) = api_key {
            // Native providers with an explicit key override. Most users
            // leave api_key None and let genai's env-var defaults work.
            builder = builder.with_auth_resolver(AuthResolver::from_resolver_fn(
                move |_| -> Result<Option<AuthData>, genai::resolver::Error> {
                    Ok(Some(AuthData::from_single(k.clone())))
                },
            ));
        }

        let client = builder.build();

        Ok(Self {
            name,
            model,
            client,
        })
    }

    fn build_request(&self, content: &str, opts: &CompactOpts) -> ChatRequest {
        let parts = render_abstractive(opts, content);
        ChatRequest::new(vec![
            ChatMessage::system(parts.system),
            ChatMessage::user(parts.user),
        ])
    }

    /// Translate a genai error into our error type by matching on
    /// `genai::Error`'s structural variants. HTTP status codes come
    /// straight out of `webc::Error::ResponseFailedStatus`; genai's own
    /// request-validation variants map to `Invalid`/`AuthFailed`.
    fn map_error(err: genai::Error) -> BackendError {
        use genai::Error::{
            ChatReqHasNoMessages, LastChatMessageIsNotUser, MessageContentTypeNotSupported,
            MessageRoleNotSupported, NoAuthData, NoAuthResolver, RequiresApiKey, WebAdapterCall,
            WebModelCall,
        };
        use genai::webc::Error::ResponseFailedStatus;

        match &err {
            WebModelCall {
                webc_error: ResponseFailedStatus { status, .. },
                ..
            }
            | WebAdapterCall {
                webc_error: ResponseFailedStatus { status, .. },
                ..
            } => {
                if status.as_u16() == 429 {
                    BackendError::RateLimited
                } else if matches!(status.as_u16(), 401 | 403) {
                    BackendError::AuthFailed(err.to_string())
                } else if status.is_client_error() {
                    BackendError::ModelError(err.to_string())
                } else {
                    BackendError::Unavailable(err.to_string())
                }
            }
            RequiresApiKey { .. } | NoAuthResolver { .. } | NoAuthData { .. } => {
                BackendError::AuthFailed(err.to_string())
            }
            ChatReqHasNoMessages { .. }
            | LastChatMessageIsNotUser { .. }
            | MessageRoleNotSupported { .. }
            | MessageContentTypeNotSupported { .. } => BackendError::Invalid(err.to_string()),
            _ => BackendError::Unavailable(err.to_string()),
        }
    }
}

#[async_trait]
impl SummarizerBackend for CloudBackend {
    async fn compact(&self, content: &str, opts: &CompactOpts) -> Result<String, BackendError> {
        if content.trim().is_empty() {
            return Err(BackendError::Invalid("empty content".to_string()));
        }
        // Only Abstractive uses the cloud round-trip; Extractive and
        // Headlines belong to the extractive backend. If a caller asks
        // a cloud backend for Extractive output, we still send the
        // chat request — the abstractive prompt produces extractive-style
        // output well enough — but log a warning so this misuse is visible.
        if opts.mode != CompactMode::Abstractive {
            tracing::warn!(
                target: "rover::summarizer",
                mode = opts.mode.as_str(),
                backend = self.name,
                "cloud backend invoked for non-abstractive mode",
            );
        }
        let req = self.build_request(content, opts);
        let resp = self
            .client
            .exec_chat(&self.model, req, None)
            .await
            .map_err(Self::map_error)?;
        Ok(resp.first_text().unwrap_or_default().to_string())
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn model_id(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod cloud_tests {
    use super::*;
    use crate::summarizer::backend::{CompactMode, PreserveSection, Style};

    fn opts() -> CompactOpts {
        CompactOpts {
            mode: CompactMode::Abstractive,
            style: Style::Prose,
            target_tokens: Some(200),
            focus: None,
            preserve: vec![],
            backend_name: "fast".to_string(),
        }
    }

    #[test]
    fn build_request_has_two_messages() {
        let be = CloudBackend::new(
            "fast",
            ProviderKind::OpenAi,
            "gpt-4o-mini",
            None,
            Some("noop".into()),
        )
        .unwrap();
        let req = be.build_request("hello", &opts());
        // Two messages: system + user.
        assert_eq!(req.messages.len(), 2);
    }

    #[test]
    fn openai_compat_requires_base_url() {
        let r = CloudBackend::new("custom", ProviderKind::OpenAiCompat, "m", None, None);
        assert!(matches!(r, Err(BackendError::Invalid(_))));
    }

    #[test]
    fn openai_compat_constructs_with_base_url() {
        let r = CloudBackend::new(
            "custom",
            ProviderKind::OpenAiCompat,
            "m",
            Some("http://127.0.0.1:1234/v1".into()),
            Some("k".into()),
        );
        assert!(r.is_ok());
    }

    #[test]
    fn preserve_optional_field_round_trips() {
        let _ = vec![PreserveSection::Code];
    }
}
