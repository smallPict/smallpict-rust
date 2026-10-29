# Changelog

All notable changes to the `smallpict` crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `CompressionMode` enum (`Lossy`, `Extreme`, `Lossless`; serialized lowercase) and `OptimizeOptions::mode` / `OptimizeOptionsBuilder::mode()`, sent as `options.mode` on `POST /v1/optimize` (FND-07). Omitted when unset, so the server default (`lossy`) applies.
- `JobStatusResult::mode_applied` (`Option<String>`, so new server values never break parsing) and `JobStatusResult::quality_applied` (`Option<u32>`), filled only from the server response and absent for older jobs.
- `OptimizeOptions::folder` (`Option<String>`) and `OptimizeOptionsBuilder::folder()`, sent as `options.folder` on `POST /v1/optimize` (FLD-03). Omitted when unset. A folder with an empty segment (`a//b`, leading or trailing `/`), a `..` segment or more than 3 segments returns `SmallPictError::Validation` without an HTTP call; the server's 400 `invalid_folder` maps to `SmallPictError::Validation` with the server's message.
- `JobStatusResult::folder` (`Option<String>`): the folder stored with the job, as the server returns it (lower case). Absent for jobs without a folder.
- `OptimizeResult::folder` (`Option<String>`): the folder the server stored for the upload, as returned in the `POST /v1/optimize` response (lower case). `None` without a folder and for a passthrough fallback.
- `Client::purge_folder()` and `BlockingClient::purge_folder()`, which send `{"purge_type": "folder", "folder": ...}` to `POST /v1/purge`. The folder is checked like `options.folder` before the request. `purge_cdn()` and `PurgeType` are unchanged.
- `SmallPictError::Forbidden` carries `details: Option<serde_json::Value>`, so HTTP 403 `mode_not_in_plan` exposes `details.mode` and `details.allowed_modes`.

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
