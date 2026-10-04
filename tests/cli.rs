// SPDX-License-Identifier: MIT
use std::process::{Command, Stdio};
#[test]
fn coordination_flags_are_accepted_by_the_real_binary() {
    let out = Command::new(env!("CARGO_BIN_EXE_validator-v4l2"))
        .args(["help", "--wire-plan", "--exhaustive", "--stop-on-stdin"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("validator-v4l2"));
}
#[test]
fn invalid_parameters_fail_before_accessing_hardware() {
    for options in [
        vec!["receive", "--channels", "17"],
        vec!["transmit", "--flags", "48"],
        vec!["receive", "--frames", "0"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_validator-v4l2"))
            .args(options)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(!String::from_utf8_lossy(&out.stderr).contains("QUERYCAP"));
    }
}
