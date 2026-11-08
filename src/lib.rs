//! # SmallPict Rust SDK
//!
//! Official Rust client library for the [SmallPict Image Optimization API](https://smallpict.app) —
//! next-gen image transcoding (AVIF, WebP), smart compression, Edge CDN delivery, and cache purging.
//!
//! ## Quick Start (Async)
//!
//! ```no_run
//! use smallpict::{Client, ImageFormat, OptimizeOptions};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let client = Client::builder()
//!         .api_key(std::env::var("SMALLPICT_API_KEY")?)
//!         .secret_key(std::env::var("SMALLPICT_SECRET_KEY")?)
//!         .build()?;
//!     let image_data = std::fs::read("hero.png")?;
//!
//!     let options = OptimizeOptions::builder()
//!         .format(ImageFormat::Avif)
//!         .quality(80)
//!         .max_width(1920)
//!         .build();
//!
//!     let result = client.optimize(&image_data, Some(options)).await?;
//!     println!("CDN URL: {}", result.url);
//!     println!("Saved: {:.2}% ({} bytes)", result.savings_percentage, result.bytes_saved);
//!     Ok(())
//! }
//! ```
//!
//! ## Quick Start (Blocking)
//!
//! Enable the `blocking` feature in `Cargo.toml`:
//! ```toml
//! [dependencies]
//! smallpict = { version = "1.0", features = ["blocking"] }
//! ```
//!
//! ```no_run
//! #[cfg(feature = "blocking")]
//! {
//!     use smallpict::blocking::BlockingClient;
//!     use smallpict::models::{ImageFormat, OptimizeOptions};
//!
//!     let client = BlockingClient::builder()
//!         .api_key(std::env::var("SMALLPICT_API_KEY")?)
//!         .secret_key(std::env::var("SMALLPICT_SECRET_KEY")?)
//!         .build()?;
//!     let image_data = std::fs::read("hero.png")?;
//!     let result = client.optimize(&image_data, None)?;
//!     println!("CDN URL: {}", result.url);
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod crypto;
pub mod errors;
pub(crate) mod mime;
pub mod models;
pub(crate) mod request;

#[cfg(feature = "async")]
pub mod client;

#[cfg(feature = "blocking")]
pub mod blocking;

// Re-exports for convenient top-level usage
pub use errors::{sanitize_message, SecretRedacted, SmallPictError};
pub use models::{
    BoosterExpiry, Capacity, CapacityBooster, CapacityCdn, CapacityLapse, CapacityStorage,
    CompressionMode, FallbackMode, FitMode, ImageFormat, JobStatusResult, OptimizeOptions,
    OptimizeOptionsBuilder, OptimizeResult, PurgeResponse, PurgeType, QuotaResponse,
};

#[cfg(feature = "async")]
pub use client::{Client, ClientBuilder};

#[cfg(feature = "blocking")]
pub use blocking::{BlockingClient, BlockingClientBuilder};
