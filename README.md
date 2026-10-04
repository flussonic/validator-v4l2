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
./validator-v4l2 loop --pair /dev/video4=/dev/video0 --mode 1080p25 --frames 250
# Full enumerated output matrix, up to UHD/DCI 4K, plus feature variations.
./validator-v4l2 plan --pair /dev/video4=/dev/video0
./validator-v4l2 quick --pair /dev/video4=/dev/video0 --report matrix.jsonl
# Repeat the matrix in a reproducible random order for a day.
./validator-v4l2 soak --pair /dev/video4=/dev/video0 --duration 86400 --seed 1 --report soak.jsonl
# Test the software generator/validator without a board.
./validator-v4l2 software --frames 12 --report software.jsonl
```

A paired case checks the generated picture and frame counter, audio tones and
continuity, ANC payloads, HDR/Level B metadata, SD VBI waveforms, frame errors,
sequence gaps, timestamps and exposed driver counters. Modes, formats and
settings are discovered from the driver. Missing counters remain unknown.
The generator includes the Sapsan seven bars, bouncing square, changing
text and PTS band; see [generator provenance](docs/generator.md).

For two servers, install the same executable on both. The coordinator can
run on either server or a workstation; SSH uses the configured keys:

```sh
./validator-v4l2 loop --tx-host root@sender --rx-host root@receiver --remote-bin /usr/local/bin/validator-v4l2 --pair /dev/video4=/dev/video0 --mode 1080p25
./validator-v4l2 quick --tx-host root@sender --rx-host root@receiver --pair /dev/video4=/dev/video0 --report remote.jsonl
```

Reports are append-only JSON Lines. Exit 0 means no attempted case failed,
1 means validation failure, 2 means invalid arguments or an operational error.
A report with SKIP/OBSERVED entries does not certify all functions. Actual
coverage and limitations are documented in [coverage](docs/coverage.md).
