//! `MistralRsCaptioner` — local vision via `mistralrs`. Implementation: Task 23.

use crate::vlm::{VlmCaptioner, VlmError};

/// Placeholder stub for Task 23.
pub struct MistralRsCaptioner {
    name: String,
    model: String,
}

impl MistralRsCaptioner {
    /// Placeholder constructor. Returns `Ok(Self)`.
    pub fn new(name: &str, model: &str, _max_concurrent: usize) -> Result<Self, VlmError> {
        Ok(Self {
            name: name.to_string(),
            model: model.to_string(),
        })
    }
}

#[async_trait::async_trait]
impl VlmCaptioner for MistralRsCaptioner {
    fn name(&self) -> &str {
        &self.name
    }

    fn model_id(&self) -> &str {
        &self.model
    }

    async fn caption(
        &self,
        _image_bytes: &[u8],
        _alt: Option<&str>,
        _max_tokens: usize,
    ) -> Result<String, VlmError> {
        Err(VlmError::Unavailable {
            name: self.name.clone(),
            reason: "MistralRsCaptioner not yet implemented (Task 23)".into(),
        })
    }
}
