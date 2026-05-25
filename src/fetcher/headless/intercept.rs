//! CDP Fetch domain handler: classify intercepted sub-requests and either
//! `FulfillRequest` (empty 200) or `ContinueRequest`. Per PRD §5.7,
//! blocked requests are ALWAYS fulfilled with empty 200, NEVER aborted —
//! many SPAs error hard on failed CSS/font loads.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, EventRequestPaused, FulfillRequestParams, RequestId,
};
use chromiumoxide::cdp::browser_protocol::network::ResourceType;
use url::Url;

use crate::config::HeadlessConfig;
use crate::fetcher::headless::{HeadlessError, third_party};
use crate::fetcher::ssrf::SsrfLevel;

/// Outcome of classifying a paused request. Used to choose which CDP
/// command to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterceptAction {
    Continue,
    FulfillEmpty,
}

/// Decide what to do with a paused request given the resource type, the
/// first-party host of the navigating frame, and the headless config.
///
/// The SSRF gate runs *before* this in [`handle_paused`]; `classify` only
/// covers the resource-type and third-party block-list gates.
pub fn classify(
    url: &Url,
    req_type: ResourceType,
    frame_first_party_host: &str,
    cfg: &HeadlessConfig,
) -> InterceptAction {
    let block_type = match req_type {
        ResourceType::Image => cfg.block_images,
        ResourceType::Media => cfg.block_media,
        ResourceType::Font => cfg.block_fonts,
        ResourceType::Stylesheet => cfg.block_css,
        _ => false,
    };
    if block_type {
        return InterceptAction::FulfillEmpty;
    }
    if cfg.block_third_party && third_party::matches(url, frame_first_party_host) {
        return InterceptAction::FulfillEmpty;
    }
    // Service workers travel as ResourceType::Other; we conservatively
    // continue here since chromiumoxide doesn't always expose the SW flag.
    // The `block_service_workers` config knob is honored at browser-init
    // time via `Page::set_bypass_service_worker(true)`.
    InterceptAction::Continue
}

/// Handle a `Fetch.requestPaused` event by applying the SSRF gate and the
/// block-list gate, then dispatching the appropriate CDP command.
pub async fn handle_paused(
    page: &Page,
    event: EventRequestPaused,
    cfg: &HeadlessConfig,
    ssrf_level: SsrfLevel,
    project_root: Option<&std::path::Path>,
) -> Result<(), HeadlessError> {
    let request_id = event.request_id.clone();
    let url_str = event.request.url.clone();
    let req_type = event.resource_type;

    // 1. SSRF gate.
    let url = match Url::parse(&url_str) {
        Ok(u) => u,
        Err(_) => {
            // Can't parse → safest to fulfill empty.
            return fulfill_empty(page, request_id).await;
        }
    };
    if crate::fetcher::ssrf::validate_url_for_level(&url, ssrf_level, project_root)
        .await
        .is_err()
    {
        return fulfill_empty(page, request_id).await;
    }

    // 2. Block-list / resource-type gate.
    let host = url.host_str().unwrap_or("");
    let action = classify(&url, req_type, host, cfg);
    match action {
        InterceptAction::FulfillEmpty => fulfill_empty(page, request_id).await,
        InterceptAction::Continue => continue_request(page, request_id).await,
    }
}

async fn fulfill_empty(page: &Page, request_id: RequestId) -> Result<(), HeadlessError> {
    // empty body is base64("")
    let body = BASE64_STANDARD.encode("");
    let params = FulfillRequestParams::builder()
        .request_id(request_id)
        .response_code(200)
        .body(body)
        .build()
        .map_err(HeadlessError::Cdp)?;
    page.execute(params)
        .await
        .map_err(|e| HeadlessError::Cdp(e.to_string()))?;
    Ok(())
}

async fn continue_request(page: &Page, request_id: RequestId) -> Result<(), HeadlessError> {
    page.execute(ContinueRequestParams::new(request_id))
        .await
        .map_err(|e| HeadlessError::Cdp(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_block_all_assets() -> HeadlessConfig {
        HeadlessConfig {
            block_images: true,
            block_fonts: true,
            block_media: true,
            block_css: true,
            block_third_party: true,
            ..HeadlessConfig::default()
        }
    }

    #[test]
    fn classify_image_blocked_when_block_images() {
        let u = Url::parse("https://example.com/x.png").unwrap();
        let a = classify(
            &u,
            ResourceType::Image,
            "example.com",
            &cfg_block_all_assets(),
        );
        assert_eq!(a, InterceptAction::FulfillEmpty);
    }

    #[test]
    fn classify_third_party_tracker_blocked() {
        let u = Url::parse("https://www.google-analytics.com/collect").unwrap();
        let a = classify(
            &u,
            ResourceType::Xhr,
            "example.com",
            &cfg_block_all_assets(),
        );
        assert_eq!(a, InterceptAction::FulfillEmpty);
    }

    #[test]
    fn classify_first_party_xhr_continues() {
        let u = Url::parse("https://example.com/api/data").unwrap();
        let a = classify(
            &u,
            ResourceType::Xhr,
            "example.com",
            &cfg_block_all_assets(),
        );
        assert_eq!(a, InterceptAction::Continue);
    }

    #[test]
    fn classify_css_not_blocked_by_default() {
        let cfg = HeadlessConfig::default();
        assert!(!cfg.block_css);
        let u = Url::parse("https://example.com/styles.css").unwrap();
        let a = classify(&u, ResourceType::Stylesheet, "example.com", &cfg);
        assert_eq!(a, InterceptAction::Continue);
    }
}
