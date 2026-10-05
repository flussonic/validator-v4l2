# validator-v4l2

A standalone Rust validator for the five-plane SDI V4L2 contract shared by
Flussonic's DeckLink, AJA, DekTec, Stream Labs, AVMatrix and Magewell drivers.
MIT application, bundled original `include/sdi_av.h` with its syscall exception.
No vendor SDK, Rust crate downloads or FFmpeg are needed.

`--scte104-fragments` selects a 287-byte multi-operation SCTE-104 message:
a splice request plus 63 avail identifiers, carried in two ST 2010 ANC
packets on consecutive VANC lines. Use the option on `transmit`, expected
`receive` and `inspect-anc`; omitted means the existing single-packet
fixture. Inspection compares the complete per-type payload multiset and
rejects missing, duplicated or adjacent-picture fragments. This checks
raw transport, not SCTE-104 assembly by a downstream media server.

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
./validator-v4l2 quick --pair /dev/video4=/dev/video1 --report matrix.jsonl --html matrix.html
# Every timing/format combination.
./validator-v4l2 quick --pair /dev/video4=/dev/video1 --exhaustive --report full.jsonl
# Repeat the matrix in a reproducible random order for a day.
./validator-v4l2 soak --pair /dev/video4=/dev/video1 --duration 86400 --seed 1 --report soak.jsonl --html soak.html
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

For distributed testing, start the built-in HTTP agent on each server:

```sh
export VALIDATOR_HTTP_TOKEN="same-long-random-token-on-agents-and-coordinator"
./validator-v4l2 serve --listen 0.0.0.0:8787
```

Run one coordinator on a workstation or either server:

```sh
./validator-v4l2 quick --tx-url http://sender:8787 --rx-url http://receiver:8787 --pair /dev/video4=/dev/video1 --report remote.jsonl --html remote.html
./validator-v4l2 soak --tx-url http://sender:8787 --rx-url http://receiver:8787 --pair /dev/video4=/dev/video1 --duration 86400 --report soak.jsonl --html soak.html
```

The coordinator discovers modes, starts both workers, renews their leases,
collects results and stops transmission. Agents stop orphaned workers when a
lease expires. No SSH commands are used in HTTP mode. See the
[agent protocol](docs/http-agent.md) for inventory, job APIs and authentication.
The old SSH options remain available for existing scripts.

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

## Saved reports

`--report result.jsonl --html result.html` saves both machine-readable data
and an offline HTML report on the coordinating server. It works with local
pairs and `--tx-host` / `--rx-host` remote pairs. For an existing log:

```sh
./validator-v4l2 report --file result.jsonl --html result.html
```

The report filters by result and searches devices, modes and errors. It shows
receiver frames, gaps, reported CRC errors, detected audio channels and ANC;
transmitter failures remain failures even when the receiver passes. SKIP and
OBSERVED never count as PASS. No browser packages or network access are needed.
