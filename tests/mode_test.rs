//! FND-07 compression mode: mapping cases from smallpict-sdk-shared/compression-modes.json,
//! job status fields, and 403 mode_not_in_plan details.
#![allow(deprecated)] // exercises the deprecated `lossless` flag on purpose

use serde_json::Value;
use smallpict::{Client, CompressionMode, OptimizeOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

/// Answers every request with `status` and `body` (an optimize POST is queued first, see
/// below); sends each request body (JSON) back.
fn mock_server(status: u16, body: &'static str) -> (String, mpsc::Receiver<Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let base = url.clone();
    let (tx, rx) = mpsc::channel();
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
    (url, rx)
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

const QUEUED: &str = r#"{"job_id":"j1","status":"queued","upload_url":"{URL}/staging/j1"}"#;
const OK: &str = r#"{"job_id":"j1","status":"completed","url":"https://cdn/x"}"#;

/// (name, mode, lossless, expected wire mode or None when omitted)
type Case = (
    &'static str,
    Option<CompressionMode>,
    Option<bool>,
    Option<&'static str>,
);

#[tokio::test]
async fn mapping_cases_from_shared_fixture() {
    use CompressionMode::*;
    let cases: [Case; 6] = [
        ("no mode, no flag", None, None, None),
        ("mode only", Some(Extreme), None, Some("extreme")),
        ("mode auto", Some(Auto), None, Some("auto")),
        ("deprecated flag only", None, Some(true), Some("lossless")),
        ("flag false is ignored", None, Some(false), None),
        (
            "flag agrees with mode",
            Some(Lossless),
            Some(true),
            Some("lossless"),
        ),
    ];
    for (name, mode, lossless, wire) in cases {
        let (url, rx) = mock_server(200, OK);
        let opts = OptimizeOptions {
            mode,
            lossless,
            ..Default::default()
        };
        client(&url).optimize(b"img", Some(opts)).await.unwrap();
        let sent = rx.recv().unwrap();
        let options = &sent["options"];
        assert_eq!(options.get("mode").and_then(Value::as_str), wire, "{name}");
        assert!(options.get("lossless").is_none(), "{name}: lossless sent");
    }
}

#[tokio::test]
async fn quality_omitted_when_unset_so_server_picks_default() {
    for opts in [None, Some(OptimizeOptions::default())] {
        let (url, rx) = mock_server(200, OK);
        client(&url).optimize(b"img", opts).await.unwrap();
        let sent = rx.recv().unwrap();
        assert!(
            sent["options"].get("quality").is_none(),
            "quality sent: {sent}"
        );
    }
}

#[tokio::test]
async fn quality_sent_when_set() {
    let (url, rx) = mock_server(200, OK);
    let opts = OptimizeOptions::builder()
        .mode(CompressionMode::Extreme)
        .quality(70)
        .build();
    client(&url).optimize(b"img", Some(opts)).await.unwrap();
    let sent = rx.recv().unwrap();
    assert_eq!(sent["options"]["quality"], 70);
}

#[tokio::test]
async fn flag_contradicts_mode_is_validation_error_without_request() {
    let (url, rx) = mock_server(200, OK);
    let opts = OptimizeOptions::builder()
        .mode(CompressionMode::Lossy)
        .lossless(true)
        .build();
    let err = client(&url).optimize(b"img", Some(opts)).await.unwrap_err();
    assert!(err.is_validation_error(), "{err}");
    assert!(rx.try_recv().is_err(), "no HTTP request expected");
}

#[tokio::test]
async fn mode_not_in_plan_keeps_details() {
    let (url, _rx) = mock_server(
        403,
        r#"{"error":{"code":"mode_not_in_plan","message":"Mode extreme is not in your plan","details":{"mode":"extreme","allowed_modes":["lossy"]}}}"#,
    );
    let opts = OptimizeOptions::builder()
        .mode(CompressionMode::Extreme)
        .build();
    let err = client(&url).optimize(b"img", Some(opts)).await.unwrap_err();
    assert!(err.is_forbidden());
    match err {
        smallpict::SmallPictError::Forbidden {
            message, details, ..
        } => {
            assert_eq!(message, "Mode extreme is not in your plan");
            let d = details.expect("details");
            assert_eq!(d["mode"], "extreme");
            assert_eq!(d["allowed_modes"], serde_json::json!(["lossy"]));
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn job_status_parses_mode_and_quality_applied() {
    let (url, _rx) = mock_server(
        200,
        r#"{"job_id":"j1","status":"completed","mode_applied":"extreme","quality_applied":62}"#,
    );
    let st = client(&url).get_job_status("j1").await.unwrap();
    assert_eq!(st.mode_applied.as_deref(), Some("extreme"));
    assert_eq!(st.quality_applied, Some(62));

    // Old jobs omit both fields; unknown future modes still parse.
    let (url, _rx) = mock_server(200, r#"{"job_id":"j0","status":"completed"}"#);
    let st = client(&url).get_job_status("j0").await.unwrap();
    assert_eq!((st.mode_applied, st.quality_applied), (None, None));
    let (url, _rx) = mock_server(
        200,
        r#"{"job_id":"j2","status":"completed","mode_applied":"ultra"}"#,
    );
    let st = client(&url).get_job_status("j2").await.unwrap();
    assert_eq!(st.mode_applied.as_deref(), Some("ultra"));
}

#[cfg(feature = "blocking")]
#[test]
fn blocking_client_maps_deprecated_flag() {
    let (url, rx) = mock_server(200, OK);
    let c = smallpict::BlockingClient::builder()
        .api_key("sp_sdk_test_1234567890")
        .secret_key("sec_test_abcdefgh")
        .base_url(url)
        .max_retries(0)
        .poll_interval(Duration::from_millis(1))
        .build()
        .unwrap();
    let opts = OptimizeOptions::builder().lossless(true).build();
    c.optimize(b"img", Some(opts)).unwrap();
    let sent = rx.recv().unwrap();
    assert_eq!(sent["options"]["mode"], "lossless");
    assert!(sent["options"].get("lossless").is_none());
}
