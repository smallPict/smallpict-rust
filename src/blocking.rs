use crate::crypto::{self, EMPTY_SHA256};
use crate::errors::{
    is_purge_rate_limited, job_failed, job_timeout, reject_url_source, upload_failed,
    SecretRedacted, SmallPictError,
};
use crate::models::{
    check_folder, passthrough, FallbackMode, JobStatusResult, OptimizeOptions, OptimizeResult,
    PurgeResponse, PurgeType, QueuedJob, QuotaResponse,
};
use bytes::Bytes;
use rand::Rng;
use reqwest::blocking::Client as ReqwestBlockingClient;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_LENGTH, CONTENT_TYPE};
use serde_json::Value;
use std::env;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Synchronous (blocking) client for interacting with the SmallPict API.
#[derive(Clone, Debug)]
pub struct BlockingClient {
    pub(crate) api_key: SecretRedacted<String>,
    pub(crate) secret_key: SecretRedacted<String>,
    pub(crate) base_url: String,
    pub(crate) timeout: Duration,
    pub(crate) max_retries: u32,
    pub(crate) max_retry_delay: Duration,
    pub(crate) fallback_mode: FallbackMode,
    pub(crate) poll_interval: Duration,
    pub(crate) wait_timeout: Duration,
    pub(crate) http_client: ReqwestBlockingClient,
}

impl BlockingClient {
    /// Create a new synchronous Client with default options. The secret key is read from
    /// `SMALLPICT_SECRET_KEY`; use [`BlockingClient::builder`] to pass it explicitly.
    pub fn new(api_key: impl Into<String>) -> Result<Self, SmallPictError> {
        BlockingClientBuilder::new().api_key(api_key).build()
    }

    /// Obtain a [`BlockingClientBuilder`] to configure blocking client options.
    pub fn builder() -> BlockingClientBuilder {
        BlockingClientBuilder::new()
    }

    /// Optimizes raw in-memory image bytes synchronously.
    ///
    /// Queues the job, PUTs the bytes to the pre-signed `upload_url` the API returns, and
    /// polls the job status every `poll_interval` until it is completed. Returns the finished
    /// job (`url`, `format`, `bytes_saved`, `mode_applied`, `quality_applied`, `folder`).
    /// A retry after a 5xx may create a second job until the server honours `Idempotency-Key`.
    ///
    /// Errors: `SmallPictError::Optimize` with code `job_failed` (the server's message,
    /// `details.job_id`) or `upload_failed` (the storage refused the PUT), and
    /// `SmallPictError::Timeout` naming the job when it is still running after `wait_timeout`.
    pub fn optimize(
        &self,
        data: &[u8],
        options: Option<OptimizeOptions>,
    ) -> Result<OptimizeResult, SmallPictError> {
        let mut opts = options.unwrap_or_default();
        opts.mode = opts.wire_mode()?;
        if let Some(ref folder) = opts.folder {
            check_folder(folder)?;
        }
        let filename = opts.filename.clone().unwrap_or_else(|| "image.jpg".into());
        let mime_type = opts
            .mime_type
            .clone()
            .or_else(|| opts.content_type.clone())
            .or_else(|| crate::mime::detect_mime(data).map(str::to_string))
            .unwrap_or_else(|| "image/jpeg".into());

        let payload = serde_json::json!({
            "filename": filename,
            "mime_type": mime_type,
            "content_type": mime_type,
            "filesize": data.len(),
            "options": serde_json::to_value(&opts).map_err(SmallPictError::from)?,
        });

        let queued = match self.request(
            reqwest::Method::POST,
            "/v1/optimize",
            Some(payload),
            opts.idempotency_key.as_deref(),
        ) {
            Ok(val) => serde_json::from_value::<QueuedJob>(val)?,
            Err(e) => {
                if e.is_quota_exceeded() && self.fallback_mode == FallbackMode::Passthrough {
                    return Ok(passthrough(data, &mime_type));
                }
                return Err(e);
            }
        };

        // The worker waits 30 s from the request for the object: upload now, poll after.
        self.upload(&queued.upload_url, data, &mime_type)?;

        let deadline = Instant::now() + self.wait_timeout;
        loop {
            std::thread::sleep(
                self.poll_interval
                    .min(deadline.saturating_duration_since(Instant::now())),
            );
            let st = self.get_job_status(&queued.job_id)?;
            match st.status.as_str() {
                "completed" => return Ok(OptimizeResult::from_status(st, data.len() as u64)),
                "failed" => return Err(job_failed(&st.job_id, st.error.as_ref())),
                _ if Instant::now() >= deadline => {
                    return Err(job_timeout(&queued.job_id, self.wait_timeout))
                }
                _ => {}
            }
        }
    }

    /// PUTs the bytes to the pre-signed `upload_url` with only `Content-Type` and
    /// `Content-Length`: S3 refuses other signed headers. No retry: the worker stops
    /// waiting for the object 30 s after the job was queued.
    fn upload(&self, url: &str, data: &[u8], mime_type: &str) -> Result<(), SmallPictError> {
        let resp = self
            .http_client
            .put(url)
            .header(CONTENT_TYPE, mime_type)
            .header(CONTENT_LENGTH, data.len())
            .body(data.to_vec())
            .timeout(self.timeout)
            .send()
            .map_err(|e| SmallPictError::Network {
                source: e.without_url(),
            })?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            return Err(upload_failed(status.as_u16(), &body));
        }
        Ok(())
    }

    /// Reads an image file from local disk and performs synchronous optimization. An
    /// `http(s)://` path returns `SmallPictError::Validation`: the API does not fetch URLs.
    pub fn optimize_file(
        &self,
        path: impl AsRef<Path>,
        options: Option<OptimizeOptions>,
    ) -> Result<OptimizeResult, SmallPictError> {
        let path = path.as_ref();
        reject_url_source(path)?;
        let bytes = fs::read(path)?;

        let mut opts = options.unwrap_or_default();
        if opts.filename.is_none() {
            if let Some(file_stem) = path.file_name().and_then(|s| s.to_str()) {
                opts.filename = Some(file_stem.to_string());
            }
        }

        self.optimize(&bytes, Some(opts))
    }

    /// Retrieves real-time account quota usage synchronously.
    pub fn get_quota(&self) -> Result<QuotaResponse, SmallPictError> {
        let val = self.request(reqwest::Method::GET, "/v1/quota", None, None)?;
        serde_json::from_value(val).map_err(SmallPictError::from)
    }

    /// Invalidates cached assets across global Edge CDN nodes synchronously.
    pub fn purge_cdn(
        &self,
        urls: &[impl AsRef<str>],
        purge_type: PurgeType,
    ) -> Result<PurgeResponse, SmallPictError> {
        let url_list: Vec<String> = urls.iter().map(|u| u.as_ref().to_string()).collect();
        let payload = serde_json::json!({
            "purge_type": purge_type.to_string(),
            "urls": url_list,
        });

        let val = self.request(reqwest::Method::POST, "/v1/purge", Some(payload), None)?;
        serde_json::from_value(val).map_err(SmallPictError::from)
    }

    /// Purges every file delivered under one upload folder (`purge_type: "folder"`).
    /// A malformed folder returns `SmallPictError::Validation` without an HTTP call.
    /// The server limits folder purges per hour; past the limit it answers 429
    /// `purge_rate_limited` (`SmallPictError::RateLimit`), which is not retried.
    pub fn purge_folder(&self, folder: &str) -> Result<PurgeResponse, SmallPictError> {
        check_folder(folder)?;
        let payload = serde_json::json!({
            "purge_type": "folder",
            "folder": folder,
        });

        let val = self.request(reqwest::Method::POST, "/v1/purge", Some(payload), None)?;
        serde_json::from_value(val).map_err(SmallPictError::from)
    }

    /// Verifies if the configured API key is valid synchronously.
    pub fn validate_key(&self) -> bool {
        self.get_quota().is_ok()
    }

    /// Polls current progress of an asynchronous image conversion task synchronously.
    pub fn get_job_status(&self, job_id: &str) -> Result<JobStatusResult, SmallPictError> {
        if job_id.is_empty() {
            return Err(SmallPictError::Validation {
                message: "job_id is required".into(),
                details: None,
            });
        }

        let path = format!("/v1/optimize/status?job_id={}", urlencoding::encode(job_id));
        let val = self.request(reqwest::Method::GET, &path, None, None)?;
        serde_json::from_value(val).map_err(SmallPictError::from)
    }

    pub(crate) fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        payload: Option<Value>,
        idempotency_key: Option<&str>,
    ) -> Result<Value, SmallPictError> {
        let clean_path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{}", path)
        };
        let normalized_path = if !clean_path.starts_with("/v1/") && !clean_path.starts_with("/v2/")
        {
            format!("/v1{}", clean_path)
        } else {
            clean_path
        };

        let url = format!("{}{}", self.base_url, normalized_path);
        // The contract signs the path only; the server accepts path?query just as a fallback.
        let sign_path = normalized_path
            .split('?')
            .next()
            .unwrap_or(&normalized_path);

        let (body_bytes, body_hash) = if let Some(ref json_val) = payload {
            let bytes = serde_json::to_vec(json_val).map_err(SmallPictError::from)?;
            let hash = crypto::sha256_hex(&bytes);
            (Some(Bytes::from(bytes)), hash)
        } else {
            (None, EMPTY_SHA256.to_string())
        };

        let idemp = idempotency_key
            .map(ToString::to_string)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let mut attempt = 0;
        let base_delay = Duration::from_millis(250);

        while attempt <= self.max_retries {
            attempt += 1;

            let now_sec = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .to_string();

            let mut headers = HeaderMap::new();
            headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
            headers.insert(
                HeaderName::from_static("x-api-key"),
                HeaderValue::from_str(&self.api_key.0).map_err(|_| SmallPictError::Validation {
                    message: "Invalid characters in API key".into(),
                    details: None,
                })?,
            );

            if body_bytes.is_some() {
                headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            }

            let string_to_sign =
                crypto::build_string_to_sign(method.as_str(), sign_path, &now_sec, &body_hash);
            let signature = crypto::hmac_sha256_hex(&self.secret_key.0, &string_to_sign);
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
            if matches!(
                method,
                reqwest::Method::POST | reqwest::Method::PATCH | reqwest::Method::DELETE
            ) {
                headers.insert(
                    HeaderName::from_static("idempotency-key"),
                    HeaderValue::from_str(&idemp).map_err(|_| SmallPictError::Validation {
                        message: "idempotency_key must be visible ASCII".into(),
                        details: None,
                    })?,
                );
            }

            let mut req = self
                .http_client
                .request(method.clone(), &url)
                .headers(headers)
                .timeout(self.timeout);

            if let Some(ref b) = body_bytes {
                req = req.body(b.clone());
            }

            let resp = match req.send() {
                Ok(r) => r,
                Err(e) => {
                    if attempt <= self.max_retries {
                        let delay = (base_delay * (1u32 << attempt.saturating_sub(1).min(10)))
                            .min(self.max_retry_delay);
                        std::thread::sleep(delay);
                        continue;
                    }
                    return Err(SmallPictError::Network { source: e });
                }
            };

            let status = resp.status();
            let req_id = resp
                .headers()
                .get("x-request-id")
                .and_then(|v| v.to_str().ok())
                .map(ToString::to_string);
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());

            let resp_bytes = resp.bytes().map_err(SmallPictError::from)?;

            if ((status == reqwest::StatusCode::TOO_MANY_REQUESTS
                && !is_purge_rate_limited(&resp_bytes))
                || (status.is_server_error() && status.as_u16() <= 504))
                && attempt <= self.max_retries
            {
                let mut delay = base_delay * (1u32 << attempt.saturating_sub(1).min(10));
                if let Some(sec) = retry_after {
                    delay = Duration::from_secs(sec);
                }
                delay = delay.min(self.max_retry_delay);
                let jitter = Duration::from_millis(rand::thread_rng().gen_range(0..100));
                std::thread::sleep(delay + jitter);
                continue;
            }

            let parsed_json: Result<Value, _> = serde_json::from_slice(&resp_bytes);

            if !status.is_success() {
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

                return match status.as_u16() {
                    400 => Err(SmallPictError::Validation { message, details }),
                    401 => Err(SmallPictError::Unauthorized {
                        message,
                        request_id: req_id,
                    }),
                    402 => Err(SmallPictError::QuotaExceeded {
                        message,
                        details,
                        request_id: req_id,
                    }),
                    403 => Err(SmallPictError::Forbidden {
                        message,
                        details,
                        request_id: req_id,
                    }),
                    404 => Err(SmallPictError::NotFound {
                        message,
                        request_id: req_id,
                    }),
                    429 => Err(SmallPictError::RateLimit {
                        message,
                        retry_after,
                        request_id: req_id,
                    }),
                    _ => Err(SmallPictError::Server {
                        status: status.as_u16(),
                        message,
                        code,
                        details,
                        request_id: req_id,
                    }),
                };
            }

            return parsed_json.map_err(SmallPictError::from);
        }

        Err(SmallPictError::Timeout {
            message: "Request failed after maximum retries".into(),
        })
    }
}

/// Builder pattern for configuring and instantiating a [`BlockingClient`].
#[derive(Debug, Clone)]
pub struct BlockingClientBuilder {
    api_key: Option<String>,
    secret_key: Option<String>,
    base_url: Option<String>,
    timeout: Duration,
    max_retries: u32,
    max_retry_delay: Duration,
    fallback_mode: FallbackMode,
    poll_interval: Duration,
    wait_timeout: Duration,
}

impl Default for BlockingClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockingClientBuilder {
    pub fn new() -> Self {
        Self {
            api_key: env::var("SMALLPICT_API_KEY").ok(),
            secret_key: env::var("SMALLPICT_SECRET_KEY").ok(),
            base_url: env::var("SMALLPICT_BASE_URL").ok(),
            timeout: Duration::from_secs(30),
            max_retries: 3,
            max_retry_delay: Duration::from_secs(30),
            fallback_mode: FallbackMode::Throw,
            poll_interval: Duration::from_secs(1),
            wait_timeout: Duration::from_secs(120),
        }
    }

    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    pub fn secret_key(mut self, secret: impl Into<String>) -> Self {
        self.secret_key = Some(secret.into());
        self
    }

    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into().trim_end_matches('/').to_string());
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// Longest wait between attempts, whatever `Retry-After` says (default 30 s).
    pub fn max_retry_delay(mut self, delay: Duration) -> Self {
        self.max_retry_delay = delay;
        self
    }

    pub fn fallback_mode(mut self, mode: FallbackMode) -> Self {
        self.fallback_mode = mode;
        self
    }

    /// How often `optimize()` polls the job status while waiting (default 1 s).
    pub fn poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// How long `optimize()` waits for the job to finish before `SmallPictError::Timeout`
    /// (default 120 s). The job keeps running on the server.
    pub fn wait_timeout(mut self, timeout: Duration) -> Self {
        self.wait_timeout = timeout;
        self
    }

    pub fn build(self) -> Result<BlockingClient, SmallPictError> {
        let key = self.api_key.ok_or_else(|| SmallPictError::Validation {
            message: "Missing required SmallPict API key. Provide `.api_key(...)` or set SMALLPICT_API_KEY environment variable.".into(),
            details: None,
        })?;

        if key.trim().is_empty() {
            return Err(SmallPictError::Validation {
                message: "API key cannot be empty".into(),
                details: None,
            });
        }

        let secret = self
            .secret_key
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| SmallPictError::Validation {
                message: "Missing required SmallPict secret key. Provide `.secret_key(...)` or set SMALLPICT_SECRET_KEY environment variable.".into(),
                details: None,
            })?;

        let base = self
            .base_url
            .unwrap_or_else(|| "https://api.smallpict.app".to_string());

        let http_client = ReqwestBlockingClient::builder()
            .timeout(self.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(SmallPictError::from)?;

        Ok(BlockingClient {
            api_key: SecretRedacted(key),
            secret_key: SecretRedacted(secret),
            base_url: base,
            timeout: self.timeout,
            max_retries: self.max_retries,
            max_retry_delay: self.max_retry_delay,
            fallback_mode: self.fallback_mode,
            poll_interval: self.poll_interval,
            wait_timeout: self.wait_timeout,
            http_client,
        })
    }
}
