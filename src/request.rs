//! The parts of a signed API request that do not depend on the transport: path and body
//! preparation, per-attempt headers, and the retry-or-fail decision for a response.
//! `client::Client` and `blocking::BlockingClient` each keep only their send loop.

use crate::crypto::{self, EMPTY_SHA256};
use crate::errors::{is_purge_rate_limited, SmallPictError};
use bytes::Bytes;
use rand::Rng;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE};
use reqwest::{Method, StatusCode};
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const BASE_DELAY: Duration = Duration::from_millis(250);

/// One request as sent on every attempt: URL, the path that is signed, body and its hash,
/// and the idempotency key that stays the same across retries.
pub(crate) struct Prepared {
    pub url: String,
    pub sign_path: String,
    pub body: Option<Bytes>,
    pub body_hash: String,
    pub idempotency_key: String,
}

pub(crate) fn prepare(
    base_url: &str,
    path: &str,
    payload: Option<&Value>,
    idempotency_key: Option<&str>,
) -> Result<Prepared, SmallPictError> {
    let clean_path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };
    let normalized_path = if !clean_path.starts_with("/v1/") && !clean_path.starts_with("/v2/") {
        format!("/v1{}", clean_path)
    } else {
        clean_path
    };
    // The contract signs the path only; the server accepts path?query just as a fallback.
    let sign_path = normalized_path
        .split('?')
        .next()
        .unwrap_or(&normalized_path)
        .to_string();
    let (body, body_hash) = match payload {
        Some(json_val) => {
            let bytes = serde_json::to_vec(json_val).map_err(SmallPictError::from)?;
            let hash = crypto::sha256_hex(&bytes);
            (Some(Bytes::from(bytes)), hash)
        }
        None => (None, EMPTY_SHA256.to_string()),
    };
    Ok(Prepared {
        url: format!("{}{}", base_url, normalized_path),
        sign_path,
        body,
        body_hash,
        idempotency_key: idempotency_key
            .map(ToString::to_string)
            .unwrap_or_else(|| Uuid::new_v4().to_string()),
    })
}

/// Headers of one attempt: the HMAC signature (SDK_SPEC section 1), a fresh nonce and, on
/// mutating methods, the idempotency key.
pub(crate) fn headers(
    api_key: &str,
    secret_key: &str,
    method: &Method,
    prepared: &Prepared,
) -> Result<HeaderMap, SmallPictError> {
    let now_sec = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();

    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        HeaderName::from_static("x-api-key"),
        HeaderValue::from_str(api_key).map_err(|_| SmallPictError::Validation {
            message: "Invalid characters in API key".into(),
            details: None,
        })?,
    );
    if prepared.body.is_some() {
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    }

    let string_to_sign = crypto::build_string_to_sign(
        method.as_str(),
        &prepared.sign_path,
        &now_sec,
        &prepared.body_hash,
    );
    let signature = crypto::hmac_sha256_hex(secret_key, &string_to_sign);
    headers.insert(
        HeaderName::from_static("x-timestamp"),
        HeaderValue::from_str(&now_sec).expect("valid ascii"),
    );
    headers.insert(
        HeaderName::from_static("x-signature"),
        HeaderValue::from_str(&signature).expect("valid ascii"),
    );
    // The server rejects a repeated signature as a replay unless the nonce differs.
    headers.insert(
        HeaderName::from_static("x-nonce"),
        HeaderValue::from_str(&Uuid::new_v4().to_string()).expect("valid uuid"),
    );
    if matches!(*method, Method::POST | Method::PATCH | Method::DELETE) {
        headers.insert(
            HeaderName::from_static("idempotency-key"),
            HeaderValue::from_str(&prepared.idempotency_key).map_err(|_| {
                SmallPictError::Validation {
                    message: "idempotency_key must be visible ASCII".into(),
                    details: None,
                }
            })?,
        );
    }
    Ok(headers)
}

/// How long to wait before attempt `attempt + 1` after a transport error, or `None` when the
/// retries are used up.
pub(crate) fn transport_retry(
    attempt: u32,
    max_retries: u32,
    max_retry_delay: Duration,
) -> Option<Duration> {
    (attempt <= max_retries).then(|| backoff(attempt).min(max_retry_delay))
}

fn backoff(attempt: u32) -> Duration {
    BASE_DELAY * (1u32 << attempt.saturating_sub(1).min(10))
}

/// What to do with a response: wait and try again, or hand the caller its result.
pub(crate) enum Outcome {
    Retry(Duration),
    Done(Result<Value, SmallPictError>),
}

/// The response of one attempt, read without the transport.
pub(crate) struct Response {
    pub status: StatusCode,
    pub request_id: Option<String>,
    pub retry_after: Option<u64>,
    pub body: Bytes,
}

impl Response {
    pub fn new(status: StatusCode, headers: &HeaderMap, body: Bytes) -> Self {
        Response {
            status,
            request_id: headers
                .get("x-request-id")
                .and_then(|v| v.to_str().ok())
                .map(ToString::to_string),
            retry_after: headers
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok()),
            body,
        }
    }
}

pub(crate) fn classify(
    resp: Response,
    attempt: u32,
    max_retries: u32,
    max_retry_delay: Duration,
) -> Outcome {
    let status = resp.status;
    // The hourly purge limit does not reset in seconds, so retrying it only burns attempts.
    let retryable = (status == StatusCode::TOO_MANY_REQUESTS && !is_purge_rate_limited(&resp.body))
        || (status.is_server_error() && status.as_u16() <= 504);
    if retryable && attempt <= max_retries {
        let mut delay = backoff(attempt);
        if let Some(sec) = resp.retry_after {
            delay = Duration::from_secs(sec);
        }
        let jitter = Duration::from_millis(rand::thread_rng().gen_range(0..100));
        return Outcome::Retry(delay.min(max_retry_delay) + jitter);
    }

    let parsed_json: Result<Value, _> = serde_json::from_slice(&resp.body);
    if status.is_success() {
        return Outcome::Done(parsed_json.map_err(SmallPictError::from));
    }

    let mut message = format!("API request failed with HTTP {}", status.as_u16());
    let mut details = None;
    let mut code = None;
    if let Ok(ref val) = parsed_json {
        if let Some(err_obj) = val.get("error") {
            if let Some(m) = err_obj.get("message").and_then(|v| v.as_str()) {
                message = m.to_string();
            }
            if let Some(c) = err_obj.get("code").and_then(|v| v.as_str()) {
                code = Some(c.to_string());
            }
            if let Some(d) = err_obj.get("details") {
                details = Some(d.clone());
            }
        } else if let Some(m) = val.get("message").and_then(|v| v.as_str()) {
            message = m.to_string();
        }
    }
    let request_id = resp.request_id;
    Outcome::Done(Err(match status.as_u16() {
        400 => SmallPictError::Validation { message, details },
        401 => SmallPictError::Unauthorized {
            message,
            request_id,
        },
        402 => SmallPictError::QuotaExceeded {
            message,
            details,
            request_id,
        },
        403 => SmallPictError::Forbidden {
            message,
            details,
            request_id,
        },
        404 => SmallPictError::NotFound {
            message,
            request_id,
        },
        429 => SmallPictError::RateLimit {
            message,
            retry_after: resp.retry_after,
            request_id,
        },
        _ => SmallPictError::Server {
            status: status.as_u16(),
            message,
            code,
            details,
            request_id,
        },
    }))
}
