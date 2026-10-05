# validator-v4l2

A standalone Rust validator for the five-plane SDI V4L2 contract shared by
Flussonic's DeckLink, AJA, DekTec, Stream Labs, AVMatrix and Magewell drivers.
MIT application, bundled original `include/sdi_av.h` with its syscall exception.
No vendor SDK, Rust crate downloads or FFmpeg are needed.

Linux prerequisites: Rust 1.75+, Cargo, make, a C compiler, ar and Linux UAPI
headers. SSH is needed only for remote endpoints. Building and testing:

```sh
make
make test
./validator-v4l2 --help
./validator-v4l2 list
./validator-v4l2 quick --report quick.jsonl
```

`quick` inventories every node, checks its five-plane ABI, captures inputs
with an available signal and reports busy/disconnected inputs as SKIP. It does
not stop services or guess physical cabling. A capture without our known
source is OBSERVED, not an end-to-end PASS. Output nodes need an explicit pair.

```sh
# A physical SDI loop on one server; use the actual connected, free nodes.
./validator-v4l2 loop --pair /dev/video4=/dev/video1 --mode 1080p25 --frames 250
# Valid 5.1 E-AC-3 on a two-slot SDI carrier.
./validator-v4l2 loop --pair /dev/video4=/dev/video1 --mode 1080p25 --channels 2 --eac3
# Quick coverage of advertised modes/formats up to UHD/DCI 4K, plus features.
./validator-v4l2 plan --pair /dev/video4=/dev/video1
./validator-v4l2 quick --pair /dev/video4=/dev/video1 --report matrix.jsonl
# Every timing/format combination.
./validator-v4l2 quick --pair /dev/video4=/dev/video1 --exhaustive --report full.jsonl
# Repeat the matrix in a reproducible random order for a day.
./validator-v4l2 soak --pair /dev/video4=/dev/video1 --duration 86400 --seed 1 --report soak.jsonl
# Output-only endurance on one server without a connected receiver.
./validator-v4l2 soak --device /dev/video4 --duration 3600 --report output.jsonl
# Test the software generator/validator without a board.
./validator-v4l2 software --frames 12 --report software.jsonl
```

A paired case checks the generated picture and frame counter, audio tones and
continuity, ANC payloads, HDR/Level B metadata, SD VBI waveforms, frame errors,
sequence gaps, timestamps and exposed driver counters. Modes, formats and
settings are discovered from the driver. SIGINT/SIGTERM stops streaming
and restores changed sysfs defaults; paired transmitters also stop when
the receiver finishes. Missing counters remain unknown.
Paired runs discard four startup capture frames before checking payloads and
counter increases. `--warmup 0` includes startup; standalone `receive` defaults
to zero. Discarded frames are separate from `--frames` and reported explicitly.
The generator includes the Sapsan seven bars, bouncing square, changing
text and PTS band; see [generator provenance](docs/generator.md).
Encoded audio uses the bundled [synthetic E-AC-3 fixture](fixtures/README.md).
For inputs that slice audio using packet timestamps, `--windowed-audio`
checks the whole-run sample count within 0.1% plus a tenth of a frame,
allows per-frame count jitter and still checks every tone sample and continuity.
The default checks each frame's sample count within one sample.

For two servers, install the same executable on both. The coordinator can
run on either server or a workstation; SSH uses the configured keys:

```sh
./validator-v4l2 loop --tx-host root@sender --rx-host root@receiver --remote-bin /usr/local/bin/validator-v4l2 --pair /dev/video4=/dev/video1 --mode 1080p25
./validator-v4l2 quick --tx-host root@sender --rx-host root@receiver --pair /dev/video4=/dev/video1 --report remote.jsonl
```

Reports are append-only JSON Lines. Exit 0 means no attempted case failed,
1 means validation failure, 2 means invalid arguments or an operational error.
A report with SKIP/OBSERVED entries does not certify all functions. Actual
coverage and limitations are documented in [coverage](docs/coverage.md).


Capture and test sources belong to this validator, not individual driver
repositories. ANC replay and generated SCTE forwarding checks:

```
validator-v4l2 receive --device /dev/video0 --mode 1080p25 --channels 8 --frames 250 --warmup 4 --expect --windowed-audio --no-vbi --dump /tmp/anc-dump
validator-v4l2 inspect-anc --dump /tmp/anc-dump --mode 1080p25 --anc-types 60/60,41/05,41/07,41/01 --expect
validator-v4l2 inspect-ts --url http://capture.example:8080/streaming/mpegts/sdi --duration 5 --expect
validator-v4l2 inspect-ts --file capture.ts --expect
```

`inspect-anc` checks fixture payloads against the embedded picture counter;
it currently accepts 1080p25 dumps. This is a separate, restricted test:
`receive` retains strict ABI/location/all-family checks. Unknown original
location and checksum are reported. Selected types with no generated
reference (such as hardware VPID) get presence checks only, listed in the
report. `inspect-ts` checks CRCs of small unencrypted, uncancelled
splice_insert sections fitting one TS packet. `--expect` additionally
requires consecutive generated event IDs. It does not certify PMT or
splice timing, arbitrary SCTE messages or spanning sections. URL capture
uses plain HTTP; no external capture utility or vendor SDK is required.
