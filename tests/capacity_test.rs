//! SDK-02 capacity and upgrade links: quota responses and error cases copied from
//! smallpict-sdk-shared/capacity.json (not available in CI).

use serde_json::{json, Value};
use smallpict::{Client, QuotaResponse, SmallPictError};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

const FIXTURE: &str = r#"{
  "quota_responses": {
    "wordpress_with_capacity": {
      "plan": "pro",
      "bytes_used": 1048576,
      "capacity": {
        "period_end": "2026-11-30T00:00:00Z",
        "storage": { "limit_bytes": 75161927680, "used_bytes": 7516192768, "base_bytes": 53687091200, "addon_blocks": 2 },
        "cdn": { "limit_bytes": 53687091200, "used_bytes": 44023414784, "status": "warn_80" },
        "booster": {
          "active": true,
          "wp_quota_bytes": 2147483648,
          "api_transforms": null,
          "cdn_bytes": 107374182400,
          "next_expiry": { "kind": "cdn_bytes", "units": 42949672960, "expires_at": "2027-01-05T00:00:00Z" }
        },
        "lapse": null,
        "manage_url": "https://smallpict.app/dashboard/billing"
      }
    },
    "api_blocked": {
      "plan": "velocity",
      "bytes_used": 0,
      "capacity": {
        "period_end": null,
        "storage": { "limit_bytes": 107374182400, "used_bytes": 1073741824, "base_bytes": 107374182400, "addon_blocks": 0 },
        "cdn": null,
        "booster": { "active": false, "wp_quota_bytes": null, "api_transforms": 5000, "cdn_bytes": null, "next_expiry": null },
        "lapse": { "stage": "blocked", "delete_after": "2027-01-28T00:00:00Z" },
        "manage_url": "https://smallpict.app/dashboard/billing"
      }
    },
    "sandbox": {
      "plan": "sandbox",
      "bytes_used": 0,
      "capacity": {
        "period_end": null,
        "storage": null,
        "cdn": { "limit_bytes": 1073741824, "used_bytes": 0, "status": "ok" },
        "booster": { "active": false, "wp_quota_bytes": null, "api_transforms": 0, "cdn_bytes": 0, "next_expiry": null },
        "lapse": null,
        "manage_url": "https://smallpict.app/dashboard/billing"
      }
    },
    "capacity_unreadable": { "plan": "pro", "bytes_used": 0, "capacity": null },
    "older_server_without_capacity": { "plan": "pro", "bytes_used": 0 }
  },
  "error_cases": [
    { "http_status": 402, "code": "QUOTA_EXCEEDED", "sdk_error": "QuotaExceededError", "details": { "upgrade_url": "https://smallpict.app/dashboard/billing" }, "upgrade_url": "https://smallpict.app/dashboard/billing" },
    { "http_status": 402, "code": "storage_quota_exceeded", "sdk_error": "QuotaExceededError", "details": { "limit_bytes": 10, "used_bytes": 10, "remaining_bytes": 0, "upgrade_url": "https://smallpict.app/dashboard/billing" }, "upgrade_url": "https://smallpict.app/dashboard/billing" },
    { "http_status": 402, "code": "account_read_only", "sdk_error": "QuotaExceededError", "details": { "delete_after": "2027-01-28T00:00:00Z", "upgrade_url": "https://smallpict.app/dashboard/billing" }, "upgrade_url": "https://smallpict.app/dashboard/billing" },
    { "http_status": 402, "code": "QUOTA_EXCEEDED", "sdk_error": "QuotaExceededError", "details": { "payload_bytes": 60000000 }, "upgrade_url": null },
    { "http_status": 403, "code": "mode_not_in_plan", "sdk_error": "PermissionDeniedError", "details": { "mode": "extreme", "allowed_modes": ["lossy", "lossless"] }, "allowed_modes": ["lossy", "lossless"], "upgrade_url": null },
    { "http_status": 409, "code": "booster_period_limit_exceeded", "sdk_error": "SmallPictError", "details": { "remaining_steps": 0, "upgrade_url": "https://smallpict.app/dashboard/billing" }, "upgrade_url": "https://smallpict.app/dashboard/billing" }
  ]
}"#;

const MANAGE: &str = "https://smallpict.app/dashboard/billing";

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).unwrap()
}

/// Parses a fixture response. The fixtures omit `quota_limit` and `quota_percentage`, which
/// `QuotaResponse` has always required, so they are added here.
fn quota(name: &str) -> QuotaResponse {
    let mut v = fixture()["quota_responses"][name].clone();
    v["quota_limit"] = json!(0);
    v["quota_percentage"] = json!(0.0);
    serde_json::from_value(v).unwrap()
}

#[test]
fn wordpress_with_capacity_every_member() {
    let q = quota("wordpress_with_capacity");
    assert_eq!(q.plan, "pro");
    assert_eq!(q.bytes_used, 1048576);
    let c = q.capacity.expect("capacity");
    assert_eq!(c.period_end.as_deref(), Some("2026-11-30T00:00:00Z"));
    let s = c.storage.expect("storage");
    assert_eq!(s.limit_bytes, 75161927680);
    assert_eq!(s.used_bytes, 7516192768);
    assert_eq!(s.base_bytes, 53687091200);
    assert_eq!(s.addon_blocks, 2);
    let cdn = c.cdn.expect("cdn");
    assert_eq!(cdn.limit_bytes, 53687091200);
    assert_eq!(cdn.used_bytes, 44023414784);
    assert_eq!(cdn.status, "warn_80");
    assert!(c.booster.active);
    assert_eq!(c.booster.wp_quota_bytes, Some(2147483648));
    assert_eq!(c.booster.api_transforms, None);
    assert_eq!(c.booster.cdn_bytes, Some(107374182400));
    let e = c.booster.next_expiry.expect("next_expiry");
    assert_eq!(e.kind, "cdn_bytes");
    assert_eq!(e.units, 42949672960);
    assert_eq!(e.expires_at, "2027-01-05T00:00:00Z");
    assert!(c.lapse.is_none());
    assert_eq!(c.manage_url, MANAGE);
}

#[test]
fn api_blocked_null_members_stay_none() {
    let c = quota("api_blocked").capacity.expect("capacity");
    assert!(c.period_end.is_none());
    assert_eq!(c.storage.expect("storage").addon_blocks, 0);
    assert!(c.cdn.is_none());
    assert!(!c.booster.active);
    assert_eq!(c.booster.wp_quota_bytes, None);
    assert_eq!(c.booster.api_transforms, Some(5000));
    assert_eq!(c.booster.cdn_bytes, None);
    assert!(c.booster.next_expiry.is_none());
    let l = c.lapse.expect("lapse");
    assert_eq!(l.stage, "blocked");
    assert_eq!(l.delete_after.as_deref(), Some("2027-01-28T00:00:00Z"));
    assert_eq!(c.manage_url, MANAGE);
}

#[test]
fn sandbox_zeros() {
    let c = quota("sandbox").capacity.expect("capacity");
    assert!(c.storage.is_none());
    let cdn = c.cdn.expect("cdn");
    assert_eq!((cdn.limit_bytes, cdn.used_bytes), (1073741824, 0));
    assert_eq!(cdn.status, "ok");
    assert!(!c.booster.active);
    assert_eq!(c.booster.api_transforms, Some(0));
    assert_eq!(c.booster.cdn_bytes, Some(0));
    assert_eq!(c.booster.wp_quota_bytes, None);
    assert!(c.lapse.is_none());
}

#[test]
fn capacity_null_and_absent_parse_as_none() {
    assert!(quota("capacity_unreadable").capacity.is_none());
    assert!(quota("older_server_without_capacity").capacity.is_none());
}

#[test]
fn existing_quota_response_without_capacity_still_parses() {
    let q: QuotaResponse = serde_json::from_str(
        r#"{"plan":"api_velocity","bytes_used":1258291200,"quota_limit":10737418240,"quota_percentage":11.72,"cdn_egress_used_bytes":3450000000}"#,
    )
    .unwrap();
    assert_eq!(q.quota_limit, 10737418240);
    assert_eq!(q.cdn_egress_used_bytes, Some(3450000000));
    assert!(q.capacity.is_none());
}

/// Serves one HTTP request with `status` and `body`.
fn mock_server(status: u16, body: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
        }
        let mut stream = stream;
        write!(
            stream,
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    url
}

async fn error_for(case: &Value) -> SmallPictError {
    let body =
        json!({"error": {"code": case["code"], "message": "denied", "details": case["details"]}});
    let url = mock_server(
        case["http_status"].as_u64().unwrap() as u16,
        body.to_string(),
    );
    let client = Client::builder()
        .api_key("sp_sdk_test_1234567890")
        .secret_key("sec_test_abcdefgh")
        .base_url(&url)
        .max_retries(0)
        .build()
        .unwrap();
    client.get_quota().await.expect_err("error")
}

#[tokio::test]
async fn error_cases_from_shared_fixture() {
    for case in fixture()["error_cases"].as_array().unwrap() {
        let err = error_for(case).await;
        let code = case["code"].as_str().unwrap();
        match case["sdk_error"].as_str().unwrap() {
            "QuotaExceededError" => assert!(err.is_quota_exceeded(), "{code}: {err:?}"),
            "PermissionDeniedError" => assert!(err.is_forbidden(), "{code}: {err:?}"),
            _ => {
                assert!(
                    matches!(err, SmallPictError::Server { status: 409, .. }),
                    "{code}: {err:?}"
                );
                assert_eq!(err.code(), Some(code));
            }
        }
        assert_eq!(err.upgrade_url(), case["upgrade_url"].as_str(), "{code}");
        let modes: Option<Vec<&str>> = case["allowed_modes"]
            .as_array()
            .map(|a| a.iter().filter_map(|m| m.as_str()).collect());
        assert_eq!(err.allowed_modes(), modes, "{code}");
        assert_eq!(err.details(), Some(&case["details"]), "{code}");
    }
}

#[tokio::test]
async fn non_string_upgrade_url_is_not_exposed() {
    let case =
        json!({"http_status": 402, "code": "QUOTA_EXCEEDED", "details": {"upgrade_url": 42}});
    assert_eq!(error_for(&case).await.upgrade_url(), None);
    let case = json!({"http_status": 409, "code": "subscription_not_active", "details": {"upgrade_url": {"href": "x"}}});
    assert_eq!(error_for(&case).await.upgrade_url(), None);
}
