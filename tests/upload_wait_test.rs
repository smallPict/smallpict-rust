//! SDK_SPEC 3d upload and wait: optimize() queues the job, PUTs the bytes to `upload_url`
//! and polls the job status until it is completed or failed.

use smallpict::{Client, OptimizeOptions, SmallPictError};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

struct Req {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Req {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Answers the requests in order with `replies` (status, body; `{URL}` becomes the server URL),
/// repeating the last one once they run out, and sends every request back.
fn server(replies: Vec<(u16, &'static str)>) -> (String, mpsc::Receiver<Req>) {
    server_with_headers(replies.into_iter().map(|(s, r)| (s, r, "")).collect())
}

/// Like `server`, with extra raw header lines (each ending in `\r\n`) per reply.
fn server_with_headers(
    replies: Vec<(u16, &'static str, &'static str)>,
) -> (String, mpsc::Receiver<Req>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let base = url.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for (i, stream) in listener.incoming().enumerate() {
            let stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            let mut parts = first.split_whitespace();
            let method = parts.next().unwrap_or_default().to_string();
            let path = parts.next().unwrap_or_default().to_string();
            let mut headers = Vec::new();
            let mut len = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                let (k, v) = line.split_once(':').unwrap();
                let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_string());
                if k == "content-length" {
                    len = v.parse().unwrap();
                }
                headers.push((k, v));
            }
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let _ = tx.send(Req {
                method,
                path,
                headers,
                body,
            });
            let (status, reply, extra) = replies[i.min(replies.len() - 1)];
            let reply = reply.replace("{URL}", &base);
            let extra = extra.replace("{URL}", &base);
            let mut stream = stream;
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{reply}",
                reply.len()
            )
            .unwrap();
        }
    });
    (url, rx)
}

fn next(rx: &mpsc::Receiver<Req>) -> Req {
    rx.recv_timeout(Duration::from_secs(5))
        .expect("request not sent")
}

fn client(url: &str) -> Client {
    Client::builder()
        .api_key("sp_sdk_test_1234567890")
        .secret_key("sec_test_abcdefgh")
        .base_url(url)
        .max_retries(0)
        .poll_interval(Duration::from_millis(1))
        .build()
        .unwrap()
}

const QUEUED: &str = r#"{"job_id":"j1","status":"queued","upload_url":"{URL}/staging/j1?sig=abc","object_key":"k","bucket":"b"}"#;
const PROCESSING: &str = r#"{"job_id":"j1","status":"processing"}"#;
const COMPLETED: &str = r#"{"job_id":"j1","status":"completed","url":"https://cdn/x.avif","format":"avif","bytes_saved":3,"mode_applied":"lossy","quality_applied":80,"folder":"shoes"}"#;
const FAILED: &str = r#"{"job_id":"j1","status":"failed","error":{"code":"job_failed","message":"client upload never arrived"}}"#;

fn png_opts() -> Option<OptimizeOptions> {
    Some(OptimizeOptions::builder().mime_type("image/png").build())
}

fn assert_put(put: &Req, bytes: &[u8]) {
    assert_eq!(put.method, "PUT");
    assert_eq!(put.path, "/staging/j1?sig=abc");
    assert_eq!(put.header("content-type"), Some("image/png"));
    assert_eq!(put.header("content-length"), Some("5"));
    for h in [
        "x-api-key",
        "x-timestamp",
        "x-signature",
        "authorization",
        "idempotency-key",
    ] {
        assert!(put.header(h).is_none(), "PUT carries {h}");
    }
    assert_eq!(put.body, bytes);
}

#[tokio::test]
async fn optimize_uploads_then_polls_until_completed() {
    let (url, rx) = server(vec![
        (202, QUEUED),
        (200, ""),
        (200, PROCESSING),
        (200, COMPLETED),
    ]);
    let res = client(&url).optimize(b"12345", png_opts()).await.unwrap();

    let post = next(&rx);
    assert_eq!(
        (post.method.as_str(), post.path.as_str()),
        ("POST", "/v1/optimize")
    );
    let sent: serde_json::Value = serde_json::from_slice(&post.body).unwrap();
    assert_eq!(sent["filesize"], 5);
    assert_eq!(sent["mime_type"], "image/png");
    assert_put(&next(&rx), b"12345");
    for _ in 0..2 {
        let poll = next(&rx);
        assert_eq!(poll.method, "GET");
        assert_eq!(poll.path, "/v1/optimize/status?job_id=j1");
        assert!(poll.header("x-signature").is_some());
    }

    assert_eq!(res.job_id, "j1");
    assert_eq!(res.status, "completed");
    assert_eq!(res.url, "https://cdn/x.avif");
    assert_eq!(res.format, "avif");
    assert_eq!(res.bytes_saved, 3);
    assert_eq!(res.original_size, 5);
    assert_eq!(res.compressed_size, 2);
    assert_eq!(res.savings_percentage, 60.0);
    assert_eq!(res.mode_applied.as_deref(), Some("lossy"));
    assert_eq!(res.quality_applied, Some(80));
    assert_eq!(res.folder.as_deref(), Some("shoes"));
}

#[tokio::test]
async fn failed_job_is_job_failed_with_server_message() {
    let (url, _rx) = server(vec![(202, QUEUED), (200, ""), (200, FAILED)]);
    let err = client(&url)
        .optimize(b"12345", png_opts())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("job_failed"));
    assert!(
        err.to_string().contains("client upload never arrived"),
        "{err}"
    );
    assert_eq!(err.details().unwrap()["job_id"], "j1");
}

#[tokio::test]
async fn job_still_running_after_wait_timeout_is_timeout_naming_the_job() {
    let (url, _rx) = server(vec![(202, QUEUED), (200, ""), (200, PROCESSING)]);
    let c = Client::builder()
        .api_key("sp_sdk_test_1234567890")
        .secret_key("sec_test_abcdefgh")
        .base_url(&url)
        .max_retries(0)
        .poll_interval(Duration::from_millis(1))
        .wait_timeout(Duration::from_millis(20))
        .build()
        .unwrap();
    let err = c.optimize(b"12345", png_opts()).await.unwrap_err();
    match err {
        SmallPictError::Timeout { ref message } => {
            assert!(message.contains("j1"), "{message}")
        }
        other => panic!("expected Timeout, got {other}"),
    }
}

#[tokio::test]
async fn refused_upload_is_upload_failed_with_status() {
    let (url, rx) = server(vec![(202, QUEUED), (403, "SignatureDoesNotMatch")]);
    let err = client(&url)
        .optimize(b"12345", png_opts())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("upload_failed"));
    assert!(matches!(
        err,
        SmallPictError::Optimize {
            status: Some(403),
            ..
        }
    ));
    assert!(err.to_string().contains("SignatureDoesNotMatch"), "{err}");
    next(&rx);
    next(&rx);
    assert!(
        rx.recv_timeout(Duration::from_millis(50)).is_err(),
        "no poll after a refused upload"
    );
}

#[tokio::test]
async fn url_source_is_validation_error_without_request() {
    let (url, rx) = server(vec![(202, QUEUED)]);
    for src in ["https://example.com/a.png", "http://example.com/a.png"] {
        let err = client(&url).optimize_file(src, None).await.unwrap_err();
        assert!(err.is_validation_error(), "{src}: {err}");
        assert!(err.to_string().contains("does not fetch URLs"), "{err}");
    }
    assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
}

#[cfg(feature = "blocking")]
#[test]
fn blocking_client_uploads_then_polls() {
    let (url, rx) = server(vec![
        (202, QUEUED),
        (200, ""),
        (200, PROCESSING),
        (200, COMPLETED),
    ]);
    let c = smallpict::BlockingClient::builder()
        .api_key("sp_sdk_test_1234567890")
        .secret_key("sec_test_abcdefgh")
        .base_url(url)
        .max_retries(0)
        .poll_interval(Duration::from_millis(1))
        .build()
        .unwrap();
    let res = c.optimize(b"12345", png_opts()).unwrap();
    next(&rx);
    assert_put(&next(&rx), b"12345");
    assert_eq!(res.url, "https://cdn/x.avif");
    assert_eq!(res.compressed_size, 2);

    let (url, _rx) = server(vec![(202, QUEUED), (200, ""), (200, FAILED)]);
    let c = smallpict::BlockingClient::builder()
        .api_key("sp_sdk_test_1234567890")
        .secret_key("sec_test_abcdefgh")
        .base_url(url)
        .max_retries(0)
        .poll_interval(Duration::from_millis(1))
        .build()
        .unwrap();
    assert_eq!(
        c.optimize(b"12345", png_opts()).unwrap_err().code(),
        Some("job_failed")
    );
    assert!(c
        .optimize_file("https://example.com/a.png", None)
        .unwrap_err()
        .is_validation_error());
}

const SERVER_ERROR: &str = r#"{"error":{"code":"INTERNAL_ERROR","message":"try again"}}"#;
const PURGED: &str = r#"{"message":"Purge accepted"}"#;

#[tokio::test]
async fn retry_sends_new_nonce_and_same_idempotency_key() {
    let (url, rx) = server(vec![(503, SERVER_ERROR), (202, PURGED)]);
    let c = Client::builder()
        .api_key("sp_sdk_test_1234567890")
        .secret_key("sec_test_abcdefgh")
        .base_url(url)
        .max_retries(1)
        .build()
        .unwrap();
    c.purge_cdn(&["https://cdn/x.avif"], smallpict::PurgeType::Url)
        .await
        .unwrap();
    let first = next(&rx);
    let second = next(&rx);
    assert!(
        first.header("x-nonce").is_some(),
        "no X-Nonce on the first attempt"
    );
    assert_ne!(first.header("x-nonce"), second.header("x-nonce"));
    assert_eq!(
        first.header("idempotency-key"),
        second.header("idempotency-key")
    );
}

#[cfg(feature = "blocking")]
#[test]
fn blocking_retry_sends_new_nonce_and_same_idempotency_key() {
    let (url, rx) = server(vec![(503, SERVER_ERROR), (202, PURGED)]);
    let c = smallpict::BlockingClient::builder()
        .api_key("sp_sdk_test_1234567890")
        .secret_key("sec_test_abcdefgh")
        .base_url(url)
        .max_retries(1)
        .build()
        .unwrap();
    c.purge_cdn(&["https://cdn/x.avif"], smallpict::PurgeType::Url)
        .unwrap();
    let first = next(&rx);
    let second = next(&rx);
    assert!(
        first.header("x-nonce").is_some(),
        "no X-Nonce on the first attempt"
    );
    assert_ne!(first.header("x-nonce"), second.header("x-nonce"));
    assert_eq!(
        first.header("idempotency-key"),
        second.header("idempotency-key")
    );
}

const QUEUED_DEAD_UPLOAD: &str = r#"{"job_id":"j1","status":"queued","upload_url":"http://127.0.0.1:9/staging/j1?X-Amz-Signature=abc","object_key":"k","bucket":"b"}"#;

#[tokio::test]
async fn upload_transport_error_does_not_leak_the_presigned_url() {
    let (url, _rx) = server(vec![(202, QUEUED_DEAD_UPLOAD)]);
    let err = client(&url)
        .optimize(b"12345", png_opts())
        .await
        .unwrap_err();
    let text = err.to_string();
    assert!(matches!(err, SmallPictError::Network { .. }), "{text}");
    assert!(!text.contains("X-Amz-Signature"), "{text}");
    assert!(!text.contains("127.0.0.1:9"), "{text}");
}

#[tokio::test]
async fn upload_failed_message_is_truncated() {
    let big: &'static str = Box::leak("x".repeat(5000).into_boxed_str());
    let (url, _rx) = server(vec![(202, QUEUED), (403, big)]);
    let err = client(&url)
        .optimize(b"12345", png_opts())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("upload_failed"));
    assert!(err.to_string().len() < 400, "{}", err.to_string().len());
}

#[tokio::test]
async fn retry_after_is_capped_and_large_max_retries_does_not_panic() {
    let (url, rx) = server(vec![(503, SERVER_ERROR)]);
    let c = Client::builder()
        .api_key("sp_sdk_test_1234567890")
        .secret_key("sec_test_abcdefgh")
        .base_url(url)
        .max_retries(40)
        .max_retry_delay(Duration::from_millis(5))
        .build()
        .unwrap();
    let started = std::time::Instant::now();
    let err = c
        .purge_cdn(&["https://cdn/x.avif"], smallpict::PurgeType::Url)
        .await
        .unwrap_err();
    assert!(
        matches!(err, SmallPictError::Server { status: 503, .. }),
        "{err}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    let mut count = 0;
    while rx.try_recv().is_ok() {
        count += 1;
    }
    assert_eq!(count, 41);
}

#[tokio::test]
async fn retry_after_header_is_capped() {
    let (url, _rx) = server_with_headers(vec![
        (503, SERVER_ERROR, "Retry-After: 3600\r\n"),
        (202, PURGED, ""),
    ]);
    let c = Client::builder()
        .api_key("sp_sdk_test_1234567890")
        .secret_key("sec_test_abcdefgh")
        .base_url(url)
        .max_retries(1)
        .max_retry_delay(Duration::from_millis(5))
        .build()
        .unwrap();
    let started = std::time::Instant::now();
    c.purge_cdn(&["https://cdn/x.avif"], smallpict::PurgeType::Url)
        .await
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn bad_idempotency_key_is_a_validation_error_not_a_panic() {
    let (url, _rx) = server(vec![(202, QUEUED)]);
    let opts = OptimizeOptions::builder()
        .mime_type("image/png")
        .idempotency_key("order\n42")
        .build();
    let err = client(&url)
        .optimize(b"12345", Some(opts))
        .await
        .unwrap_err();
    assert!(err.is_validation_error(), "{err}");
}

#[tokio::test]
async fn redirects_are_not_followed_with_signed_headers() {
    let (url, rx) = server_with_headers(vec![
        (302, "", "Location: {URL}/elsewhere\r\n"),
        (
            200,
            r#"{"plan":"free","bytes_used":0,"quota_limit":1,"quota_percentage":0.0}"#,
            "",
        ),
    ]);
    let err = client(&url).get_quota().await.unwrap_err();
    assert!(
        matches!(err, SmallPictError::Server { status: 302, .. }),
        "{err}"
    );
    next(&rx);
    assert!(rx.try_recv().is_err(), "the redirect was followed");
}

#[tokio::test]
async fn request_signature_matches_the_recomputed_hmac() {
    let (url, rx) = server(vec![(
        200,
        r#"{"plan":"free","bytes_used":0,"quota_limit":1,"quota_percentage":0.0}"#,
    )]);
    client(&url).get_quota().await.unwrap();
    let req = next(&rx);
    let ts = req.header("x-timestamp").expect("x-timestamp");
    let expected = smallpict::crypto::hmac_sha256_hex(
        "sec_test_abcdefgh",
        &smallpict::crypto::build_string_to_sign(
            "GET",
            &req.path,
            ts,
            smallpict::crypto::EMPTY_SHA256,
        ),
    );
    assert_eq!(req.header("x-signature"), Some(expected.as_str()));
    assert_eq!(req.header("x-api-key"), Some("sp_sdk_test_1234567890"));
}

#[tokio::test]
async fn statuses_401_404_500_map_to_their_variants() {
    for (status, check) in [
        (
            401u16,
            (|e: &SmallPictError| matches!(e, SmallPictError::Unauthorized { .. }))
                as fn(&SmallPictError) -> bool,
        ),
        (404, |e| matches!(e, SmallPictError::NotFound { .. })),
        (500, |e| {
            matches!(e, SmallPictError::Server { status: 500, .. })
        }),
    ] {
        let (url, _rx) = server(vec![(status, r#"{"error":{"code":"X","message":"m"}}"#)]);
        let err = client(&url).get_quota().await.unwrap_err();
        assert!(check(&err), "status {status}: {err}");
    }
}

#[tokio::test]
async fn server_error_is_retried_and_retry_after_is_honoured() {
    let (url, rx) = server_with_headers(vec![
        (503, SERVER_ERROR, "Retry-After: 1\r\n"),
        (
            200,
            r#"{"plan":"free","bytes_used":0,"quota_limit":1,"quota_percentage":0.0}"#,
            "",
        ),
    ]);
    let c = Client::builder()
        .api_key("sp_sdk_test_1234567890")
        .secret_key("sec_test_abcdefgh")
        .base_url(url)
        .max_retries(1)
        .build()
        .unwrap();
    let started = std::time::Instant::now();
    c.get_quota().await.unwrap();
    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    next(&rx);
    next(&rx);
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn optimize_file_sends_the_file_name() {
    let dir = std::env::temp_dir().join(format!("smallpict-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("logo.png");
    std::fs::write(&path, b"12345").unwrap();
    let (url, rx) = server(vec![(202, QUEUED), (200, ""), (200, COMPLETED)]);
    client(&url)
        .optimize_file(path.to_str().unwrap(), png_opts())
        .await
        .unwrap();
    let post = next(&rx);
    let body: serde_json::Value = serde_json::from_slice(&post.body).unwrap();
    assert_eq!(body["filename"], "logo.png");
    assert_eq!(body["filesize"], 5);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn status_poll_signs_the_path_without_the_query() {
    let (url, rx) = server(vec![(202, QUEUED), (200, ""), (200, COMPLETED)]);
    client(&url).optimize(b"12345", png_opts()).await.unwrap();
    next(&rx);
    next(&rx);
    let poll = next(&rx);
    assert_eq!(poll.path, "/v1/optimize/status?job_id=j1");
    let ts = poll.header("x-timestamp").unwrap();
    let expected = smallpict::crypto::hmac_sha256_hex(
        "sec_test_abcdefgh",
        &smallpict::crypto::build_string_to_sign(
            "GET",
            "/v1/optimize/status",
            ts,
            smallpict::crypto::EMPTY_SHA256,
        ),
    );
    assert_eq!(poll.header("x-signature"), Some(expected.as_str()));
}

async fn optimize_mime(bytes: &[u8], opts: Option<OptimizeOptions>) -> (String, String) {
    let (url, rx) = server(vec![(202, QUEUED), (200, ""), (200, COMPLETED)]);
    client(&url).optimize(bytes, opts).await.unwrap();
    let post = next(&rx);
    let put = next(&rx);
    let body: serde_json::Value = serde_json::from_slice(&post.body).unwrap();
    (
        body["mime_type"].as_str().unwrap().to_string(),
        put.header("content-type").unwrap().to_string(),
    )
}

#[tokio::test]
async fn mime_type_is_detected_from_the_bytes() {
    assert_eq!(
        optimize_mime(b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR", None).await,
        ("image/png".to_string(), "image/png".to_string())
    );
    assert_eq!(optimize_mime(b"GIF89a\x00\x00", None).await.0, "image/gif");
    assert_eq!(
        optimize_mime(b"RIFF\x00\x00\x00\x00WEBPVP8 ", None).await.0,
        "image/webp"
    );
    assert_eq!(
        optimize_mime(b"\x00\x00\x00\x1cftypavifmif1", None).await.0,
        "image/avif"
    );
    assert_eq!(optimize_mime(b"not an image", None).await.0, "image/jpeg");
    let explicit = OptimizeOptions::builder().mime_type("image/webp").build();
    assert_eq!(
        optimize_mime(b"\x89PNG\r\n\x1a\n", Some(explicit)).await.0,
        "image/webp"
    );
}

#[tokio::test]
async fn non_5xx_statuses_are_not_labelled_internal_error() {
    let (url, _rx) = server(vec![(
        409,
        r#"{"error":{"code":"booster_period_limit_exceeded","message":"limit"}}"#,
    )]);
    let err = client(&url).get_quota().await.unwrap_err();
    assert!(
        matches!(err, SmallPictError::Server { status: 409, .. }),
        "{err}"
    );
    assert!(
        err.to_string()
            .starts_with("[booster_period_limit_exceeded]"),
        "{err}"
    );
    assert!(!err.is_server_error(), "{err}");
}
