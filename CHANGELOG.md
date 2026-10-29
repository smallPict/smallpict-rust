# Changelog

All notable changes to the `smallpict` crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `CompressionMode` enum (`Lossy`, `Extreme`, `Lossless`; serialized lowercase) and `OptimizeOptions::mode` / `OptimizeOptionsBuilder::mode()`, sent as `options.mode` on `POST /v1/optimize` (FND-07). Omitted when unset, so the server default (`lossy`) applies.
- `JobStatusResult::mode_applied` (`Option<String>`, so new server values never break parsing) and `JobStatusResult::quality_applied` (`Option<u32>`), filled only from the server response and absent for older jobs.
- `SmallPictError::Forbidden` carries `details: Option<serde_json::Value>`, so HTTP 403 `mode_not_in_plan` exposes `details.mode` and `details.allowed_modes`.

### Deprecated
- `OptimizeOptions::lossless` and `OptimizeOptionsBuilder::lossless()`; use `mode` (`CompressionMode::Lossless`). Removal not before the next major version. `lossless = true` without a mode is sent as `mode: "lossless"`; `lossless = true` with a different explicit mode returns `SmallPictError::Validation` without an HTTP call. `lossless` is never sent on the wire.

### Fixed
- `lossless(true)` used to be ignored by the server; it is now honoured (sent as `mode: "lossless"`).

### Compatibility
- Source-breaking for code that builds `OptimizeOptions` or `JobStatusResult` with struct literals without `..Default::default()` (neither struct is `#[non_exhaustive]`; `OptimizeOptions` implements `Default`, `JobStatusResult` does not), and for code that constructs or matches `SmallPictError::Forbidden { message, request_id }` without `..`. Code that sets the `lossless` field or calls `.lossless()` now gets a deprecation warning.

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
