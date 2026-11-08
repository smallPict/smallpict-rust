# Changelog

All notable changes to the `smallpict` crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- `SecretRedacted`'s `Display` prints `[REDACTED]` like `Debug`. It used to run the message sanitizer, whose pattern knew only `sec_`/`secret_` fixtures, so `println!("{}", secret)` printed a real `sps_...` secret in full. `sanitize_message` now masks `sps_...` secrets too.
- Every request carries a fresh `X-Nonce`, so a retry inside the same second, or two same-second requests with identical bodies, are no longer refused by the server as a replay (401 "Replay attack detected"); before, a transient 429 or 5xx could surface as `SmallPictError::Unauthorized`.
- `Idempotency-Key` is generated once per call and kept across retries instead of once per attempt.

### Changed
- **`secret_key` is required.** `ClientBuilder::build()` and `BlockingClientBuilder::build()` return `SmallPictError::Validation` when neither `.secret_key(...)` nor `SMALLPICT_SECRET_KEY` is set. The Bearer-only mode (API key without signature) is gone: the API refuses it with 403, so a client built that way never worked. `Client::new(api_key)` still reads the secret from the environment. README and crate docs pass both keys.
- Release workflow runs `cargo test --all-features` before `cargo publish`; a tag on a commit that fails it no longer publishes. CI also builds on the declared `rust-version` (1.75.0) and runs `cargo audit`. Actions are pinned to commit SHAs.

## [0.1.0] - 2026-11-05

### Fixed
- **`optimize()` now returns an optimized image.** `POST /v1/optimize` only queues the job and returns a pre-signed `upload_url`; the SDK stopped there, so the worker failed every job with "client upload never arrived" and callers got an empty `url` with 0 % saved. `Client::optimize()` and `BlockingClient::optimize()` now PUT the bytes to `upload_url` (plain PUT with `Content-Type` and `Content-Length` as sent, no API headers, no retry), poll `GET /v1/optimize/status` every `poll_interval` (default 1 s) and return once the job is `completed`, with `url`, `format`, `bytes_saved`, `mode_applied`, `quality_applied` and `folder` from the job status (`SDK_SPEC.md` section 3d).

### Added
- `ClientBuilder::poll_interval()` and `wait_timeout()` (`Duration`, defaults 1 s and 120 s), also on `BlockingClientBuilder`.
- `OptimizeResult::mode_applied` (`Option<String>`) and `quality_applied` (`Option<u32>`).
- `SmallPictError::Optimize { code, message, status, details }`: code `job_failed` (a failed job, with the server's `error.message` and `details.job_id`) or `upload_failed` (the storage refused the PUT, with its HTTP status in `status`). `code()` and `details()` return them. A new variant of a public enum: an exhaustive `match` on `SmallPictError` needs an `Optimize` arm (source-breaking). A job still running after `wait_timeout` returns `SmallPictError::Timeout` with the job id in the message; it keeps running on the server.

### Changed
- **`OptimizeResult::status` is always `"completed"`** and `OptimizeResult::upload_url` is gone: there is no result to return before the job finished. Code that checked for `"queued"` or used `upload_url` to upload by itself can drop that.
- **An `http(s)://` path passed to `optimize_file()` returns `SmallPictError::Validation`** before any request. The API never fetched URLs; the SDK does not download on your behalf.
- `compressed_size` and `savings_percentage` are derived from the bytes sent and `bytes_saved` of the job status; `url` and `format` are never filled from the request.
- README: "What `optimize()` does".

## [0.0.7] - 2026-10-31

### Added
- `CompressionMode::Auto` (`"auto"`): the server picks the quality per image from a visual score (CMP-07). Plans: WP Pro and Business, Velocity, Momentum and sandbox; a server without auto enabled answers `400 invalid_mode`. A new variant of a public enum: an exhaustive `match` on `CompressionMode` needs an `Auto` arm (source-breaking).
- `CompressionMode` enum (`Lossy`, `Extreme`, `Lossless`; serialized lowercase) and `OptimizeOptions::mode` / `OptimizeOptionsBuilder::mode()`, sent as `options.mode` on `POST /v1/optimize` (FND-07). Omitted when unset, so the server default (`lossy`) applies.
- `JobStatusResult::mode_applied` (`Option<String>`, so new server values never break parsing) and `JobStatusResult::quality_applied` (`Option<u32>`), filled only from the server response and absent for older jobs.
- `OptimizeOptions::folder` (`Option<String>`) and `OptimizeOptionsBuilder::folder()`, sent as `options.folder` on `POST /v1/optimize` (FLD-03). Omitted when unset. A folder with an empty segment (`a//b`, leading or trailing `/`), a `..` segment or more than 3 segments returns `SmallPictError::Validation` without an HTTP call; the server's 400 `invalid_folder` maps to `SmallPictError::Validation` with the server's message.
- `JobStatusResult::folder` (`Option<String>`): the folder stored with the job, as the server returns it (lower case). Absent for jobs without a folder.
- `OptimizeResult::folder` (`Option<String>`): the folder the server stored for the upload, as returned in the `POST /v1/optimize` response (lower case). `None` without a folder and for a passthrough fallback.
- `Client::purge_folder()` and `BlockingClient::purge_folder()`, which send `{"purge_type": "folder", "folder": ...}` to `POST /v1/purge`. The folder is checked like `options.folder` before the request. `purge_cdn()` and `PurgeType` are unchanged.
- `SmallPictError::Forbidden` carries `details: Option<serde_json::Value>`, so HTTP 403 `mode_not_in_plan` exposes `details.mode` and `details.allowed_modes`.
- **`get_quota()` returns read-only capacity (SDK-02).** `QuotaResponse::capacity` (`Option<Capacity>`) carries `period_end`, `storage` (`CapacityStorage`), `cdn` (`CapacityCdn`), `booster` (`CapacityBooster` with `next_expiry: Option<BoosterExpiry>`), `lapse` (`CapacityLapse`) and `manage_url`. Members the contract marks nullable are `Option`; byte counts are `i64`; `cdn.status`, `lapse.stage` and `next_expiry.kind` are strings so new server values still parse. `capacity` is `None` when the server sends `null` and when an older server omits it.
- **Upgrade links on errors.** `SmallPictError::upgrade_url()` returns `details.upgrade_url` as a plain string, or `None` when it is absent or not a string. The SDK never opens or fetches it, nor `manage_url`.
- `SmallPictError::allowed_modes()` returns `details.allowed_modes` of a 403 `mode_not_in_plan`; `SmallPictError::details()` returns `error.details` of any variant that carries them; `SmallPictError::code()` returns the server error code of a `Server` error.
- `SmallPictError::QuotaExceeded` carries `details` (402 `QUOTA_EXCEEDED`, `storage_quota_exceeded`, `account_read_only`), and `SmallPictError::Server` carries `code` and `details`, so the 409 capacity codes (`booster_period_limit_exceeded` and the others) expose their code, status 409 and upgrade link. The status-to-variant mapping is unchanged; no variant is added.

### Deprecated
- `OptimizeOptions::lossless` and `OptimizeOptionsBuilder::lossless()`; use `mode` (`CompressionMode::Lossless`). Removal not before the next major version. `lossless = true` without a mode is sent as `mode: "lossless"`; `lossless = true` with a different explicit mode returns `SmallPictError::Validation` without an HTTP call. `lossless` is never sent on the wire.

### Changed
- HTTP 429 with error code `purge_rate_limited` (hourly limit of folder and `all` purges) is no longer retried automatically; it returns `SmallPictError::RateLimit` after one request. Other 429 responses are retried as before.

### Fixed
- `OptimizeOptions::quality` now defaults to `None` and is sent only when set (CMP-04). The SDK used to send `quality: 80` on every request, so `mode: extreme` produced lossy-Q80-sized files; the server now applies its default per mode (lossy 80, extreme a per-codec default, lossless ignores quality). Lossy results are unchanged. Code that read `OptimizeOptions::default().quality` as `Some(80)` now gets `None`.
- README: compression mode section shows `CompressionMode::Extreme` without a quality.
- `lossless(true)` used to be ignored by the server; it is now honoured (sent as `mode: "lossless"`).

### Compatibility
- Source-breaking for code that builds `OptimizeOptions` or `JobStatusResult` with struct literals without `..Default::default()` (neither struct is `#[non_exhaustive]`; `OptimizeOptions` implements `Default`, `JobStatusResult` does not), and for code that constructs or matches `SmallPictError::Forbidden { message, request_id }` without `..`. `OptimizeOptions::folder` and `JobStatusResult::folder` fall under the same note; they add no break beyond the one this release already makes to those structs. **`OptimizeResult::folder` is a new source break:** `OptimizeResult` (published in 0.0.6) is not `#[non_exhaustive]` and has no `Default`, so code that builds it with a struct literal (for example a test double) must now add `folder: None`. Parsing a response is not affected. Code that sets the `lossless` field or calls `.lossless()` now gets a deprecation warning.
- **`QuotaResponse::capacity`, `SmallPictError::QuotaExceeded::details`, `SmallPictError::Server::{code, details}` are new source breaks.** `QuotaResponse` is not `#[non_exhaustive]` and has no `Default`, so a struct literal (for example a test double) must add `capacity: None`. Code that constructs `QuotaExceeded` or `Server`, or matches them without `..`, must add the new fields. Parsing responses is not affected.

## [1.0.0] - 2026-08-22

### Added
- Official Rust SDK implementation for SmallPict OpenAPI 3.1.0 API.
- Zero-leak credential security with `SecretRedacted<T>` wrapper and regex masking in `Display` and `Debug`.
- High-concurrency asynchronous client `Client` powered by `tokio` and `reqwest`.
- Synchronous client `BlockingClient` enabled via the `blocking` feature flag.
- 4 unified core client methods: `optimize()`, `get_quota()`, `purge_cdn()`, and `validate_key()`.
- Helper `get_job_status()` for polling asynchronous image conversion tasks.
- Builder pattern for `ClientBuilder`, `BlockingClientBuilder`, and `OptimizeOptionsBuilder`.
- Resilient HTTP transport with 30s timeouts, exponential backoff, jitter, and automatic `Idempotency-Key` UUID injection.
- Optional `FallbackMode::Passthrough` for uninterrupted availability on quota limit exhaustion.
