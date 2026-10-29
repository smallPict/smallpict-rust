use crate::errors::SmallPictError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Output format for image transcoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    #[default]
    Auto,
    Avif,
    Webp,
    Jpeg,
    Png,
}

impl fmt::Display for ImageFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::Avif => write!(f, "avif"),
            Self::Webp => write!(f, "webp"),
            Self::Jpeg => write!(f, "jpeg"),
            Self::Png => write!(f, "png"),
        }
    }
}

/// Bounding box resizing strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FitMode {
    #[default]
    Cover,
    Contain,
    Inside,
    Outside,
}

impl fmt::Display for FitMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cover => write!(f, "cover"),
            Self::Contain => write!(f, "contain"),
            Self::Inside => write!(f, "inside"),
            Self::Outside => write!(f, "outside"),
        }
    }
}

/// Action to take if storage or processing quota limit is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FallbackMode {
    #[default]
    Throw,
    Passthrough,
}

/// Compression mode of `POST /v1/optimize` (`options.mode`). The server applies
/// `Lossy` when no mode is sent and rejects a mode the plan does not include
/// with HTTP 403 `mode_not_in_plan`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CompressionMode {
    #[default]
    Lossy,
    Extreme,
    Lossless,
}

impl fmt::Display for CompressionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lossy => write!(f, "lossy"),
            Self::Extreme => write!(f, "extreme"),
            Self::Lossless => write!(f, "lossless"),
        }
    }
}

/// CDN cache invalidation scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PurgeType {
    #[default]
    Url,
    All,
}

impl fmt::Display for PurgeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Url => write!(f, "url"),
            Self::All => write!(f, "all"),
        }
    }
}

/// Image optimization options payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizeOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<ImageFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_dimension: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fit: Option<FitMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<CompressionMode>,
    /// Deprecated: `Some(true)` is sent as `mode: "lossless"`; never sent on the wire.
    #[deprecated(note = "use mode")]
    #[serde(skip_serializing)]
    pub lossless: Option<bool>,
    pub strip_metadata: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
}

#[allow(deprecated)]
impl Default for OptimizeOptions {
    fn default() -> Self {
        Self {
            format: Some(ImageFormat::Auto),
            quality: Some(80),
            max_width: None,
            max_height: None,
            max_dimension: None,
            fit: Some(FitMode::Cover),
            mode: None,
            lossless: Some(false),
            strip_metadata: true,
            filename: None,
            mime_type: None,
            content_type: None,
            idempotency_key: None,
        }
    }
}

impl OptimizeOptions {
    pub fn builder() -> OptimizeOptionsBuilder {
        OptimizeOptionsBuilder::default()
    }

    /// Mode to send on the wire: `mode` if set, else `Lossless` when the deprecated
    /// `lossless` flag is true. A flag that contradicts an explicit mode is an error.
    #[allow(deprecated)]
    pub(crate) fn wire_mode(&self) -> Result<Option<CompressionMode>, SmallPictError> {
        match (self.mode, self.lossless == Some(true)) {
            (Some(m), true) if m != CompressionMode::Lossless => Err(SmallPictError::Validation {
                message: format!(
                    "lossless = true contradicts mode = \"{}\"; set only mode",
                    m
                ),
                details: None,
            }),
            (None, true) => Ok(Some(CompressionMode::Lossless)),
            (m, _) => Ok(m),
        }
    }
}

/// Builder for constructing [`OptimizeOptions`].
#[derive(Debug, Default, Clone)]
pub struct OptimizeOptionsBuilder {
    options: OptimizeOptions,
}

impl OptimizeOptionsBuilder {
    pub fn format(mut self, format: ImageFormat) -> Self {
        self.options.format = Some(format);
        self
    }

    pub fn quality(mut self, quality: u8) -> Self {
        self.options.quality = Some(quality.clamp(1, 100));
        self
    }

    pub fn max_width(mut self, width: u32) -> Self {
        self.options.max_width = Some(width);
        self
    }

    pub fn max_height(mut self, height: u32) -> Self {
        self.options.max_height = Some(height);
        self
    }

    pub fn max_dimension(mut self, dimension: u32) -> Self {
        self.options.max_dimension = Some(dimension);
        self
    }

    pub fn fit(mut self, fit: FitMode) -> Self {
        self.options.fit = Some(fit);
        self
    }

    pub fn mode(mut self, mode: CompressionMode) -> Self {
        self.options.mode = Some(mode);
        self
    }

    #[deprecated(note = "use mode")]
    #[allow(deprecated)]
    pub fn lossless(mut self, lossless: bool) -> Self {
        self.options.lossless = Some(lossless);
        self
    }

    pub fn strip_metadata(mut self, strip: bool) -> Self {
        self.options.strip_metadata = strip;
        self
    }

    pub fn filename(mut self, name: impl Into<String>) -> Self {
        self.options.filename = Some(name.into());
        self
    }

    pub fn mime_type(mut self, mime: impl Into<String>) -> Self {
        self.options.mime_type = Some(mime.into());
        self
    }

    pub fn content_type(mut self, ct: impl Into<String>) -> Self {
        self.options.content_type = Some(ct.into());
        self
    }

    pub fn idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.options.idempotency_key = Some(key.into());
        self
    }

    pub fn build(self) -> OptimizeOptions {
        self.options
    }
}

/// Result of an image optimization operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizeResult {
    pub job_id: String,
    pub status: String,
    pub url: String,
    pub format: String,
    pub original_size: u64,
    pub compressed_size: u64,
    pub bytes_saved: u64,
    pub savings_percentage: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_url: Option<String>,
    #[serde(skip)]
    pub data: Option<Vec<u8>>,
}

/// Real-time quota and usage metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaResponse {
    pub plan: String,
    pub bytes_used: u64,
    pub quota_limit: u64,
    pub quota_percentage: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cdn_egress_used_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cdn_egress_quota_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_keys_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_sites_count: Option<u32>,
}

/// CDN cache purge response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PurgeResponse {
    pub message: String,
}

/// Progress status of an asynchronous image conversion task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStatusResult {
    pub job_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_saved: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    /// Mode the server applied; a string so new server values never break parsing.
    /// Absent for jobs created before compression modes existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode_applied: Option<String>,
    /// Quality the server applied. Absent for older jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality_applied: Option<u32>,
}
