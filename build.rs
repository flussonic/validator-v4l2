// SPDX-License-Identifier: MIT
use std::{env, path::PathBuf, process::Command};
fn main() {
    println!("cargo:rerun-if-changed=native/v4l2.c");
    println!("cargo:rerun-if-changed=include/sdi_av.h");
    if env::var("CARGO_CFG_TARGET_OS").unwrap() != "linux" {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    assert!(Command::new(env::var("CC").unwrap_or("cc".into()))
        .args([
            "-std=gnu11",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-Iinclude",
            "-c",
            "native/v4l2.c",
            "-o"
        ])
        .arg(out.join("v4l2.o"))
        .status()
        .unwrap()
        .success());
    assert!(Command::new(env::var("AR").unwrap_or("ar".into()))
        .arg("crs")
        .arg(out.join("libv4l2.a"))
        .arg(out.join("v4l2.o"))
        .status()
        .unwrap()
        .success());
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=v4l2");
}
