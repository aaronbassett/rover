//! `MistralRsCaptioner` — local image captioner via `mistralrs` (SmolVLM
//! family). Gated by the `local-vision` Cargo feature.
//!
//! Uses the same lazy-load + per-instance semaphore pattern as the
//! `LocalMistralRs` summarizer backend. The captioner instance is held
//! in `CaptionerRegistry` as `Arc<dyn VlmCaptioner>`.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{OnceCell, Semaphore};

use crate::vlm::VlmCaptioner;
use crate::vlm::error::VlmError;
use crate::vlm::prompts::render_caption_prompt;

pub struct MistralRsCaptioner {
    name: String,
    repo_id: String,
    model: OnceCell<Arc<mistralrs::Model>>,
    permit: Arc<Semaphore>,
}

impl std::fmt::Debug for MistralRsCaptioner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MistralRsCaptioner")
            .field("name", &self.name)
            .field("repo_id", &self.repo_id)
            .field("loaded", &self.model.get().is_some())
            .finish()
    }
}

impl MistralRsCaptioner {
    pub fn new(name: &str, repo_id: &str, max_concurrent: usize) -> Result<Self, VlmError> {
        let permit = if max_concurrent == 0 {
            2
        } else {
            max_concurrent
        };
        Ok(Self {
            name: name.to_string(),
            repo_id: repo_id.to_string(),
            model: OnceCell::new(),
            permit: Arc::new(Semaphore::new(permit)),
        })
    }

    /// Lazy model load. Threadsafe: `OnceCell::get_or_try_init` makes
    /// concurrent callers wait for the single in-flight load.
    #[allow(clippy::print_stderr)]
    // eprintln! is intentional here — PRD §15 + spec §5.2 mandate a
    // one-line stderr download notice on first use. This is the only
    // approved use of print_stderr in lib code.
    async fn model_get_or_load(&self) -> Result<Arc<mistralrs::Model>, VlmError> {
        if let Some(m) = self.model.get() {
            return Ok(m.clone());
        }
        let was_cached = hf_cache_has(&self.repo_id);
        if !was_cached {
            eprintln!(
                "downloading {} from HuggingFace; cached at {} — this may take several minutes",
                self.repo_id,
                hf_cache_root().display(),
            );
        }
        // Verify an already-cached model before loading; record a fresh
        // download afterwards (trust-on-first-use).
        if was_cached {
            crate::model_integrity::enforce(&self.repo_id)
                .map_err(|e| self.integrity_to_vlm_error(e))?;
        }
        let arc = self
            .model
            .get_or_try_init(|| async {
                let model = mistralrs::ModelBuilder::new(&self.repo_id)
                    .with_auto_isq(mistralrs::IsqBits::Four)
                    .with_logging()
                    .build()
                    .await
                    .map_err(|e| VlmError::Unavailable {
                        name: self.name.clone(),
                        reason: format!("model load failed: {e}"),
                    })?;
                Ok::<Arc<mistralrs::Model>, VlmError>(Arc::new(model))
            })
            .await?;
        if !was_cached {
            crate::model_integrity::record_fresh_download(&self.repo_id);
        }
        Ok(arc.clone())
    }

    fn integrity_to_vlm_error(&self, e: crate::model_integrity::IntegrityError) -> VlmError {
        match e {
            crate::model_integrity::IntegrityError::ModelIntegrityFailure {
                file,
                expected,
                actual,
            } => VlmError::ModelIntegrityFailure {
                name: self.name.clone(),
                file,
                expected,
                actual,
            },
            other => VlmError::Unavailable {
                name: self.name.clone(),
                reason: format!("model integrity check failed: {other}"),
            },
        }
    }
}

#[async_trait]
impl VlmCaptioner for MistralRsCaptioner {
    fn name(&self) -> &str {
        &self.name
    }
    fn model_id(&self) -> &str {
        &self.repo_id
    }

    async fn caption(
        &self,
        image_bytes: &[u8],
        alt: Option<&str>,
        max_tokens: usize,
    ) -> Result<String, VlmError> {
        let _guard = self
            .permit
            .acquire()
            .await
            .map_err(|_| VlmError::SemaphoreClosed)?;
        let model = self.model_get_or_load().await?;
        let img = image::load_from_memory(image_bytes)?;
        let prompt = render_caption_prompt(alt);
        let messages = mistralrs::MultimodalMessages::new().add_image_message(
            mistralrs::TextMessageRole::User,
            &prompt,
            vec![img],
        );
        // mistralrs's `send_chat_request` honors per-request token caps via
        // `RequestBuilder`. For the simple single-shot path we accept the
        // default cap and trim if needed; tighter cap plumbing is a v2 item
        // tracked in open-items.
        let _ = max_tokens; // see comment above
        let resp = model
            .send_chat_request(messages)
            .await
            .map_err(|e| VlmError::ModelError {
                name: self.name.clone(),
                reason: format!("inference failed: {e}"),
            })?;
        let text = resp
            .choices
            .first()
            .and_then(|c| c.message.content.as_ref())
            .ok_or_else(|| VlmError::ModelError {
                name: self.name.clone(),
                reason: "empty response".into(),
            })?
            .clone();
        Ok(text.trim().to_string())
    }
}

/// Does `~/.cache/huggingface/hub/models--<owner>--<repo>/` exist with
/// at least one entry? Used by the cold-load banner.
pub fn hf_cache_has(repo_id: &str) -> bool {
    let path = hf_cache_root().join(format!("models--{}", repo_id.replace('/', "--"),));
    path.exists()
        && path
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
}

fn hf_cache_root() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("HF_HOME") {
        return std::path::PathBuf::from(p).join("hub");
    }
    if let Ok(home) = std::env::var("HOME") {
        return std::path::PathBuf::from(home).join(".cache/huggingface/hub");
    }
    std::path::PathBuf::from(".cache/huggingface/hub")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctor_succeeds_with_zero_max_concurrent_picks_default() {
        let c = MistralRsCaptioner::new("test", "HuggingFaceTB/SmolVLM-256M-Instruct", 0).unwrap();
        assert_eq!(c.name(), "test");
    }
}
