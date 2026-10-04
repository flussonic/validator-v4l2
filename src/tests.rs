// SPDX-License-Identifier: MIT
use super::*;
fn geometry(f: &str) -> (Layout, Mode) {
    let l = Layout {
        width: 720,
        height: 576,
        fourcc: fourcc(f).unwrap(),
        stride: if f == "SD10" {
            1920
        } else {
            720 * if ["SD16", "SDXU", "SDAR"].contains(&f) {
                4
            } else {
                2
            }
        },
        sizes: [0; 5],
    };
    let m = Mode {
        width: 720,
        height: 576,
        num: 30000,
        den: 1001,
        total_lines: 625,
        ..Mode::default()
    };
    (l, m)
}
#[test]
fn video_formats_preserve_marker_and_bars() {
    for f in FORMATS {
        let (l, _) = geometry(f);
        let mut b = vec![0; l.stride as usize * l.height as usize];
        for n in [0, 1, 25, 0x123456789abcdef0] {
            video(&mut b, l, n).unwrap();
            assert_eq!(marker(&b, l).unwrap(), n);
            check_video(
                &b,
                l,
                n,
                Mode {
                    num: 30,
                    den: 1,
                    ..geometry(f).1
                },
            )
            .unwrap();
        }
    }
}
#[test]
fn fractional_audio_cadence_has_no_drift() {
    let (l, m) = geometry("SDUY");
    let mut g = Generator::new(Config::default());
    let mut buffers: [Vec<u8>; 5] = [
        vec![0; l.stride as usize * l.height as usize],
        vec![0; 262144],
        vec![0; 262144],
        vec![0; 128],
        vec![0; 48960],
    ];
    let mut sum = 0;
    for _ in 0..300 {
        let sizes = g
            .fill(
                {
                    let [a, b, c, d, e] = &mut buffers;
                    [
                        a.as_mut_slice(),
                        b.as_mut_slice(),
                        c.as_mut_slice(),
                        d.as_mut_slice(),
                        e.as_mut_slice(),
                    ]
                },
                l,
                m,
            )
            .unwrap();
        sum += sizes[1] / 64;
    }
    assert_eq!(sum, 480480);
}
#[test]
fn anc_rejects_truncation_and_parity() {
    let (_, m) = geometry("SDUY");
    let ps = fixtures(7, m);
    let mut b = vec![0; 1024];
    let n = anc(&mut b, &ps).unwrap();
    assert_eq!(packets(&b[..n]).unwrap(), ps);
    for cut in [1, 7, 9, n - 1] {
        assert!(packets(&b[..cut]).is_err());
    }
    b[9] ^= 1;
    assert!(packets(&b[..n]).is_err());
}
#[test]
fn audio_corruption_fails() {
    let (l, m) = geometry("SDUY");
    let c = Config::default();
    let mut g = Generator::new(c.clone());
    let mut buffers: [Vec<u8>; 5] = [
        vec![0; l.stride as usize * l.height as usize],
        vec![0; 262144],
        vec![0; 262144],
        vec![0; 128],
        vec![0; 48960],
    ];
    let mut s = Stats::default();
    for seq in 0..2 {
        let lens = g
            .fill(
                {
                    let [a, b, c, d, e] = &mut buffers;
                    [
                        a.as_mut_slice(),
                        b.as_mut_slice(),
                        c.as_mut_slice(),
                        d.as_mut_slice(),
                        e.as_mut_slice(),
                    ]
                },
                l,
                m,
            )
            .unwrap();
        if seq == 1 {
            put32(&mut buffers[1], 0, 0x40000000);
        }
        s.frame(
            std::array::from_fn(|i| &buffers[i][..lens[i] as usize]),
            l,
            m,
            seq,
            0,
            seq as u64 + 1,
            Some(&c),
        )
        .unwrap();
    }
    assert!(s.failure_count > 0);
}
#[test]
fn malformed_metadata_cannot_overread() {
    let (l, m) = geometry("SDUY");
    let mut g = Generator::new(Config::default());
    let mut buffers: [Vec<u8>; 5] = [
        vec![0; l.stride as usize * l.height as usize],
        vec![0; 262144],
        vec![0; 262144],
        vec![0; 128],
        vec![0; 48960],
    ];
    let lens = g
        .fill(
            {
                let [a, b, c, d, e] = &mut buffers;
                [
                    a.as_mut_slice(),
                    b.as_mut_slice(),
                    c.as_mut_slice(),
                    d.as_mut_slice(),
                    e.as_mut_slice(),
                ]
            },
            l,
            m,
        )
        .unwrap();
    put32(&mut buffers[3], 28, u32::MAX);
    let mut s = Stats::default();
    assert!(s
        .frame(
            std::array::from_fn(|i| &buffers[i][..lens[i] as usize]),
            l,
            m,
            0,
            0,
            1,
            None
        )
        .is_err());
}
#[test]
fn sequence_wrap_is_legal_but_repeat_is_not() {
    let (l, m) = geometry("SDUY");
    let mut g = Generator::new(Config::default());
    let mut buffers: [Vec<u8>; 5] = [
        vec![0; l.stride as usize * l.height as usize],
        vec![0; 262144],
        vec![0; 262144],
        vec![0; 128],
        vec![0; 48960],
    ];
    let mut s = Stats::default();
    for (i, seq) in [u32::MAX, 0, 0].into_iter().enumerate() {
        let lens = g
            .fill(
                {
                    let [a, b, c, d, e] = &mut buffers;
                    [
                        a.as_mut_slice(),
                        b.as_mut_slice(),
                        c.as_mut_slice(),
                        d.as_mut_slice(),
                        e.as_mut_slice(),
                    ]
                },
                l,
                m,
            )
            .unwrap();
        s.frame(
            std::array::from_fn(|i| &buffers[i][..lens[i] as usize]),
            l,
            m,
            seq,
            0,
            i as u64 + 1,
            None,
        )
        .unwrap();
    }
    assert_eq!(s.failure_count, 1);
}
#[test]
fn quote_json_and_shell() {
    assert_eq!(report::quote("a\n\"\\"), "\"a\\n\\\"\\\\\"");
    assert_eq!(shell_quote("a'b"), "'a'\\''b'");
}

#[test]
fn copied_pts_band_roundtrips() {
    let mut p = vec![0; 720 * 576];
    for pts in [0, 123456789, 9_999_999_999_999_999] {
        sapsan::pts_luma::encode_pts_into_luma(&mut p, 720, 576, pts);
        assert_eq!(
            sapsan::pts_luma::decode_pts_from_luma(&p, 720, 720, 576),
            pts
        );
    }
}

#[test]
fn remote_plan_is_lossless_and_rejects_bad_payload() {
    let c = Case {
        mode: "3840x2160p59.940".into(),
        format: "SD10".into(),
        config: Config {
            flags: 24,
            channels: 8,
            alternate: 25,
            ..Config::default()
        },
        memory: "dmabuf".into(),
    };
    let wire = wire_case(&c);
    let back = parse_plan(&wire).unwrap();
    assert_eq!(wire_case(&back[0]), wire);
    assert!(parse_plan("bad").is_err());
    assert!(parse_plan(&wire.replace("\t8\t25", "\t99\t25")).is_err());
}
