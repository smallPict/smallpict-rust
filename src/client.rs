use crate::errors::{
    job_failed, job_timeout, reject_url_source, upload_failed, SecretRedacted, SmallPictError,
};
use crate::models::{
    check_folder, passthrough, FallbackMode, JobStatusResult, OptimizeOptions, OptimizeResult,
    PurgeResponse, PurgeType, QueuedJob, QuotaResponse,
};
use crate::request;
use bytes::Bytes;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE};
use serde_json::Value;
use std::env;
use std::path::Path;
use std::time::{Duration, Instant};

/// Asynchronous client for interacting with the SmallPict API.
#[derive(Clone, Debug)]
pub struct Client {
    pub(crate) api_key: SecretRedacted<String>,
    pub(crate) secret_key: SecretRedacted<String>,
    pub(crate) base_url: String,
    pub(crate) timeout: Duration,
    pub(crate) max_retries: u32,
    pub(crate) max_retry_delay: Duration,
    pub(crate) fallback_mode: FallbackMode,
    pub(crate) poll_interval: Duration,
    pub(crate) wait_timeout: Duration,
    pub(crate) http_client: reqwest::Client,
}

impl Client {
    /// Create a new Client with default options. The secret key is read from
    /// `SMALLPICT_SECRET_KEY`; use [`Client::builder`] to pass it explicitly.
    pub fn new(api_key: impl Into<String>) -> Result<Self, SmallPictError> {
        ClientBuilder::new().api_key(api_key).build()
    }

    /// Obtain a [`ClientBuilder`] to configure client options.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    /// Optimizes raw in-memory image bytes.
    ///
    /// Queues the job, PUTs the bytes to the pre-signed `upload_url` the API returns, and
    /// polls the job status every `poll_interval` until it is completed. Returns the finished
    /// job (`url`, `format`, `bytes_saved`, `mode_applied`, `quality_applied`, `folder`).
    /// A retry after a 5xx may create a second job until the server honours `Idempotency-Key`.
    ///
    /// Errors: `SmallPictError::Optimize` with code `job_failed` (the server's message,
    /// `details.job_id`) or `upload_failed` (the storage refused the PUT), and
    /// `SmallPictError::Timeout` naming the job when it is still running after `wait_timeout`.
    /// Optimizes raw in-memory image bytes; see [`Self::optimize_bytes`] to avoid the copy.
    pub async fn optimize(
        &self,
        data: &[u8],
        options: Option<OptimizeOptions>,
    ) -> Result<OptimizeResult, SmallPictError> {
        self.optimize_bytes(Bytes::copy_from_slice(data), options)
            .await
    }

    /// Like [`Self::optimize`] but takes ownership of the buffer, so the upload sends it without a copy.
    pub async fn optimize_bytes(
        &self,
        data: Bytes,
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
            .or_else(|| crate::mime::detect_mime(&data).map(str::to_string))
            .unwrap_or_else(|| "image/jpeg".into());

        let payload = serde_json::json!({
            "filename": filename,
            "mime_type": mime_type,
            "content_type": mime_type,
            "filesize": data.len(),
            "options": serde_json::to_value(&opts).map_err(SmallPictError::from)?,
        });

        let queued = match self
            .request(
                reqwest::Method::POST,
                "/v1/optimize",
                Some(payload),
                opts.idempotency_key.as_deref(),
            )
            .await
        {
            Ok(val) => serde_json::from_value::<QueuedJob>(val)?,
            Err(e) => {
                if e.is_quota_exceeded() && self.fallback_mode == FallbackMode::Passthrough {
                    return Ok(passthrough(&data, &mime_type));
                }
                return Err(e);
            }
        };

        // The worker waits 30 s from the request for the object: upload now, poll after.
        self.upload(&queued.upload_url, data.clone(), &mime_type)
            .await?;

        let deadline = Instant::now() + self.wait_timeout;
        loop {
            tokio::time::sleep(
                self.poll_interval
                    .min(deadline.saturating_duration_since(Instant::now())),
            )
            .await;
            let st = self.get_job_status(&queued.job_id).await?;
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
    async fn upload(&self, url: &str, data: Bytes, mime_type: &str) -> Result<(), SmallPictError> {
        let resp = self
            .http_client
            .put(url)
            .header(CONTENT_TYPE, mime_type)
            .header(CONTENT_LENGTH, data.len())
            .body(data)
            .timeout(self.timeout)
            .send()
            .await
            .map_err(|e| SmallPictError::Network {
                source: e.without_url(),
            })?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(upload_failed(status.as_u16(), &body));
        }
        Ok(())
    }

    /// Reads an image from local disk and performs optimization. An `http(s)://` path
    /// returns `SmallPictError::Validation`: the API does not fetch URLs.
    pub async fn optimize_file(
        &self,
        path: impl AsRef<Path>,
        options: Option<OptimizeOptions>,
    ) -> Result<OptimizeResult, SmallPictError> {
        let path = path.as_ref();
        reject_url_source(path)?;
        let bytes = tokio::fs::read(path).await?;

        let mut opts = options.unwrap_or_default();
        if opts.filename.is_none() {
            if let Some(file_stem) = path.file_name().and_then(|s| s.to_str()) {
                opts.filename = Some(file_stem.to_string());
            }
        }

        self.optimize_bytes(Bytes::from(bytes), Some(opts)).await
    }

    /// Retrieves real-time account quota usage and egress metrics.
    pub async fn get_quota(&self) -> Result<QuotaResponse, SmallPictError> {
        let val = self
            .request(reqwest::Method::GET, "/v1/quota", None, None)
            .await?;
        serde_json::from_value(val).map_err(SmallPictError::from)
    }

    /// Invalidates cached assets across global Edge CDN nodes.
    pub async fn purge_cdn(
        &self,
        urls: &[impl AsRef<str>],
        purge_type: PurgeType,
    ) -> Result<PurgeResponse, SmallPictError> {
        let url_list: Vec<String> = urls.iter().map(|u| u.as_ref().to_string()).collect();
        let payload = serde_json::json!({
            "purge_type": purge_type.to_string(),
            "urls": url_list,
        });

        let val = self
            .request(reqwest::Method::POST, "/v1/purge", Some(payload), None)
            .await?;
        serde_json::from_value(val).map_err(SmallPictError::from)
    }

    /// Purges every file delivered under one upload folder (`purge_type: "folder"`).
    /// A malformed folder returns `SmallPictError::Validation` without an HTTP call.
    /// The server limits folder purges per hour; past the limit it answers 429
    /// `purge_rate_limited` (`SmallPictError::RateLimit`), which is not retried.
    pub async fn purge_folder(&self, folder: &str) -> Result<PurgeResponse, SmallPictError> {
        check_folder(folder)?;
        let payload = serde_json::json!({
            "purge_type": "folder",
            "folder": folder,
        });

        let val = self
            .request(reqwest::Method::POST, "/v1/purge", Some(payload), None)
            .await?;
        serde_json::from_value(val).map_err(SmallPictError::from)
    }

    /// Verifies if the configured API key is valid.
    pub async fn validate_key(&self) -> bool {
        self.get_quota().await.is_ok()
    }

    /// Polls current progress of an asynchronous image conversion task.
    pub async fn get_job_status(&self, job_id: &str) -> Result<JobStatusResult, SmallPictError> {
        if job_id.is_empty() {
            return Err(SmallPictError::Validation {
                message: "job_id is required".into(),
                details: None,
            });
        }

        let path = format!("/v1/optimize/status?job_id={}", urlencoding::encode(job_id));
        let val = self
            .request(reqwest::Method::GET, &path, None, None)
            .await?;
        serde_json::from_value(val).map_err(SmallPictError::from)
    }

    pub(crate) async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        payload: Option<Value>,
        idempotency_key: Option<&str>,
    ) -> Result<Value, SmallPictError> {
        let prepared = request::prepare(&self.base_url, path, payload.as_ref(), idempotency_key)?;
        let mut attempt = 0;

        while attempt <= self.max_retries {
            attempt += 1;
            let headers =
                request::headers(&self.api_key.0, &self.secret_key.0, &method, &prepared)?;
            let mut req = self
                .http_client
                .request(method.clone(), &prepared.url)
                .headers(headers)
                .timeout(self.timeout);
            if let Some(ref b) = prepared.body {
                req = req.body(b.clone());
            }

            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    match request::transport_retry(attempt, self.max_retries, self.max_retry_delay)
                    {
                        Some(delay) => {
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        None => return Err(SmallPictError::Network { source: e }),
                    }
                }
            };
            let status = resp.status();
            let resp_headers = resp.headers().clone();
            let body = resp.bytes().await.map_err(SmallPictError::from)?;
            let response = request::Response::new(status, &resp_headers, body);

            match request::classify(response, attempt, self.max_retries, self.max_retry_delay) {
                request::Outcome::Retry(delay) => tokio::time::sleep(delay).await,
                request::Outcome::Done(result) => return result,
            }
        }

        Err(SmallPictError::Timeout {
            message: "Request failed after maximum retries".into(),
        })
    }
}

/// Builder pattern for configuring and instantiating a [`Client`].
#[derive(Debug, Clone)]
pub struct ClientBuilder {
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

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientBuilder {
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

    pub fn build(self) -> Result<Client, SmallPictError> {
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

        let http_client = reqwest::Client::builder()
            .timeout(self.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(SmallPictError::from)?;

        Ok(Client {
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
