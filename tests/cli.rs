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

#[test]
fn software_run_saves_jsonl_and_offline_html() {
    let dir = std::env::temp_dir().join(format!("validator-html-run-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("run.jsonl");
    let html = dir.join("run.html");
    let out = Command::new(env!("CARGO_BIN_EXE_validator-v4l2"))
        .args(["software", "--frames", "1", "--report"])
        .arg(&log)
        .arg("--html")
        .arg(&html)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(std::fs::read_to_string(&log)
        .unwrap()
        .contains("\"status\":\"PASS\""));
    let page = std::fs::read_to_string(&html).unwrap();
    assert!(page.contains("SDI validator report"));
    assert!(page.contains("crc_errors"));
    assert!(!page.contains("{{LOG}}"));
    assert!(!page.contains("{{SCRIPT}}"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn report_preserves_log_and_escapes_active_markup() {
    let dir = std::env::temp_dir().join(format!("validator-html-safe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("run.jsonl");
    let html = dir.join("run.html");
    let payload = "{\"status\":\"FAIL\",\"case\":\"</textarea><script>malicious()</script>\",\"detail\":\"a&b\"}\n";
    std::fs::write(&log, payload).unwrap();
    let binary = env!("CARGO_BIN_EXE_validator-v4l2");
    let result = Command::new(binary)
        .args(["report", "--file"])
        .arg(&log)
        .arg("--html")
        .arg(&html)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let page = std::fs::read_to_string(&html).unwrap();
    assert!(!page.contains("</textarea><script>malicious()"));
    assert!(page.contains("&lt;/textarea&gt;&lt;script&gt;malicious()"));
    let same = Command::new(binary)
        .args(["report", "--file"])
        .arg(&log)
        .arg("--html")
        .arg(&log)
        .output()
        .unwrap();
    assert_eq!(same.status.code(), Some(2));
    assert_eq!(std::fs::read_to_string(&log).unwrap(), payload);
    let alias = dir.join("alias.html");
    std::fs::hard_link(&log, &alias).unwrap();
    let alias_result = Command::new(binary)
        .args(["report", "--file"])
        .arg(&log)
        .arg("--html")
        .arg(&alias)
        .output()
        .unwrap();
    assert_eq!(alias_result.status.code(), Some(2));
    assert_eq!(std::fs::read_to_string(&log).unwrap(), payload);
    std::fs::remove_dir_all(dir).unwrap();
}
