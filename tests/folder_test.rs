//! FLD-03 upload folder: `options.folder` on optimize, `folder` in job status,
//! folder purges, client-side folder checks and the no-retry rule for `purge_rate_limited`.

use serde_json::Value;
use smallpict::{Client, OptimizeOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

/// Answers every request with `status` and `body` (an optimize POST is queued first, see
/// below); sends each request body (JSON) back
/// and counts the requests served.
fn mock_server(
    status: u16,
    body: &'static str,
) -> (String, mpsc::Receiver<Value>, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let base = url.clone();
    let (tx, rx) = mpsc::channel();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            let method = first
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string();
            let is_optimize = first.contains(" /v1/optimize ");
            let mut len = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut buf = vec![0; len];
            reader.read_exact(&mut buf).unwrap();
            counter.fetch_add(1, Ordering::SeqCst);
            let _ = tx.send(serde_json::from_slice(&buf).unwrap_or(Value::Null));
            // optimize() flow: a 200 POST queues the job, the PUT upload succeeds, the poll gets `body`.
            let (status, reply) = match (method.as_str(), status) {
                ("POST", 200) if is_optimize => (202, QUEUED.replace("{URL}", &base)),
                ("PUT", _) => (200, String::new()),
                _ => (status, body.to_string()),
            };
            let mut stream = stream;
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            )
            .unwrap();
        }
    });
    (url, rx, hits)
}

fn client(url: &str, retries: u32) -> Client {
    Client::builder()
        .api_key("sp_sdk_test_1234567890")
        .base_url(url)
        .max_retries(retries)
        .poll_interval(Duration::from_millis(1))
        .build()
        .unwrap()
}

const QUEUED: &str = r#"{"job_id":"j1","status":"queued","upload_url":"{URL}/staging/j1"}"#;
const OK: &str = r#"{"job_id":"j1","status":"completed","url":"https://cdn/x"}"#;
const PURGED: &str = r#"{"message":"Purge job enqueued successfully"}"#;

#[tokio::test]
async fn optimize_sends_folder_as_option() {
    let (url, rx, _) = mock_server(200, OK);
    let opts = OptimizeOptions::builder().folder("Products/Shoes").build();
    client(&url, 0).optimize(b"img", Some(opts)).await.unwrap();
    assert_eq!(rx.recv().unwrap()["options"]["folder"], "Products/Shoes");
}

#[tokio::test]
async fn optimize_omits_folder_when_unset() {
    let (url, rx, _) = mock_server(200, OK);
    client(&url, 0).optimize(b"img", None).await.unwrap();
    assert!(rx.recv().unwrap()["options"].get("folder").is_none());
}

#[tokio::test]
async fn optimize_result_surfaces_folder_and_parses_without_it() {
    let (url, _rx, _) = mock_server(
        200,
        r#"{"job_id":"j1","status":"completed","url":"https://cdn/x","folder":"products/shoes"}"#,
    );
    let opts = OptimizeOptions::builder().folder("Products/Shoes").build();
    let res = client(&url, 0).optimize(b"img", Some(opts)).await.unwrap();
    assert_eq!(res.folder.as_deref(), Some("products/shoes"));

    let (url, _rx, _) = mock_server(200, OK);
    let res = client(&url, 0).optimize(b"img", None).await.unwrap();
    assert_eq!(res.folder, None);
}

#[tokio::test]
async fn job_status_surfaces_folder_and_parses_without_it() {
    let (url, _rx, _) = mock_server(
        200,
        r#"{"job_id":"j1","status":"completed","folder":"products/shoes"}"#,
    );
    let st = client(&url, 0).get_job_status("j1").await.unwrap();
    assert_eq!(st.folder.as_deref(), Some("products/shoes"));

    let (url, _rx, _) = mock_server(200, r#"{"job_id":"j0","status":"completed"}"#);
    let st = client(&url, 0).get_job_status("j0").await.unwrap();
    assert_eq!(st.folder, None);
}

#[tokio::test]
async fn purge_folder_sends_type_and_folder_only() {
    let (url, rx, _) = mock_server(202, PURGED);
    let res = client(&url, 0)
        .purge_folder("products/shoes")
        .await
        .unwrap();
    assert_eq!(res.message, "Purge job enqueued successfully");
    assert_eq!(
        rx.recv().unwrap(),
        serde_json::json!({"purge_type": "folder", "folder": "products/shoes"})
    );
}

#[tokio::test]
async fn purge_cdn_url_body_is_unchanged() {
    let (url, rx, _) = mock_server(202, PURGED);
    client(&url, 0)
        .purge_cdn(
            &["https://cdn.smallpict.app/c/j.avif"],
            smallpict::PurgeType::Url,
        )
        .await
        .unwrap();
    assert_eq!(
        rx.recv().unwrap(),
        serde_json::json!({"purge_type": "url", "urls": ["https://cdn.smallpict.app/c/j.avif"]})
    );
}

#[tokio::test]
async fn bad_folders_are_rejected_without_request() {
    for bad in ["", "a//b", "/a", "a/", "../x", "a/../b", "a/b/c/d"] {
        let (url, rx, _) = mock_server(200, OK);
        let c = client(&url, 0);

        let err = c.purge_folder(bad).await.unwrap_err();
        assert!(err.is_validation_error(), "purge {bad:?}: {err}");

        let opts = OptimizeOptions::builder().folder(bad).build();
        let err = c.optimize(b"img", Some(opts)).await.unwrap_err();
        assert!(err.is_validation_error(), "optimize {bad:?}: {err}");

        assert!(rx.try_recv().is_err(), "{bad:?}: no HTTP request expected");
    }
}

#[tokio::test]
async fn server_invalid_folder_is_validation_error_with_server_message() {
    let (url, _rx, _) = mock_server(
        400,
        r#"{"error":{"code":"invalid_folder","message":"folder segment has invalid characters"}}"#,
    );
    let opts = OptimizeOptions::builder().folder("a b").build();
    let err = client(&url, 0)
        .optimize(b"img", Some(opts))
        .await
        .unwrap_err();
    match err {
        smallpict::SmallPictError::Validation { message, .. } => {
            assert_eq!(message, "folder segment has invalid characters")
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn purge_rate_limited_is_not_retried() {
    let (url, _rx, hits) = mock_server(
        429,
        r#"{"error":{"code":"purge_rate_limited","message":"Folder purges are limited to 10 per hour"}}"#,
    );
    let err = client(&url, 3).purge_folder("products").await.unwrap_err();
    assert!(err.is_rate_limit(), "{err}");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn other_429_is_still_retried() {
    let (url, _rx, hits) = mock_server(
        429,
        r#"{"error":{"code":"RATE_LIMIT_EXCEEDED","message":"slow down"}}"#,
    );
    let err = client(&url, 1).purge_folder("products").await.unwrap_err();
    assert!(err.is_rate_limit(), "{err}");
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[cfg(feature = "blocking")]
#[test]
fn blocking_client_folder_purge_and_no_retry() {
    let c = |url: String| {
        smallpict::BlockingClient::builder()
            .api_key("sp_sdk_test_1234567890")
            .base_url(url)
            .max_retries(3)
            .poll_interval(Duration::from_millis(1))
            .build()
            .unwrap()
    };

    let (url, rx, _) = mock_server(202, PURGED);
    c(url).purge_folder("a/b").unwrap();
    assert_eq!(
        rx.recv().unwrap(),
        serde_json::json!({"purge_type": "folder", "folder": "a/b"})
    );

    let (url, rx, _) = mock_server(200, OK);
    let opts = OptimizeOptions::builder().folder("a").build();
    c(url.clone()).optimize(b"img", Some(opts)).unwrap();
    assert_eq!(rx.recv().unwrap()["options"]["folder"], "a");
    assert!(c(url)
        .purge_folder("a//b")
        .unwrap_err()
        .is_validation_error());

    let (url, _rx, hits) = mock_server(
        429,
        r#"{"error":{"code":"purge_rate_limited","message":"limited"}}"#,
    );
    assert!(c(url).purge_folder("a").unwrap_err().is_rate_limit());
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}
