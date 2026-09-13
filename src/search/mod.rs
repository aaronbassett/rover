//! Web search: discovery of web resources.
//!
//! Rover's division of labour is deliberate and load-bearing:
//!
//! * **`search` discovers** — it returns ranked candidate URLs plus enough
//!   metadata to choose between them.
//! * **`fetch` reads** — it goes to the origin, extracts, guards, caches.
//!
//! A search never triggers a fetch. Nothing in this module touches the
//! fetch pipeline, the page cache, or the summariser; the agent decides
//! which of the returned URLs are worth spending a fetch on. That boundary
//! is what stops `search` from quietly becoming search-scrape-summarise,
//! with the token cost, the origin load, and the injection surface that
//! implies.
//!
//! # Layering
//!
//! * [`model`] — the stable wire shape of a response (always compiled).
//! * [`request`] — normalisation and validation of a request against the
//!   `[search]` config (always compiled, so `rover.toml` validates
//!   identically in every build).
//! * [`error`] — the typed failures, one per stable MCP error code.
//! * [`brave`] — the only provider implementation (`web-search` only).
//! * [`SearchService`] — the boundary the rest of Rover talks to. Nothing
//!   outside this module names a Brave type.
//!
//! # Provider abstraction
//!
//! There is no `SearchProvider` trait and no `provider = "..."` config key.
//! Brave is the only implementation, and a trait with one impl is a guess
//! about the second one. What *is* kept clean is the boundary: callers hand
//! [`SearchService`] a validated [`SearchRequest`] and get back a
//! [`SearchResponse`]. Adding a provider later means adding an arm to
//! [`SearchService`] and a config key to select it — not rewriting the MCP
//! tool, the CLI, or the wire contract.

pub mod error;
pub mod model;
pub mod request;

#[cfg(feature = "web-search")]
pub mod brave;
#[cfg(feature = "web-search")]
pub mod rate;

pub use error::SearchError;
pub use model::{SearchResponse, SearchResult};
pub use request::{SearchOverrides, SearchRequest};

use crate::config::SearchConfig;

/// Whether search can actually run, and why not when it can't.
///
/// The three states are what `rover doctor` reports and what the generated
/// agent steering keys off: steering should never point an agent at a tool
/// that cannot work in this build, with this configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchAvailability {
    /// Built without the `web-search` cargo feature.
    NotCompiled,
    /// Compiled, but no credential is present in the environment.
    NotConfigured,
    /// Compiled and credentialed.
    Ready,
}

impl SearchAvailability {
    /// Resolve availability from config plus the compiled feature set.
    pub fn detect(cfg: &SearchConfig) -> Self {
        #[cfg(not(feature = "web-search"))]
        {
            let _ = cfg;
            Self::NotCompiled
        }
        #[cfg(feature = "web-search")]
        {
            if cfg.is_configured() {
                Self::Ready
            } else {
                Self::NotConfigured
            }
        }
    }

    /// True only when a search call would actually be attempted.
    pub const fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }

    /// One-line description used in tool descriptions, `rover doctor`, and
    /// the generated steering.
    pub const fn describe(self) -> &'static str {
        match self {
            Self::NotCompiled => {
                "unavailable: this build was compiled without the `web-search` cargo feature"
            }
            Self::NotConfigured => {
                "unavailable: no Brave Search API key in the environment (see `[search] api_key_env`)"
            }
            Self::Ready => "available: Brave Search is configured",
        }
    }
}

/// The search boundary the rest of Rover talks to.
///
/// Cheap to clone behind an `Arc`; holds one HTTP client and one endpoint
/// pacer for the process's lifetime.
pub struct SearchService {
    cfg: SearchConfig,
    #[cfg(feature = "web-search")]
    provider: brave::BraveProvider,
}

impl std::fmt::Debug for SearchService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchService")
            .field("availability", &self.availability())
            .finish_non_exhaustive()
    }
}

impl SearchService {
    /// Build the service from config. Never fails and never touches the
    /// network: a missing credential is reported at call time (and by
    /// `rover doctor`), not at startup, so Rover still starts — and still
    /// fetches — with no search configuration at all.
    pub fn new(cfg: &SearchConfig, user_agent: &str) -> Self {
        #[cfg(feature = "web-search")]
        {
            Self {
                cfg: cfg.clone(),
                provider: brave::BraveProvider::new(cfg.clone(), user_agent),
            }
        }
        #[cfg(not(feature = "web-search"))]
        {
            let _ = user_agent;
            Self { cfg: cfg.clone() }
        }
    }

    /// The `[search]` defaults this service was built with.
    pub fn config(&self) -> &SearchConfig {
        &self.cfg
    }

    /// Current availability, re-checked on every call so a key exported
    /// after the server started is picked up without a restart.
    pub fn availability(&self) -> SearchAvailability {
        SearchAvailability::detect(&self.cfg)
    }

    /// Run a search.
    ///
    /// Returns [`SearchError::FeatureNotCompiled`] in a build without the
    /// feature and [`SearchError::NotConfigured`] without a credential —
    /// never an empty result set dressed up as success.
    pub async fn search(&self, req: &SearchRequest) -> Result<SearchResponse, SearchError> {
        #[cfg(feature = "web-search")]
        {
            self.provider.search(req).await
        }
        #[cfg(not(feature = "web-search"))]
        {
            let _ = req;
            Err(SearchError::FeatureNotCompiled)
        }
    }

    /// Validate and run in one step, folding per-call overrides over the
    /// configured defaults.
    pub async fn search_with(
        &self,
        query: &str,
        overrides: SearchOverrides,
    ) -> Result<SearchResponse, SearchError> {
        // Availability first: an unconfigured or uncompiled build should say
        // so plainly rather than reporting an argument problem it would only
        // hit later, and an uncompiled build has no provider to ask.
        match self.availability() {
            SearchAvailability::NotCompiled => return Err(SearchError::FeatureNotCompiled),
            SearchAvailability::NotConfigured => {
                return Err(SearchError::NotConfigured {
                    env_var: self.cfg.api_key_env.clone(),
                });
            }
            SearchAvailability::Ready => {}
        }
        let req = SearchRequest::build(query, &self.cfg, overrides)?;
        self.search(&req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialises the tests that mutate the credential environment
    /// variable, which is process-global.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn cfg_with_env(var: &str) -> SearchConfig {
        SearchConfig {
            api_key_env: var.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn availability_reflects_the_compiled_feature_and_the_credential() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let var = "ROVER_TEST_SEARCH_KEY_AVAILABILITY";
        let cfg = cfg_with_env(var);

        // SAFETY: serialised by ENV_LOCK; the var is test-specific.
        unsafe { std::env::remove_var(var) };
        let expected_without_key = if cfg!(feature = "web-search") {
            SearchAvailability::NotConfigured
        } else {
            SearchAvailability::NotCompiled
        };
        assert_eq!(SearchAvailability::detect(&cfg), expected_without_key);

        unsafe { std::env::set_var(var, "secret") };
        let expected_with_key = if cfg!(feature = "web-search") {
            SearchAvailability::Ready
        } else {
            SearchAvailability::NotCompiled
        };
        assert_eq!(SearchAvailability::detect(&cfg), expected_with_key);
        unsafe { std::env::remove_var(var) };
    }

    #[test]
    fn a_blank_credential_does_not_count_as_configured() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let var = "ROVER_TEST_SEARCH_KEY_BLANK";
        let cfg = cfg_with_env(var);
        // SAFETY: serialised by ENV_LOCK.
        unsafe { std::env::set_var(var, "   ") };
        assert!(!cfg.is_configured());
        assert!(cfg.api_key().is_none());
        unsafe { std::env::remove_var(var) };
    }

    #[tokio::test]
    async fn search_without_a_credential_never_pretends_to_succeed() {
        // A test-specific variable no other test touches, so the shared
        // ENV_LOCK (which cannot be held across an await) is not needed.
        let var = "ROVER_TEST_SEARCH_KEY_MISSING";
        // SAFETY: this variable is unique to this test.
        unsafe { std::env::remove_var(var) };
        let svc = SearchService::new(&cfg_with_env(var), "rover-test/0");
        let err = svc
            .search_with("anything", SearchOverrides::default())
            .await
            .expect_err("must not report success");
        if cfg!(feature = "web-search") {
            assert!(matches!(err, SearchError::NotConfigured { .. }), "{err}");
            assert!(err.to_string().contains(var), "{err}");
        } else {
            assert!(matches!(err, SearchError::FeatureNotCompiled), "{err}");
        }
    }

    #[test]
    fn describe_covers_every_state() {
        for s in [
            SearchAvailability::NotCompiled,
            SearchAvailability::NotConfigured,
            SearchAvailability::Ready,
        ] {
            assert!(!s.describe().is_empty());
        }
        assert!(SearchAvailability::Ready.is_ready());
        assert!(!SearchAvailability::NotConfigured.is_ready());
        assert!(!SearchAvailability::NotCompiled.is_ready());
    }
}
