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

For a quick check of all boards on one remote server:

```sh
# On the server with the boards:
./validator-v4l2 serve
# On the workstation:
./validator-v4l2 quickcheck --agent http://first-server:5040
# Save a report on the workstation:
./validator-v4l2 quickcheck --agent http://first-server:5040 --report quick.jsonl --html quick.html
# Exercise a connected output/input loop on that server:
./validator-v4l2 quickcheck --agent http://first-server:5040 --pair /dev/video4=/dev/video1 --report loop.jsonl --html loop.html
```

`quickcheck` automatically saves uniquely named `.html` and `.json` reports
in the coordinator's current directory, alongside an append-only `.jsonl` log.
The JSON file is a standard array of result records. Each hardware result
includes `boards` with the board name, driver, PCI bus, device node and agent.
The HTML shows board names in the case table and connection map. `--report PATH` selects
the log filename; HTML/JSON names are derived from it unless overridden with
`--html PATH` / `--json PATH`. Failed and interrupted checks also export reports.
The HTML is self-contained, with status totals, search, filters and per-case
video/audio/ANC/CRC measurements and error details.

Without `--pair`, `quickcheck` automatically inventories every board on the
agent and on the local Linux coordinator, sends uniquely identified video
probes through each compatible output/input candidate, and discovers local
loops and connections between hosts in both directions. It then runs the
video/audio/ANC/HDR matrix on every discovered route. Capture evidence proves
the connection even when PCM or CRC validation fails; those features are
checked separately in the matrix and remain failures.

For a coordinator without boards, add a second agent with
`--peer-agent http://second-server:5040`. Both servers must run the current
validator. Duplicate endpoints on the same Linux machine are scanned once.
`--discover-only` saves just the inventory and connection map;
`--inventory-only` inventories and captures locked inputs without transmitting.
An explicit `--pair` tests only that pair. The legacy `quick` command retains
its inventory/locked-input behavior.

Discovery sends a live signal and changes settings on outputs, so the test
ports must be free. Probes run sequentially, reopen the transmitter per
candidate for half-duplex boards and have bounded capture timeouts. Only three
or more consecutive frames with this probe's video identity establish a route.
Discovery uses one common mode/format per candidate, preferring HD 25 fps and
SDUY; an undetected connection is reported as SKIP, never as full coverage.
Unmatched inputs with an external signal can be observed, but their source
content cannot be verified. Unsupported receiver timings/formats are SKIP.


For distributed testing, start the built-in HTTP agent on each server:

```sh
./validator-v4l2 serve
```

Run one coordinator on a workstation or either server:

```sh
./validator-v4l2 quick --tx-url http://sender:5040 --rx-url http://receiver:5040 --pair /dev/video4=/dev/video1 --report remote.jsonl --html remote.html
./validator-v4l2 soak --tx-url http://sender:5040 --rx-url http://receiver:5040 --pair /dev/video4=/dev/video1 --duration 86400 --report soak.jsonl --html soak.html
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
