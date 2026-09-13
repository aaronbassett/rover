//! Search errors.
//!
//! Every variant maps onto exactly one stable MCP wire code (see
//! `src/mcp/error.rs`), so a caller can branch on the code rather than on
//! message text. No variant ever carries the API credential: the token is
//! read from the environment at request time, sent as a header, and never
//! stored on a request, a response, or an error.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SearchError {
    /// The binary was built without the `web-search` Cargo feature.
    #[error(
        "web search is not available in this build: Rover was compiled without the `web-search` \
         cargo feature. Install the prebuilt binary (Homebrew, the install script, or the \
         container image), or rebuild with `cargo install rover-fetch --features web-search`."
    )]
    FeatureNotCompiled,

    /// The feature is compiled but no credential is available.
    #[error(
        "web search is not configured: no Brave Search API key found in ${env_var}. Create a key \
         at https://api-dashboard.search.brave.com/ and export it, e.g. \
         `export {env_var}=...`. The variable name is set by `[search] api_key_env`."
    )]
    NotConfigured { env_var: String },

    /// The caller's arguments (or the `[search]` config) are invalid. Never
    /// reaches the network, so it never costs a request.
    #[error("invalid search request: {0}")]
    InvalidRequest(String),

    /// The provider rejected the credential (HTTP 401/403 with an
    /// authentication-shaped code).
    #[error("search provider rejected the API key: {detail}")]
    AuthFailed { detail: String },

    /// Authenticated, but the plan/subscription does not permit this call.
    #[error("search provider denied the request for this subscription: {detail}")]
    SubscriptionDenied { detail: String },

    /// Rate limited (HTTP 429). `retry_after_secs` is the provider's
    /// `Retry-After` when it sent one.
    #[error("search provider rate limited the request{}", match retry_after_secs {
        Some(s) => format!(" (retry after {s}s)"),
        None => String::new(),
    })]
    RateLimited { retry_after_secs: Option<u64> },

    /// The subscription's quota is exhausted. Distinct from rate limiting:
    /// waiting does not help within the billing period.
    #[error("search provider quota exhausted: {detail}")]
    QuotaExhausted { detail: String },

    /// The provider answered, but not with something Rover can parse.
    #[error("search provider returned a malformed response: {0}")]
    MalformedResponse(String),

    /// The provider failed on its own side (5xx), or kept failing across
    /// every permitted retry.
    #[error("search provider error: {detail}")]
    Upstream { detail: String },

    /// The request never completed: connection failure, TLS failure, DNS.
    #[error("could not reach the search provider: {detail}")]
    Network { detail: String },

    /// The request exceeded `[search] timeout_secs`.
    #[error("search request timed out after {secs}s")]
    Timeout { secs: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The credential must never be reachable through a `SearchError` —
    /// not in `Display` (which reaches the agent) and not in `Debug`
    /// (which reaches the logs).
    #[test]
    fn errors_never_carry_the_credential() {
        const SECRET: &str = "BSA-super-secret-token-value";
        let errors = vec![
            SearchError::FeatureNotCompiled,
            SearchError::NotConfigured {
                env_var: "BRAVE_SEARCH_API_KEY".into(),
            },
            SearchError::InvalidRequest("count must be 1..=20".into()),
            SearchError::AuthFailed {
                detail: "HTTP 401".into(),
            },
            SearchError::SubscriptionDenied {
                detail: "HTTP 403".into(),
            },
            SearchError::RateLimited {
                retry_after_secs: Some(2),
            },
            SearchError::QuotaExhausted {
                detail: "QUOTA_LIMITED".into(),
            },
            SearchError::MalformedResponse("expected object".into()),
            SearchError::Upstream {
                detail: "HTTP 503".into(),
            },
            SearchError::Network {
                detail: "connection refused".into(),
            },
            SearchError::Timeout { secs: 10 },
        ];
        for e in errors {
            assert!(!e.to_string().contains(SECRET), "display leaked: {e}");
            assert!(!format!("{e:?}").contains(SECRET), "debug leaked: {e:?}");
        }
    }

    #[test]
    fn not_configured_names_the_env_var_twice_for_copy_paste() {
        let e = SearchError::NotConfigured {
            env_var: "MY_BRAVE_KEY".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("$MY_BRAVE_KEY"), "{msg}");
        assert!(msg.contains("export MY_BRAVE_KEY="), "{msg}");
    }

    #[test]
    fn rate_limited_renders_retry_after_when_known() {
        assert!(
            SearchError::RateLimited {
                retry_after_secs: Some(7)
            }
            .to_string()
            .contains("retry after 7s")
        );
        assert!(
            !SearchError::RateLimited {
                retry_after_secs: None
            }
            .to_string()
            .contains("retry after")
        );
    }
}
