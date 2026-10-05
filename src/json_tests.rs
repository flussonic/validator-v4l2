// SPDX-License-Identifier: MIT
use super::*;
#[test]
fn control_messages_roundtrip_unicode_and_escaped_logs() {
    let value = object(&[
        (
            "args",
            Value::Array(vec![Value::Str("— \"SDI\"\n\\".into())]),
        ),
        ("lease_secs", Value::Int(15)),
    ]);
    assert_eq!(parse(&value.encode()).unwrap(), value);
    assert_eq!(
        parse("\"\\ud83d\\ude00\"").unwrap(),
        Value::Str("😀".into())
    );
}
#[test]
fn malformed_or_ambiguous_control_messages_are_rejected() {
    for input in [
        "{\"x\":1,\"x\":2}",
        "[1,]",
        "01",
        "1.5",
        "\"\\ud800\"",
        "true false",
        "\"a\nb\"",
    ] {
        assert!(parse(input).is_err(), "{input}");
    }
    assert!(parse(&format!("{}0{}", "[".repeat(30), "]".repeat(30))).is_err());
}
