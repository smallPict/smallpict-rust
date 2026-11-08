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
    // Server-issued secrets are "sps_" + hex; sec_/secret_ are the test fixtures.
    let secret_re = SECRET_REGEX.get_or_init(|| {
        Regex::new(r"(?i)(sps_[a-f0-9]{16,}|(sec|secret)_[a-zA-Z0-9_-]{8,})").expect("valid regex")
    });
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

/// Label of a `Server` error: the server code when it sent one, else INTERNAL_ERROR for a 5xx
/// and API_ERROR for anything else that fell through the status mapping.
fn server_label(code: Option<&str>, status: u16) -> &str {
    match code {
        Some(c) => c,
        None if status >= 500 => "INTERNAL_ERROR",
        None => "API_ERROR",
    }
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
        write!(f, "[REDACTED]")
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

    #[error("[{}] HTTP {status}: {}", server_label(.code.as_deref(), *.status), sanitize_message(.message))]
    Server {
        status: u16,
        message: String,
        /// Server error code, e.g. `booster_period_limit_exceeded` for a 409.
        code: Option<String>,
        /// Error details from the server, e.g. `upgrade_url`.
        details: Option<serde_json::Value>,
        request_id: Option<String>,
    },

    /// `optimize()` failed after the API accepted the job: `code` is `upload_failed` (the
    /// storage refused the PUT; `status` is its HTTP status) or `job_failed` (the job failed;
    /// `details.job_id` names it).
    #[error("[{}]: {}", .code.to_uppercase(), sanitize_message(.message))]
    Optimize {
        code: String,
        message: String,
        status: Option<u16>,
        details: Option<serde_json::Value>,
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

    /// True for a 5xx answer. A 409 or other non-5xx status also lands in `Server`, with its
    /// server code, but is not a server fault.
    pub fn is_server_error(&self) -> bool {
        matches!(self, Self::Server { status, .. } if *status >= 500)
    }

    /// Error details from the server (`error.details`), when the variant carries them.
    pub fn details(&self) -> Option<&serde_json::Value> {
        match self {
            Self::Validation { details, .. }
            | Self::QuotaExceeded { details, .. }
            | Self::Forbidden { details, .. }
            | Self::Server { details, .. }
            | Self::Optimize { details, .. } => details.as_ref(),
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

    /// Server error code of an error mapped to `Server` (for example a 409), the
    /// `upload_failed` / `job_failed` code of `Optimize`, or `None`.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Server { code, .. } => code.as_deref(),
            Self::Optimize { code, .. } => Some(code),
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

/// The pre-signed PUT of the image bytes was answered with a non-2xx status.
pub(crate) fn upload_failed(status: u16, body: &str) -> SmallPictError {
    let mut message = format!("Upload to staging failed with HTTP {status}");
    if !body.is_empty() {
        // An S3 error document runs to kilobytes and repeats the pre-signed query; keep the head.
        let head: String = body.chars().take(300).collect();
        message = format!("{message}: {head}");
    }
    SmallPictError::Optimize {
        code: "upload_failed".into(),
        message,
        status: Some(status),
        details: None,
    }
}

/// The job status reported `failed`; `error` is its `error` object.
pub(crate) fn job_failed(job_id: &str, error: Option<&serde_json::Value>) -> SmallPictError {
    let message = error
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .filter(|m| !m.is_empty())
        .unwrap_or("The optimization job failed.");
    SmallPictError::Optimize {
        code: "job_failed".into(),
        message: message.into(),
        status: None,
        details: Some(serde_json::json!({ "job_id": job_id })),
    }
}

/// No final status within `wait`; the job keeps running on the server.
pub(crate) fn job_timeout(job_id: &str, wait: std::time::Duration) -> SmallPictError {
    SmallPictError::Timeout {
        message: format!(
            "Job {job_id} did not finish within {}ms; poll get_job_status(\"{job_id}\") later.",
            wait.as_millis()
        ),
    }
}

/// `optimize_file` takes a local path; the API never fetches remote images.
pub(crate) fn reject_url_source(path: &std::path::Path) -> Result<(), SmallPictError> {
    let s = path.to_string_lossy();
    if s.starts_with("http://") || s.starts_with("https://") {
        return Err(SmallPictError::Validation {
            message:
                "The API does not fetch URLs: download the image and pass its bytes to optimize()."
                    .into(),
            details: None,
        });
    }
    Ok(())
}
