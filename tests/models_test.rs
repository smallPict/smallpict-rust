use smallpict::models::{
    CompressionMode, FitMode, ImageFormat, OptimizeOptions, PurgeResponse, QuotaResponse,
};

#[test]
fn test_optimize_options_builder() {
    let opts = OptimizeOptions::builder()
        .format(ImageFormat::Avif)
        .quality(85)
        .max_width(1920)
        .max_height(1080)
        .max_dimension(1600)
        .fit(FitMode::Contain)
        .mode(CompressionMode::Extreme)
        .strip_metadata(true)
        .filename("banner.png")
        .mime_type("image/png")
        .idempotency_key("idemp_123")
        .build();

    assert_eq!(opts.format, Some(ImageFormat::Avif));
    assert_eq!(opts.quality, Some(85));
    assert_eq!(opts.max_width, Some(1920));
    assert_eq!(opts.max_height, Some(1080));
    assert_eq!(opts.max_dimension, Some(1600));
    assert_eq!(opts.fit, Some(FitMode::Contain));
    assert_eq!(opts.filename, Some("banner.png".to_string()));
    assert_eq!(opts.idempotency_key, Some("idemp_123".to_string()));

    let json_str = serde_json::to_string(&opts).expect("valid serialization");
    assert!(json_str.contains("\"format\":\"avif\""));
    assert!(json_str.contains("\"quality\":85"));
    assert!(json_str.contains("\"max_dimension\":1600"));
    assert!(json_str.contains("\"mode\":\"extreme\""));
    assert!(!json_str.contains("lossless")); // deprecated flag never serialized
    assert!(!json_str.contains("idempotency_key")); // skipped from JSON body
}

#[test]
fn test_serde_models() {
    let quota_json = r#"{
        "plan": "api_velocity",
        "bytes_used": 5000000,
        "quota_limit": 10000000,
        "quota_percentage": 50.0,
        "active_keys_count": 3
    }"#;
    let quota: QuotaResponse = serde_json::from_str(quota_json).expect("valid quota deserialize");
    assert_eq!(quota.plan, "api_velocity");
    assert_eq!(quota.bytes_used, 5000000);
    assert_eq!(quota.active_keys_count, Some(3));

    let purge_json = r#"{"message":"Purge completed"}"#;
    let purge: PurgeResponse = serde_json::from_str(purge_json).expect("valid purge deserialize");
    assert_eq!(purge.message, "Purge completed");
}

#[test]
fn negative_bytes_saved_parses() {
    let st: smallpict::models::JobStatusResult = serde_json::from_str(
        r#"{"job_id":"j1","status":"completed","url":"u","format":"webp","bytes_saved":-5}"#,
    )
    .unwrap();
    assert_eq!(st.bytes_saved, Some(-5));
}
