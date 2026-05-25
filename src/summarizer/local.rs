//! `LocalMistralRs` — local LLM summarizer backend (M9).
//!
//! Gated by the `local-inference` Cargo feature. Uses `mistralrs 0.8.1`'s
//! unified `ModelBuilder` (auto-detects text vs. multimodal from the HF
//! repo id). The model is lazily loaded on first call into an `OnceCell`;
//! subsequent calls reuse the loaded `Arc<Model>`.
//!
//! Errors map cleanly into `BackendError` so the M7 fallback machinery
//! (`fallback_to_extractive`) takes over on load or inference failure.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{OnceCell, Semaphore};

use crate::summarizer::backend::{CompactOpts, SummarizerBackend};
use crate::summarizer::error::BackendError;
use crate::summarizer::prompts;
use crate::tokenizer::Tokenizer;

pub struct LocalMistralRs {
    name: String,
    repo_id: String,
    model: OnceCell<Arc<mistralrs::Model>>,
    permit: Arc<Semaphore>,
    #[allow(dead_code)]
    tokenizer: Tokenizer,
}

impl std::fmt::Debug for LocalMistralRs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalMistralRs")
            .field("name", &self.name)
            .field("repo_id", &self.repo_id)
            .field("loaded", &self.model.get().is_some())
            .finish()
    }
}

impl LocalMistralRs {
    pub fn new(name: &str, repo_id: &str, tokenizer: Tokenizer) -> Self {
        Self {
            name: name.to_string(),
            repo_id: repo_id.to_string(),
            model: OnceCell::new(),
            permit: Arc::new(Semaphore::new(1)),
            tokenizer,
        }
    }

    /// Lazy model load. Threadsafe: `OnceCell::get_or_try_init` makes
    /// concurrent callers wait for the single in-flight load.
    #[allow(clippy::print_stderr)]
    // eprintln! is intentional here — PRD §15 + spec §5.2 mandate a
    // one-line stderr download notice on first use. This is the only
    // approved use of print_stderr in lib code.
    async fn model_get_or_load(&self) -> Result<Arc<mistralrs::Model>, BackendError> {
        if let Some(m) = self.model.get() {
            return Ok(m.clone());
        }
        if !hf_cache_has(&self.repo_id) {
            eprintln!(
                "downloading {} from HuggingFace; cached at {} — this may take several minutes",
                self.repo_id,
                hf_cache_root().display(),
            );
        }
        let arc = self
            .model
            .get_or_try_init(|| async {
                let model = mistralrs::ModelBuilder::new(&self.repo_id)
                    .with_auto_isq(mistralrs::IsqBits::Eight)
                    .with_logging()
                    .build()
                    .await
                    .map_err(|e| BackendError::Unavailable(format!("model load failed: {e}")))?;
                Ok::<Arc<mistralrs::Model>, BackendError>(Arc::new(model))
            })
            .await?;
        Ok(arc.clone())
    }
}

#[async_trait]
impl SummarizerBackend for LocalMistralRs {
    fn name(&self) -> &str {
        &self.name
    }
    fn model_id(&self) -> &str {
        &self.repo_id
    }

    async fn compact(&self, content: &str, opts: &CompactOpts) -> Result<String, BackendError> {
        let _guard = self
            .permit
            .acquire()
            .await
            .map_err(|_| BackendError::Unavailable("semaphore closed".into()))?;
        let model = self.model_get_or_load().await?;
        let parts = prompts::render_abstractive(opts, content);
        let messages = mistralrs::TextMessages::new()
            .add_message(mistralrs::TextMessageRole::System, &parts.system)
            .add_message(mistralrs::TextMessageRole::User, &parts.user);
        let resp = model
            .send_chat_request(messages)
            .await
            .map_err(|e| BackendError::ModelError(format!("inference failed: {e}")))?;
        let text = resp
            .choices
            .first()
            .and_then(|c| c.message.content.as_ref())
            .ok_or_else(|| BackendError::ModelError("empty response".into()))?
            .clone();
        Ok(text.trim().to_string())
    }
}

/// Does `~/.cache/huggingface/hub/models--<owner>--<repo>/` exist with
/// at least one entry? Used by the cold-load banner and by the M9 doctor
/// check.
pub fn hf_cache_has(repo_id: &str) -> bool {
    let path = hf_cache_root().join(format!("models--{}", repo_id.replace('/', "--"),));
    path.exists()
        && path
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
}

pub fn hf_cache_root() -> std::path::PathBuf {
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
    fn hf_cache_root_respects_hf_home_env() {
        let tmp = tempfile::tempdir().unwrap();
        let prior = std::env::var("HF_HOME").ok();
        unsafe { std::env::set_var("HF_HOME", tmp.path()) };
        let root = hf_cache_root();
        assert_eq!(root, tmp.path().join("hub"));
        unsafe {
            if let Some(p) = prior {
                std::env::set_var("HF_HOME", p);
            } else {
                std::env::remove_var("HF_HOME");
            }
        }
    }

    #[test]
    fn hf_cache_has_returns_false_for_missing_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let prior = std::env::var("HF_HOME").ok();
        unsafe { std::env::set_var("HF_HOME", tmp.path()) };
        assert!(!hf_cache_has("Qwen/Qwen3.5-0.8B"));
        unsafe {
            if let Some(p) = prior {
                std::env::set_var("HF_HOME", p);
            } else {
                std::env::remove_var("HF_HOME");
            }
        }
    }
}
