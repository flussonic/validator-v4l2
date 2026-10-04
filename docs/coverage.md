# Coverage

The validator distinguishes a successful queue, an observed external signal,
a verified loop and an unavailable feature. End-to-end success requires
our generator on the connected transmitter. Quick plans cover every advertised timing up to 4096x2160 with a representative
format, all formats at representative HD/4K modes, and feature variations.
`--exhaustive` and endurance mode cover every timing/format combination.
The receiver must support the selected mode/format too.

Implemented checks:

- Five-plane ABI and metadata v4, lengths, reserved fields, vendor-tail bounds.
- SD, HD, 3G, 6G and 12G modes that the driver enumerates, including fractional
  rates, interlaced audio field counts, picture marker continuity and PTS band.
- SDUY, SDYU, SDYV, SD16, SD10, SDAR, SDXU generation and sampled picture checks.
- MMAP, aligned USERPTR and DMA-heap-backed DMABUF import. Drivers rejecting a
  requested memory type produce a failing case; heap absence is operational.
- Audio present mask separately from measured nonzero channels; tone payload,
  channel order, cadence, continuity, padding and missing output metadata.
- SMPTE 337M transport fixture, exact burst payload/continuity and non-PCM metadata. The fixture is
  a transport test, not an encoded AC-3/Dolby E decoder test.
- ANC header/UDW bounds and parity, checksum flags, line bounds and exact
  frame-associated payloads for ATC, AFD, SCTE-104, OP-47, captions and a
  private frame counter. Caption payloads are transport fixtures,
  not a semantic decoder or a standards certification suite. Unknown packets,
  including driver-produced VPID, are counted and validated structurally.
- Metadata Rec.2020/HLG/PQ and Level A/B statements, alternation within a
  stream, sysfs readback and restoring settings on normal return/error.
- SD VBI waveform passthrough, EN 300 706 teletext page 100 generation/slicing
  on PAL lines 10/11 and 323/324, and OP-47 teletext packets. Raw planes can
  be saved with `--dump`.
- Sequence gaps, error flags, monotonic clocks and increases in exposed
  loss/CRC/DMA/restart/underflow/ANC/audio counters.
- Output-only endurance is also available with `soak --device OUTPUT` and
  reports queue completion as OBSERVED, without certifying reception.
- Endurance sessions change mode, format, channel count, HDR/level flags and
  buffer memory, restart streams and randomize matrix order using a seed.

Limits: picture checks sample the image, not every pixel; the Sapsan source
is 8-bit 4:2:0 before packing and does not prove preservation of every 10-bit
LSB or 4:2:2 chroma sample. Teletext uses a fixed-rate slicer; WSS, VITC and line-21 captions are not
semantically decoded. Genlock, clock trimming, reference offsets, cable
loss/replug, PCI removal, suspend/resume and encoded-audio decoding require
external stimuli; inventory reports their settings but does not certify them.
Busy/disconnected inputs cannot prove capture support. Software validation
checks userspace only. The same matrix can be fetched automatically over SSH from the transmitter.

Reference sources: [ETSI EN 300 706](https://www.etsi.org/deliver/etsi_en/300700_300799/300706/01.02.01_60/en_300706v010201p.pdf)
for teletext signalling/address protection;
[Linux DMA-BUF synchronization](https://www.kernel.org/doc/html/latest/driver-api/dma-buf.html)
for CPU-access bracketing. These protocols are implemented independently;
GPL driver utilities are not copied.
