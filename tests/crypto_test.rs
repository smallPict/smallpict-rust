use smallpict::crypto::{self, EMPTY_SHA256};
use smallpict::errors::{sanitize_message, SecretRedacted};

#[test]
fn test_empty_sha256_constant() {
    assert_eq!(
        EMPTY_SHA256,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(crypto::sha256_hex(&[]), EMPTY_SHA256);
}

#[test]
fn test_sha256_hex() {
    assert_eq!(
        crypto::sha256_hex(b"hello world"),
        "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
    );
}

#[test]
fn test_hmac_sha256_hex() {
    let string_to_sign =
        crypto::build_string_to_sign("POST", "/v1/optimize", "1716301234", EMPTY_SHA256);
    let expected_sts =
        "POST\n/v1/optimize\n1716301234\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    assert_eq!(string_to_sign, expected_sts);

    let sig = crypto::hmac_sha256_hex("sec_test_secret_key_123", &string_to_sign);
    assert_eq!(sig.len(), 64);
}

#[test]
fn test_sanitize_message() {
    let raw = "Failed key sp_sdk_1234567890abcdef1234567890abcdef with sec_secret123456";
    let sanitized = sanitize_message(raw);

    assert!(!sanitized.contains("sp_sdk_1234567890abcdef1234567890abcdef"));
    assert!(!sanitized.contains("sec_secret123456"));
    assert!(sanitized.contains("sp_sdk_123...cdef"));
    assert!(sanitized.contains("***REDACTED***"));
}

#[test]
fn test_secret_redacted_wrapper() {
    let secret = SecretRedacted("sp_sdk_1234567890abcdef1234567890abcdef".to_string());
    assert_eq!(format!("{:?}", secret), "[REDACTED]");
    assert_eq!(format!("{}", secret), "[REDACTED]");
}

#[test]
fn secret_redacted_never_prints_a_server_issued_secret() {
    let raw = format!("sps_{}", "ab12".repeat(16));
    let secret = SecretRedacted(raw.clone());
    assert_eq!(format!("{}", secret), "[REDACTED]");
    assert_eq!(format!("{:?}", secret), "[REDACTED]");
    assert!(!sanitize_message(&format!("bad key {raw}")).contains(&raw));
}
