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
fn v210_partial_groups_fit_the_advertised_row_stride() {
    for width in [1280, 1282] {
        let packed = Layout {
            width,
            height: 16,
            fourcc: fourcc("SD10").unwrap(),
            stride: (width * 2).div_ceil(3) * 4,
            ..Layout::default()
        };
        let reference = Layout {
            fourcc: fourcc("SDUY").unwrap(),
            stride: width * 2,
            ..packed
        };
        let mut actual = vec![0; (packed.stride * packed.height) as usize];
        let mut expected = vec![0; (reference.stride * reference.height) as usize];
        video(&mut expected, reference, 17).unwrap();
        video(&mut actual, packed, 17).unwrap();
        for y in 0..packed.height {
            for x in 0..width {
                assert_eq!(
                    luma(&actual, packed, x, y),
                    luma(&expected, reference, x, y)
                );
            }
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
fn eac3_bursts_preserve_real_frames_across_video_boundaries() {
    let frames = eac3::frames();
    assert_eq!(frames.len(), 8);
    for (n, frame) in frames.iter().enumerate() {
        assert_eq!((frame[4] >> 1) & 7, 7); // 3 front, 2 surround.
        assert_eq!(frame[4] & 1, 1); // LFE, six decoded channels in total.
        let phase = n as u64 * 1536;
        assert_eq!(eac3::word(phase + 1, 0) >> 16, 16);
        assert_eq!(eac3::word(phase + 1, 1) >> 16, (frame.len() * 8) as u32);
        let recovered: Vec<_> = (0..frame.len() / 2)
            .flat_map(|i| {
                ((eac3::word(phase + 2 + i as u64 / 2, i % 2) >> 16) as u16).to_be_bytes()
            })
            .collect();
        assert_eq!(recovered, *frame);
    }
    let (l, m) = geometry("SDUY");
    let config = Config {
        channels: 2,
        nonpcm: true,
        eac3: true,
        anc: false,
        vbi: false,
        ..Config::default()
    };
    let mut generator = Generator::new(config.clone());
    let mut buffers = [
        vec![0; l.stride as usize * l.height as usize],
        vec![0; 262144],
        vec![0; 262144],
        vec![0; 128],
        vec![0; 48960],
    ];
    let mut stats = Stats::default();
    for seq in 0..20 {
        let [a, b, c, d, e] = &mut buffers;
        let lengths = generator.fill([a, b, c, d, e], l, m).unwrap();
        if seq == 19 {
            buffers[1][37 * 64 + 2] ^= 1;
        }
        stats
            .frame(
                std::array::from_fn(|i| &buffers[i][..lengths[i] as usize]),
                l,
                m,
                seq,
                0,
                seq as u64 + 1,
                Some(&config),
            )
            .unwrap();
        if seq < 19 {
            assert_eq!(stats.failure_count, 0, "{:?}", stats.errors);
        }
    }
    stats.finish(Some(&config));
    assert!(stats
        .errors
        .iter()
        .any(|e| e.contains("transport payload mismatches")));
}

#[test]
fn windowed_audio_accepts_packet_jitter_but_rejects_sample_loss() {
    fn run(windowed: bool, loss: usize) -> Stats {
        let (layout, mut mode) = geometry("SDUY");
        mode.num = 50;
        mode.den = 1;
        let config = Config {
            channels: 2,
            anc: false,
            vbi: false,
            ..Config::default()
        };
        let mut generator = Generator::new(config.clone());
        let mut buffers = [
            vec![0; layout.stride as usize * layout.height as usize],
            vec![0; 262144],
            vec![0; 262144],
            vec![0; 128],
            vec![0; 48960],
        ];
        let mut stats = Stats::default();
        stats.windowed_audio = windowed;
        let mut phase = 0;
        for seq in 0..32 {
            let [a, b, c, d, e] = &mut buffers;
            let mut lengths = generator.fill([a, b, c, d, e], layout, mode).unwrap();
            let count = (if seq % 2 == 0 { 968 } else { 952 }) - loss;
            audio(&mut buffers[1], count, phase, &config).unwrap();
            put32(&mut buffers[3], 28, count as u32);
            lengths[1] = (count * 64) as u32;
            stats
                .frame(
                    std::array::from_fn(|i| &buffers[i][..lengths[i] as usize]),
                    layout,
                    mode,
                    seq,
                    0,
                    seq as u64 + 1,
                    Some(&config),
                )
                .unwrap();
            phase += count as u64;
        }
        stats.finish(Some(&config));
        stats
    }
    assert!(run(false, 0).failure_count > 0);
    let good = run(true, 0);
    assert_eq!(good.failure_count, 0, "{:?}", good.errors);
    let lost = run(true, 192);
    assert!(lost.errors.iter().any(|e| e.contains("audio window")));
}
#[test]
fn pcm_reports_channel_skew_without_accepting_it_or_hiding_corruption() {
    fn run(skew: bool, corrupt: bool) -> Stats {
        let (layout, mut mode) = geometry("SDUY");
        mode.num = 25;
        mode.den = 1;
        let config = Config {
            channels: 16,
            anc: false,
            vbi: false,
            ..Config::default()
        };
        let mut buffers = [
            vec![0; layout.stride as usize * layout.height as usize],
            vec![0; 262144],
            vec![0; 262144],
            vec![0; 128],
            vec![0; 48960],
        ];
        let [a, b, c, d, e] = &mut buffers;
        let lengths = Generator::new(config.clone())
            .fill([a, b, c, d, e], layout, mode)
            .unwrap();
        if skew {
            for i in 0..lengths[1] as usize / 64 {
                for ch in 14..16 {
                    put32(
                        &mut buffers[1],
                        (i * 16 + ch) * 4,
                        (tone(i as u64 + 3, ch) * 256) as u32,
                    );
                }
            }
        }
        if corrupt {
            // Outside the initial phase-fitting window: a later glitch must
            // not be explained away as a constant channel offset.
            put32(&mut buffers[1], (500 * 16 + 15) * 4, 0x40000000);
        }
        let mut stats = Stats::default();
        stats
            .frame(
                std::array::from_fn(|i| &buffers[i][..lengths[i] as usize]),
                layout,
                mode,
                0,
                0,
                1,
                Some(&config),
            )
            .unwrap();
        stats.finish(Some(&config));
        stats
    }
    assert_eq!(run(false, false).failure_count, 0);
    let skew = run(true, false);
    assert!(skew.failure_count > 0);
    assert!(skew.errors.iter().any(|e| e.contains("15:+3, 16:+3")));
    let corrupt = run(true, true);
    assert!(corrupt.failure_count > 0);
    assert!(!corrupt.errors.iter().any(|e| e.contains("phase offsets")));
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

#[test]
fn teletext_header_and_row_roundtrip() {
    for header in [true, false] {
        let p = teletext::packet(123, header);
        let mut line = vec![0; 1440];
        teletext::render(&mut line, &p).unwrap();
        assert_eq!(teletext::slice(&line).unwrap(), p);
        assert!(teletext::slice(&vec![0; 1440]).is_err());
        let op = teletext::op47(123);
        assert_eq!(op[2] as usize, op.len());
        assert_eq!(op.iter().fold(0u8, |s, b| s.wrapping_add(*b)), 0);
    }
}

#[test]
fn frozen_counter_text_is_detected_independently_of_picture_marker() {
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
    let fill = |g: &mut Generator, buffers: &mut [Vec<u8>; 5]| {
        let [a, b, c, d, e] = buffers;
        g.fill(
            [
                a.as_mut_slice(),
                b.as_mut_slice(),
                c.as_mut_slice(),
                d.as_mut_slice(),
                e.as_mut_slice(),
            ],
            l,
            m,
        )
        .unwrap()
    };
    let lens = fill(&mut g, &mut buffers);
    let old = buffers[0].clone();
    let mut stats = Stats::default();
    stats
        .frame(
            std::array::from_fn(|i| &buffers[i][..lens[i] as usize]),
            l,
            m,
            0,
            0,
            1,
            Some(&c),
        )
        .unwrap();
    let lens = fill(&mut g, &mut buffers);
    let mut picture = sapsan::Picture::new(l.width, l.height, m.num, m.den);
    picture.render(0);
    let (x, y, w, h) = picture.frame_rect().unwrap();
    for yy in y..y + h {
        let range = yy * l.stride as usize + x * 2..yy * l.stride as usize + (x + w) * 2;
        buffers[0][range.clone()].copy_from_slice(&old[range]);
    }
    assert_eq!(marker(&buffers[0], l).unwrap(), 1);
    let e = stats
        .frame(
            std::array::from_fn(|i| &buffers[i][..lens[i] as usize]),
            l,
            m,
            1,
            0,
            2,
            Some(&c),
        )
        .unwrap_err();
    assert!(e.contains("frame-counter text"));
}

#[test]
fn encoded_audio_payload_corruption_is_detected() {
    let (l, m) = geometry("SDUY");
    let c = Config {
        nonpcm: true,
        ..Config::default()
    };
    let mut g = Generator::new(c.clone());
    let mut buffers: [Vec<u8>; 5] = [
        vec![0; l.stride as usize * l.height as usize],
        vec![0; 262144],
        vec![0; 262144],
        vec![0; 128],
        vec![0; 48960],
    ];
    let [a, b, cc, d, e] = &mut buffers;
    let lens = g.fill([a, b, cc, d, e], l, m).unwrap();
    put32(&mut buffers[1], 128, 0x12340000);
    let mut stats = Stats::default();
    stats
        .frame(
            std::array::from_fn(|i| &buffers[i][..lens[i] as usize]),
            l,
            m,
            0,
            0,
            1,
            Some(&c),
        )
        .unwrap();
    assert!(stats
        .errors
        .iter()
        .any(|e| e.contains("337M transport payload")));
}

#[test]
fn scte104_fixture_uses_vanc_y_for_hd() {
    let (_, mode) = geometry("SDUY");
    let packet = fixtures(1, mode)
        .into_iter()
        .find(|p| p.did == 0x41 && p.sdid == 7)
        .unwrap();
    assert_eq!(
        packet.flags, 0,
        "ST 2010 section 6 requires the HD Y stream"
    );
}
