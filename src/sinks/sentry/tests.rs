//! Tests for the Sentry sink.

use sentry::protocol::{LogLevel, TraceId};
use std::collections::BTreeMap;
use uuid::Uuid;
use vector_lib::event::LogEvent;
use vrl::value::Value;

use super::log_conversion::{convert_to_sentry_log, convert_fields_to_attributes, extract_trace_id};
use sentry::Dsn;
use std::str::FromStr;

#[test]
fn test_dsn_parsing() {
    let dsn_str = "https://public_key@sentry.io/12345";
    let dsn = Dsn::from_str(dsn_str).unwrap();
    
    assert_eq!(dsn.scheme().to_string(), "https");
    assert_eq!(dsn.host(), "sentry.io");
    assert_eq!(dsn.port(), 443); // Default HTTPS port
    assert_eq!(dsn.project_id().to_string(), "12345");
    assert_eq!(dsn.public_key(), "public_key");
}

#[test]
fn test_dsn_parsing_with_port() {
    let dsn_str = "https://public_key@example.com:8080/67890";
    let dsn = Dsn::from_str(dsn_str).unwrap();
    
    assert_eq!(dsn.scheme().to_string(), "https");
    assert_eq!(dsn.host(), "example.com");
    assert_eq!(dsn.port(), 8080);
    assert_eq!(dsn.project_id().to_string(), "67890");
    assert_eq!(dsn.public_key(), "public_key");
}

#[test]
fn test_extract_trace_id_with_trace_id_field() {
    let mut log = LogEvent::from("test message");
    let test_uuid = Uuid::new_v4();
    log.insert("trace_id", Value::from(test_uuid.to_string()));

    let (trace_id, used_field) = extract_trace_id(&log);

    assert_eq!(trace_id, TraceId::from(test_uuid.into_bytes()));
    assert_eq!(used_field, Some("trace_id"));
}

#[test]
fn test_extract_trace_id_with_sentry_trace_id_field() {
    let mut log = LogEvent::from("test message");
    let test_uuid = Uuid::new_v4();
    log.insert("sentry.trace_id", Value::from(test_uuid.to_string()));

    let (trace_id, used_field) = extract_trace_id(&log);

    assert_eq!(trace_id, TraceId::from(test_uuid.into_bytes()));
    assert_eq!(used_field, Some("sentry.trace_id"));
}

#[test]
fn test_extract_trace_id_precedence() {
    let mut log = LogEvent::from("test message");
    let trace_uuid = Uuid::new_v4();
    let sentry_uuid = Uuid::new_v4();

    // Add both fields, trace_id should take precedence
    log.insert("trace_id", Value::from(trace_uuid.to_string()));
    log.insert("sentry.trace_id", Value::from(sentry_uuid.to_string()));

    let (trace_id, used_field) = extract_trace_id(&log);

    assert_eq!(trace_id, TraceId::from(trace_uuid.into_bytes()));
    assert_eq!(used_field, Some("trace_id"));
}

#[test]
fn test_extract_trace_id_invalid_uuid() {
    let mut log = LogEvent::from("test message");
    log.insert("trace_id", Value::from("not-a-uuid"));

    let (trace_id, used_field) = extract_trace_id(&log);

    assert_eq!(trace_id, TraceId::from([0u8; 16]));
    assert_eq!(used_field, None);
}

#[test]
fn test_extract_trace_id_no_trace_fields() {
    let log = LogEvent::from("test message");

    let (trace_id, used_field) = extract_trace_id(&log);

    assert_eq!(trace_id, TraceId::from([0u8; 16]));
    assert_eq!(used_field, None);
}

#[test]
fn test_convert_to_sentry_log_basic() {
    let mut log = LogEvent::from("test message");
    log.insert("level", Value::from("info"));

    let sentry_log = convert_to_sentry_log(&log);

    assert_eq!(sentry_log.body, "test message");
    assert_eq!(sentry_log.level, LogLevel::Info);
    assert!(sentry_log.trace_id.is_some());
}

#[test]
fn test_convert_to_sentry_log_all_levels() {
    let test_cases = vec![
        ("trace", LogLevel::Trace),
        ("debug", LogLevel::Debug),
        ("info", LogLevel::Info),
        ("warn", LogLevel::Warn),
        ("warning", LogLevel::Warn),
        ("error", LogLevel::Error),
        ("err", LogLevel::Error),
        ("fatal", LogLevel::Fatal),
        ("critical", LogLevel::Fatal),
        ("alert", LogLevel::Fatal),
        ("emergency", LogLevel::Fatal),
        ("unknown", LogLevel::Info), // Default case
    ];

    for (level_str, expected_level) in test_cases {
        let mut log = LogEvent::from("test message");
        log.insert("level", Value::from(level_str));

        let sentry_log = convert_to_sentry_log(&log);

        assert_eq!(
            sentry_log.level, expected_level,
            "Failed for level: {}",
            level_str
        );
    }
}

#[test]
fn test_convert_to_sentry_log_severity_field() {
    let mut log = LogEvent::from("test message");
    log.insert("severity", Value::from("error"));

    let sentry_log = convert_to_sentry_log(&log);

    assert_eq!(sentry_log.level, LogLevel::Error);
}

#[test]
fn test_convert_to_sentry_log_sentry_level_field() {
    let mut log = LogEvent::from("test message");
    log.insert("sentry.level", Value::from("warn"));

    let sentry_log = convert_to_sentry_log(&log);

    assert_eq!(sentry_log.level, LogLevel::Warn);
}

#[test]
fn test_convert_to_sentry_log_with_trace_id() {
    let mut log = LogEvent::from("test message");
    let test_uuid = Uuid::new_v4();
    log.insert("trace_id", Value::from(test_uuid.to_string()));

    let sentry_log = convert_to_sentry_log(&log);

    assert_eq!(
        sentry_log.trace_id.unwrap(),
        TraceId::from(test_uuid.into_bytes())
    );
}

#[test]
fn test_convert_to_sentry_log_no_message() {
    let log = LogEvent::from(BTreeMap::new());

    let sentry_log = convert_to_sentry_log(&log);

    assert_eq!(sentry_log.body, "");
    assert_eq!(sentry_log.level, LogLevel::Info);
}

#[test]
fn test_convert_fields_to_attributes_excludes_reserved_fields() {
    let mut log = LogEvent::from("test message");
    log.insert("level", Value::from("info"));
    log.insert("severity", Value::from("high"));
    log.insert("timestamp", Value::from("2023-01-01T00:00:00Z"));
    log.insert("trace_id", Value::from("some-trace-id"));
    log.insert("custom_field", Value::from("custom_value"));

    let attributes = convert_fields_to_attributes(&log, Some("trace_id"));

    // Should only contain custom_field, not the reserved fields
    assert_eq!(attributes.len(), 1);
    assert!(attributes.contains_key("custom_field"));
    assert!(!attributes.contains_key("message"));
    assert!(!attributes.contains_key("level"));
    assert!(!attributes.contains_key("severity"));
    assert!(!attributes.contains_key("timestamp"));
    assert!(!attributes.contains_key("trace_id"));
}

#[test]
fn test_convert_fields_to_attributes_different_types() {
    // Start with an empty log to avoid any default fields
    let mut log = LogEvent::from(BTreeMap::new());
    log.insert("string_field", Value::from("test_string"));
    log.insert("int_field", Value::from(42i64));
    log.insert("float_field", Value::from(3.14f64));
    log.insert("bool_field", Value::from(true));
    // Use a simple bytes value that converts to a string rather than an array
    log.insert("bytes_field", Value::from("test_bytes"));

    let attributes = convert_fields_to_attributes(&log, None);

    // Should have 5 attributes
    assert_eq!(attributes.len(), 5);

    // Check that our fields are present
    assert!(attributes.contains_key("string_field"));
    assert!(attributes.contains_key("int_field"));
    assert!(attributes.contains_key("float_field"));
    assert!(attributes.contains_key("bool_field"));
    assert!(attributes.contains_key("bytes_field"));

    // Check string field
    if let Some(attr) = attributes.get("string_field") {
        match &attr.0 {
            sentry::protocol::Value::String(s) => assert_eq!(s, "test_string"),
            _ => panic!("Expected string value"),
        }
    }

    // Check integer field
    if let Some(attr) = attributes.get("int_field") {
        match &attr.0 {
            sentry::protocol::Value::Number(n) => assert_eq!(n.as_i64(), Some(42)),
            _ => panic!("Expected number value"),
        }
    }

    // Check float field
    if let Some(attr) = attributes.get("float_field") {
        match &attr.0 {
            sentry::protocol::Value::Number(n) => assert_eq!(n.as_f64(), Some(3.14)),
            _ => panic!("Expected number value"),
        }
    }

    // Check boolean field
    if let Some(attr) = attributes.get("bool_field") {
        match &attr.0 {
            sentry::protocol::Value::Bool(b) => assert_eq!(*b, true),
            _ => panic!("Expected boolean value"),
        }
    }

    // Check bytes field (should be a string)
    if let Some(attr) = attributes.get("bytes_field") {
        match &attr.0 {
            sentry::protocol::Value::String(s) => assert_eq!(s, "test_bytes"),
            _ => panic!("Expected string value for bytes"),
        }
    }
}