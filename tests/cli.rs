// SPDX-License-Identifier: MIT
use std::process::{Command, Stdio};
#[cfg(target_os = "linux")]
#[test]
fn inventory_and_quick_skip_single_plane_device_caps() {
    let library = std::env::temp_dir().join(format!("validator-non-sdi-{}.so", std::process::id()));
    assert!(Command::new("cc")
        .args(["-shared", "-fPIC", "-Wall", "-Wextra", "-Werror"])
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/non_sdi.c"))
        .arg("-o")
        .arg(&library)
        .status()
        .unwrap()
        .success());
    for command in ["list", "quick"] {
        let out = Command::new(env!("CARGO_BIN_EXE_validator-v4l2"))
            .args([command, "--device", "/dev/null"])
            .env("LD_PRELOAD", &library)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let report = String::from_utf8(out.stdout).unwrap();
        assert!(report.contains("\"status\":\"SKIP\""), "{report}");
        assert!(
            report.contains("no multiplanar video capability"),
            "{report}"
        );
        assert_eq!(report.lines().count(), 1, "{report}");
    }
    std::fs::remove_file(library).unwrap();
}
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
