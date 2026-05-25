//! `CloudCaptioner` — vision via the existing `genai` dep. Supports any
//! `genai`-known provider that accepts image inputs (OpenAI gpt-4o family,
//! Anthropic Claude with vision, Gemini, plus `openai_compat` servers that
//! implement OpenAI's vision spec).

use async_trait::async_trait;
use base64::Engine as _;
use genai::Client;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, ContentPart};

use crate::summarizer::cloud::ProviderKind;
use crate::vlm::VlmCaptioner;
use crate::vlm::error::VlmError;
use crate::vlm::prompts::render_caption_prompt;

pub struct CloudCaptioner {
    name: String,
    model: String,
    provider_model: String,
    client: Client,
}

impl std::fmt::Debug for CloudCaptioner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudCaptioner")
            .field("name", &self.name)
            .field("model", &self.model)
            .finish()
    }
}

impl CloudCaptioner {
    pub fn new(
        name: &str,
        provider: ProviderKind,
        model: &str,
        base_url: Option<String>,
        api_key: Option<String>,
    ) -> Result<Self, VlmError> {
        // Reuse the same client builder M7 uses for cloud summarizers; the
        // logic for openai_compat base_url + api_key resolution lives in
        // `summarizer::cloud::build_client`. We import it here.
        let provider_model =
            crate::summarizer::cloud::resolve_request_model(provider.clone(), model);
        let client = crate::summarizer::cloud::build_client(
            provider,
            base_url.as_deref(),
            api_key.as_deref(),
        )
        .map_err(|e| VlmError::Unavailable {
            name: name.to_string(),
            reason: e,
        })?;
        Ok(Self {
            name: name.to_string(),
            model: model.to_string(),
            provider_model,
            client,
        })
    }

    fn mime_for(image_bytes: &[u8]) -> &'static str {
        // Magic-byte sniff. Order matters (PNG and GIF have very specific signatures).
        if image_bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image/png"
        } else if image_bytes.starts_with(b"\xff\xd8\xff") {
            "image/jpeg"
        } else if image_bytes.starts_with(b"GIF87a") || image_bytes.starts_with(b"GIF89a") {
            "image/gif"
        } else if image_bytes.len() >= 12
            && &image_bytes[0..4] == b"RIFF"
            && &image_bytes[8..12] == b"WEBP"
        {
            "image/webp"
        } else {
            "application/octet-stream"
        }
    }
}

#[async_trait]
impl VlmCaptioner for CloudCaptioner {
    fn name(&self) -> &str {
        &self.name
    }

    fn model_id(&self) -> &str {
        &self.model
    }

    async fn caption(
        &self,
        image_bytes: &[u8],
        alt: Option<&str>,
        max_tokens: usize,
    ) -> Result<String, VlmError> {
        let prompt = render_caption_prompt(alt);
        let mime = Self::mime_for(image_bytes);
        let b64 = base64::engine::general_purpose::STANDARD.encode(image_bytes);

        // Build a multimodal user message: prompt text + binary image part.
        // genai 0.4 uses ContentPart::Binary(Binary { content_type, source:
        // BinarySource::Base64(...) }) for image payloads.  The convenience
        // constructor ContentPart::from_binary_base64 wraps this.
        // MessageContent has From<Vec<ContentPart>>.
        let parts: Vec<ContentPart> = vec![
            ContentPart::Text(prompt),
            ContentPart::from_binary_base64(mime, b64.as_str(), None),
        ];
        let opts = ChatOptions::default().with_max_tokens(max_tokens as u32);
        let req = ChatRequest::new(vec![ChatMessage::user(parts)]);

        let resp = self
            .client
            .exec_chat(&self.provider_model, req, Some(&opts))
            .await
            .map_err(|e| map_genai_err(&self.name, e))?;
        let text = resp.into_first_text().unwrap_or_default();
        Ok(text.trim().to_string())
    }
}

fn map_genai_err(name: &str, e: genai::Error) -> VlmError {
    use genai::Error::{NoAuthData, NoAuthResolver, RequiresApiKey, WebAdapterCall, WebModelCall};
    use genai::webc::Error::ResponseFailedStatus;

    match &e {
        WebModelCall {
            webc_error: ResponseFailedStatus { status, .. },
            ..
        }
        | WebAdapterCall {
            webc_error: ResponseFailedStatus { status, .. },
            ..
        } => {
            if matches!(status.as_u16(), 401 | 403) {
                VlmError::AuthFailed {
                    name: name.to_string(),
                }
            } else if status.as_u16() == 429 {
                VlmError::RateLimited {
                    name: name.to_string(),
                }
            } else {
                VlmError::Unavailable {
                    name: name.to_string(),
                    reason: e.to_string(),
                }
            }
        }
        RequiresApiKey { .. } | NoAuthResolver { .. } | NoAuthData { .. } => VlmError::AuthFailed {
            name: name.to_string(),
        },
        _ => VlmError::Unavailable {
            name: name.to_string(),
            reason: e.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_sniff_png() {
        let png_magic = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
        assert_eq!(CloudCaptioner::mime_for(png_magic), "image/png");
    }

    #[test]
    fn mime_sniff_jpeg() {
        let jpeg_magic = b"\xff\xd8\xff\xe0";
        assert_eq!(CloudCaptioner::mime_for(jpeg_magic), "image/jpeg");
    }

    #[test]
    fn mime_sniff_webp() {
        let mut webp = vec![0u8; 12];
        webp[0..4].copy_from_slice(b"RIFF");
        webp[8..12].copy_from_slice(b"WEBP");
        assert_eq!(CloudCaptioner::mime_for(&webp), "image/webp");
    }

    #[test]
    fn mime_sniff_unknown_returns_octet_stream() {
        assert_eq!(
            CloudCaptioner::mime_for(b"not an image"),
            "application/octet-stream"
        );
    }
}
