# SmallPict Rust SDK

Official Rust SDK for the [SmallPict Image Optimization API](https://smallpict.app) — high-performance next-gen image transcoding (AVIF, WebP), smart compression, Edge CDN delivery, and cache purging.

[![Crates.io](https://img.shields.io/crates/v/smallpict.svg)](https://crates.io/crates/smallpict)
[![Documentation](https://docs.rs/smallpict/badge.svg)](https://docs.rs/smallpict)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

---

## ⚡ Features

- **🦀 Pure Rust TLS Stack:** Uses `rustls-tls` by default to avoid OpenSSL C-runtime issues.
- **⚡ Dual Asynchronous & Blocking Modes:** High-concurrency `tokio` async client and optional sync `BlockingClient`.
- **🛡️ HMAC-SHA256 request signing:** every request is signed with your secret key; both `api_key` and `secret_key` are required.
- **✨ 4 Core Unified Methods:** `optimize()` (uploads the image and waits for the finished job), `get_quota()`, `purge_cdn()`, and `validate_key()`.
- **🔄 Resilience & Fault Tolerance:** Automatic `Idempotency-Key` UUID injection, 30s timeouts, and exponential backoff with jitter on HTTP 429/5xx.
- **🔒 Zero-Leak Privacy:** API keys and credentials are automatically wrapped in `SecretRedacted` and masked in `Display`, `Debug`, and error logs.

---

## 📥 Installation

In your `Cargo.toml`:

```toml
[dependencies]
smallpict = "0.1"
```

To enable synchronous / blocking mode:

```toml
[dependencies]
smallpict = { version = "0.1", features = ["blocking"] }
```

---

## 🚀 Quick Start

### 1. Asynchronous Example (Tokio)

```rust
use smallpict::{Client, ImageFormat, OptimizeOptions};
use std::fs;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::builder()
        .api_key(std::env::var("SMALLPICT_API_KEY")?)
        .secret_key(std::env::var("SMALLPICT_SECRET_KEY")?)
        .build()?;

    let image_bytes = fs::read("hero-banner.png")?;

    let options = OptimizeOptions::builder()
        .format(ImageFormat::Avif)
        .quality(80)
        .max_width(1920)
        .build();

    let result = client.optimize(&image_bytes, Some(options)).await?;

    println!("Optimized CDN URL: {}", result.url);
    println!("Saved: {:.2}% ({} bytes)", result.savings_percentage, result.bytes_saved);

    Ok(())
}
```

### 2. Synchronous / Blocking Example

Do not create or call a `BlockingClient` inside a tokio runtime (an axum or tonic handler, a `#[tokio::main]` function): `reqwest::blocking` panics there. Use `Client` from async code, or run the blocking call with `tokio::task::spawn_blocking`.

```rust
use smallpict::blocking::BlockingClient;
use smallpict::models::{ImageFormat, OptimizeOptions};
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = BlockingClient::builder()
        .api_key(std::env::var("SMALLPICT_API_KEY")?)
        .secret_key(std::env::var("SMALLPICT_SECRET_KEY")?)
        .build()?;
    let image_bytes = fs::read("hero-banner.png")?;

    let result = client.optimize(&image_bytes, None)?;
    println!("CDN URL: {}", result.url);

    Ok(())
}
```

### What `optimize()` does

`POST /v1/optimize` only queues a job. `optimize()` then uploads your bytes to the pre-signed `upload_url` the API returns (a plain PUT with `Content-Type` and `Content-Length`, no API headers), polls the job status every `poll_interval` (default 1 s) and returns once the job is `completed`, so `result.status` is always `"completed"` and `url`, `format`, `bytes_saved`, `mode_applied`, `quality_applied` and `folder` come from the finished job.

- A failed job returns `SmallPictError::Optimize` with `code()` `Some("job_failed")`, the server's message and `details()["job_id"]`. A refused upload returns the same variant with code `upload_failed` and the storage's HTTP status.
- A job still running after `wait_timeout` (default 120 s) returns `SmallPictError::Timeout` with the job id in the message; it keeps running on the server, so call `get_job_status(job_id)` later.
- Pass the image bytes. The API does not fetch URLs: `optimize_file("https://...")` returns `SmallPictError::Validation` without a request; download the image yourself first.

```rust
use std::time::Duration;

let client = Client::builder()
    .api_key(std::env::var("SMALLPICT_API_KEY")?)
    .poll_interval(Duration::from_millis(500))
    .wait_timeout(Duration::from_secs(60))
    .build()?;

match client.optimize(&image_bytes, None).await {
    Ok(result) => println!("{} ({} bytes saved)", result.url, result.bytes_saved),
    Err(e) if e.code() == Some("job_failed") => println!("Job failed: {e}"),
    Err(SmallPictError::Timeout { message }) => println!("Still running: {message}"),
    Err(e) => return Err(e.into()),
}
```

### 3. Compression Mode

`mode` picks the compression: `Lossy` (server default), `Extreme`, `Lossless` or `Auto`. `Auto` picks, per image, the lowest quality that keeps the visual score (SSIMULACRA 2) at or above 70, and ignores `quality`; it is on WP Pro and Business, Velocity, Momentum and sandbox keys (5 auto jobs per minute per sandbox key), and where it cannot search (PNG or GIF output, animation, time limit) the job is lossy and `mode_applied` says so. The server checks it against your plan and answers HTTP 403 `mode_not_in_plan` (`SmallPictError::Forbidden`, with `details.allowed_modes`) when the plan does not include it. `Extreme` gives the smallest files on every plan; leave `quality` out so the server uses its default for the output format, and a quality you set is kept or raised to the format floor. Mode comparison: https://smallpict.app/en/docs/image-settings . The old `lossless(true)` option is deprecated; use `.mode(CompressionMode::Lossless)`.

```rust
use smallpict::{CompressionMode, OptimizeOptions, SmallPictError};

let options = OptimizeOptions::builder()
    .mode(CompressionMode::Lossless)
    .build();

match client.optimize(&image_bytes, Some(options)).await {
    Ok(result) => println!("CDN URL: {}", result.url),
    Err(SmallPictError::Forbidden { details, .. }) => {
        println!("Allowed modes: {:?}", details.and_then(|d| d.get("allowed_modes").cloned()));
    }
    Err(e) => return Err(e.into()),
}

// Extreme without a quality: the server uses its default for the output format.
let options = OptimizeOptions::builder()
    .mode(CompressionMode::Extreme)
    .build();
let result = client.optimize(&image_bytes, Some(options)).await?;

// The job status reports what the server applied (absent for older jobs).
let status = client.get_job_status("job_id").await?;
println!("{:?} at quality {:?}", status.mode_applied, status.quality_applied);
```

### 4. Upload Folder

`folder` stores the optimized file under a folder of your account (1-3 segments such as `products/shoes`). The server lower-cases it and answers HTTP 400 `invalid_folder` (`SmallPictError::Validation`) for a malformed value; the SDK rejects empty segments, `..` and more than 3 segments before sending.

```rust
let options = OptimizeOptions::builder().folder("products/shoes").build();
let result = client.optimize(&image_bytes, Some(options)).await?;

// The job status reports the stored folder (absent without one).
let status = client.get_job_status(&result.job_id).await?;
println!("{:?}", status.folder);
```

---

## 📊 Account Quota & Edge CDN Purge

```rust
// 1. Check real-time quota usage
let quota = client.get_quota().await?;
println!("Plan: {}, Quota Used: {:.2}%", quota.plan, quota.quota_percentage);

// 2. Invalidate Edge CDN cache
let purge = client.purge_cdn(&["https://cdn.smallpict.app/opt/hero-banner.avif"], smallpict::PurgeType::Url).await?;
println!("{}", purge.message);

// 3. Purge every file delivered under one upload folder
client.purge_folder("products/shoes").await?;
```

Folder purges (10 per hour) and `PurgeType::All` purges (2 per hour) are limited per account. Past the limit the server answers HTTP 429 `purge_rate_limited` (`SmallPictError::RateLimit`), which the SDK does not retry automatically.

### Capacity and upgrade links

`get_quota()` returns `capacity` (read-only): storage, CDN, booster balances with the lot that expires first, the unpaid-account state, `period_end` and `manage_url` of the key's product. Members that do not apply are `None`; `capacity` is `None` when the server could not read it and on older servers. Capacity is bought only in the dashboard.

```rust
if let Some(cap) = client.get_quota().await?.capacity {
    if let Some(cdn) = cap.cdn {
        println!("CDN {}/{} bytes ({})", cdn.used_bytes, cdn.limit_bytes, cdn.status);
    }
    println!("Manage capacity: {}", cap.manage_url);
}

match client.optimize(&image_bytes, None).await {
    Ok(result) => println!("{}", result.url),
    Err(e) if e.is_forbidden() => println!("Allowed modes: {:?}", e.allowed_modes()),
    Err(e) => match e.upgrade_url() {
        Some(url) => println!("{e}. Add capacity: {url}"),
        None => println!("{e}"),
    },
}
```

`upgrade_url()` reads `details.upgrade_url` of 402 errors and of the 409 capacity codes (`SmallPictError::Server` with status 409; `e.code()` gives the code). The SDK never opens, fetches or builds `manage_url` or `upgrade_url`; show them to the user.

---

## 🧪 Testing

```bash
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

---

## 🔒 Security & Schema Compatibility (2026)

- **`mime_type` & `content_type` Dual Support**: `OptimizeOptions` and `OptimizeOptionsBuilder` support both `mime_type` and `content_type` fields. Requests submitted via the async `Client` and the `BlockingClient` send both `mime_type` and `content_type`, guaranteeing 100% backward and forward compatibility with all SmallPict API versions.

---

## 📄 License

MIT © [SmallPict Engineering](https://smallpict.app)
