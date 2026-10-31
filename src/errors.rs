use regex::Regex;
use std::fmt;
use std::sync::OnceLock;
use thiserror::Error;

static KEY_REGEX: OnceLock<Regex> = OnceLock::new();
static SECRET_REGEX: OnceLock<Regex> = OnceLock::new();
static BEARER_REGEX: OnceLock<Regex> = OnceLock::new();

/// Redacts sensitive credentials (API keys, HMAC secret keys, bearer tokens) from string values.
pub fn sanitize_message(msg: &str) -> String {
    if msg.is_empty() {
        return String::new();
    }

    let key_re = KEY_REGEX
        .get_or_init(|| Regex::new(r"sp_(test|sdk|wp)_[a-zA-Z0-9_-]{10,}").expect("valid regex"));
    let secret_re = SECRET_REGEX
        .get_or_init(|| Regex::new(r"(?i)(sec|secret)_[a-zA-Z0-9_-]{8,}").expect("valid regex"));
    let bearer_re = BEARER_REGEX
        .get_or_init(|| Regex::new(r"(?i)Bearer\s+[a-zA-Z0-9._-]+").expect("valid regex"));

    let res = key_re.replace_all(msg, |caps: &regex::Captures| {
        let full = &caps[0];
        if full.len() > 14 {
            format!("{}...{}", &full[..10], &full[full.len() - 4..])
        } else {
            format!("{}...", &full[..6])
        }
    });

    let res = secret_re.replace_all(&res, "***REDACTED***");
    let res = bearer_re.replace_all(&res, "Bearer ***REDACTED***");

    res.into_owned()
}

/// A wrapper struct for secrets that hides their contents in `Display` and `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretRedacted<T>(pub T);

impl<T: AsRef<str>> fmt::Debug for SecretRedacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl<T: AsRef<str>> fmt::Display for SecretRedacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sanitized = sanitize_message(self.0.as_ref());
        write!(f, "{}", sanitized)
    }
}

/// Errors produced by the SmallPict SDK.
#[derive(Debug, Error)]
pub enum SmallPictError {
    #[error("[VALIDATION_FAILED] HTTP 400: {}", sanitize_message(.message))]
    Validation {
        message: String,
        details: Option<serde_json::Value>,
    },

    #[error("[UNAUTHORIZED] HTTP 401: {}", sanitize_message(.message))]
    Unauthorized {
        message: String,
        request_id: Option<String>,
    },

    #[error("[QUOTA_EXCEEDED] HTTP 402: {}", sanitize_message(.message))]
    QuotaExceeded {
        message: String,
        /// Error details from the server, e.g. `upgrade_url`, or `limit_bytes` for `storage_quota_exceeded`.
        details: Option<serde_json::Value>,
        request_id: Option<String>,
    },

    #[error("[FORBIDDEN] HTTP 403: {}", sanitize_message(.message))]
    Forbidden {
        message: String,
        /// Error details from the server, e.g. `mode` and `allowed_modes` for `mode_not_in_plan`.
        details: Option<serde_json::Value>,
        request_id: Option<String>,
    },

    #[error("[NOT_FOUND] HTTP 404: {}", sanitize_message(.message))]
    NotFound {
        message: String,
        request_id: Option<String>,
    },

    #[error("[RATE_LIMIT_EXCEEDED] HTTP 429: {}", sanitize_message(.message))]
    RateLimit {
        message: String,
        retry_after: Option<u64>,
        request_id: Option<String>,
    },

    #[error("[INTERNAL_ERROR] HTTP {status}: {}", sanitize_message(.message))]
    Server {
        status: u16,
        message: String,
        /// Server error code, e.g. `booster_period_limit_exceeded` for a 409.
        code: Option<String>,
        /// Error details from the server, e.g. `upgrade_url`.
        details: Option<serde_json::Value>,
        request_id: Option<String>,
    },

    #[error("[TIMEOUT_ERROR]: {}", sanitize_message(.message))]
    Timeout { message: String },

    #[error("[NETWORK_ERROR]: {source}")]
    Network {
        #[from]
        source: reqwest::Error,
    },

    #[error("[SERIALIZATION_ERROR]: {source}")]
    Serialization {
        #[from]
        source: serde_json::Error,
    },

    #[error("[IO_ERROR]: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },
}

impl SmallPictError {
    pub fn is_validation_error(&self) -> bool {
        matches!(self, Self::Validation { .. })
    }

    pub fn is_unauthorized(&self) -> bool {
        matches!(self, Self::Unauthorized { .. })
    }

    pub fn is_quota_exceeded(&self) -> bool {
        matches!(self, Self::QuotaExceeded { .. })
    }

    pub fn is_forbidden(&self) -> bool {
        matches!(self, Self::Forbidden { .. })
    }

    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound { .. })
    }

    pub fn is_rate_limit(&self) -> bool {
        matches!(self, Self::RateLimit { .. })
    }

    pub fn is_server_error(&self) -> bool {
        matches!(self, Self::Server { .. })
    }

    /// Error details from the server (`error.details`), when the variant carries them.
    pub fn details(&self) -> Option<&serde_json::Value> {
        match self {
            Self::Validation { details, .. }
            | Self::QuotaExceeded { details, .. }
            | Self::Forbidden { details, .. }
            | Self::Server { details, .. } => details.as_ref(),
            _ => None,
        }
    }

    /// Dashboard page where the missing capacity is added (`details.upgrade_url`), or `None`
    /// when absent or not a string. Shown to the user; the SDK never opens or fetches it.
    pub fn upgrade_url(&self) -> Option<&str> {
        self.details()?.get("upgrade_url")?.as_str()
    }

    /// Modes the plan allows (`details.allowed_modes` of a 403 `mode_not_in_plan`), or `None`.
    pub fn allowed_modes(&self) -> Option<Vec<&str>> {
        match self {
            Self::Forbidden {
                details: Some(d), ..
            } => Some(
                d.get("allowed_modes")?
                    .as_array()?
                    .iter()
                    .filter_map(|m| m.as_str())
                    .collect(),
            ),
            _ => None,
        }
    }

    /// Server error code of an error mapped to `Server` (for example a 409), or `None`.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Server { code, .. } => code.as_deref(),
            _ => None,
        }
    }
}

/// True for the 429 `purge_rate_limited` body: the hourly folder/all purge limit, which
/// does not reset within a retry window, so the request is not retried.
pub(crate) fn is_purge_rate_limited(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/code")?
                .as_str()
                .map(|c| c == "purge_rate_limited")
        })
        .unwrap_or(false)
}
