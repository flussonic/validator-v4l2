// SPDX-License-Identifier: MIT
use super::*;
use crate::{
    pattern::{anc, put32, video},
    Options,
};
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn captured_scte35_crc_corruption_is_rejected() {
    let raw = include_bytes!("../fixtures/scte35-generated.ts");
    assert_eq!(ts_events(raw).unwrap().len(), 1);
    let mut damaged = *raw;
    // Corrupt the section, preserving TS framing and command identification.
    let payload = 4 + if damaged[3] & 0x20 != 0 {
        1 + damaged[4] as usize
    } else {
        0
    };
    let section = payload + 1 + damaged[payload] as usize;
    damaged[section + 17] ^= 1;
    assert!(ts_events(&damaged).unwrap_err().contains("CRC"));
}

#[test]
fn inspection_detects_the_one_frame_anc_association_bug() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "validator-anc-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    let layout = Layout {
        width: 1920,
        height: 1080,
        stride: 3840,
        fourcc: fourcc("SDUY").unwrap(),
        ..Layout::default()
    };
    let mode = Mode {
        width: 1920,
        height: 1080,
        total_lines: 1125,
        num: 25,
        den: 1,
        ..Mode::default()
    };
    let mut picture = vec![0; 3840 * 1080];
    video(&mut picture, layout, 31).unwrap();
    fs::write(dir.join("video0-0-0.bin"), picture).unwrap();
    let mut meta = vec![0; 144];
    for (off, value) in [
        (0, 0x30494453u32),
        (4, 4),
        (116, 16),
        (120, u32::from_le_bytes(*b"MWEC")),
        (124, 1),
        (128, 15),
        (132, 3),
    ] {
        meta[off..off + 4].copy_from_slice(&value.to_le_bytes());
    }
    fs::write(dir.join("video0-0-3.bin"), &meta).unwrap();
    let mut options = Options {
        cmd: "inspect-anc".into(),
        values: [
            ("--dump".into(), dir.to_str().unwrap().into()),
            ("--anc-types".into(), "60/60,41/05,41/07".into()),
        ]
        .into(),
        switches: vec!["--expect".into()],
    };
    for (fragments, anc_frame, missing, expected) in [
        (false, 31, false, true),
        (false, 32, false, false),
        (true, 31, false, true),
        (true, 32, false, false),
        (true, 31, true, false),
    ] {
        options.switches.retain(|s| s != "--scte104-fragments");
        if fragments {
            options.switches.push("--scte104-fragments".into());
        }
        let mut ps: Vec<_> = fixture_packets(anc_frame, mode, fragments)
            .into_iter()
            .filter(|p| [(0x60, 0x60), (0x41, 5), (0x41, 7)].contains(&(p.did, p.sdid)))
            .map(|mut p| {
                p.line = 0;
                p
            })
            .collect();
        if missing {
            let last = ps
                .iter()
                .rposition(|p| (p.did, p.sdid) == (0x41, 7))
                .unwrap();
            ps.remove(last);
        }
        put32(&mut meta, 132, ps.len() as u32);
        fs::write(dir.join("video0-0-3.bin"), &meta).unwrap();
        let mut data = vec![0; 1024];
        let size = anc(&mut data, &ps).unwrap();
        fs::write(dir.join("video0-0-2.bin"), &data[..size]).unwrap();
        assert_eq!(inspect_anc(&options).unwrap(), expected);
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn direct_http_capture_strips_headers_and_validates_the_wire_section() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        while !request.windows(4).any(|v| v == b"\r\n\r\n") {
            let mut buffer = [0u8; 1024];
            let n = socket.read(&mut buffer).unwrap();
            assert!(n > 0 && request.len() + n <= 4096);
            request.extend_from_slice(&buffer[..n]);
        }
        assert!(std::str::from_utf8(&request)
            .unwrap()
            .starts_with("GET /stream HTTP/1.0\r\n"));
        socket
            .write_all(b"HTTP/1.0 200 OK\r\nContent-Type: video/mp2t\r\n\r\n")
            .unwrap();
        socket
            .write_all(include_bytes!("../fixtures/scte35-generated.ts"))
            .unwrap();
    });
    let body = capture_http(&format!("http://{address}/stream"), 1).unwrap();
    assert_eq!(ts_events(&body).unwrap().len(), 1);
    server.join().unwrap();
}
