# Sapsan generator

Drawing code is adapted from the Flussonic Sapsan synthetic video generator:

- `syntetic.rs`: seven bars, square dimensions, reflected diagonal movement,
  YUV420p drawing, neutral chroma in the bottom band.
- `text_overlay.rs`: original 95 ASCII glyphs, atlas, blur, layout, drawing.
- `pts_luma.rs`: original decimal PTS luma band encoder and decoder.

The copy lives in `src/sapsan/`. The streaming runtime and media-server
configuration are not dependencies. Local FrameRate, FrameSize and
SourceIdentity replace their Sapsan equivalents; calendar formatting uses
std-only Gregorian conversion instead of the time crate. Cached bars avoid
redrawing the background; packed SDI conversion reuses identical rows.
The frame index is also encoded in a small binary strip at the top, so
picture continuity is independent of rounded millisecond PTS.

All offered packed formats are produced from the same YUV420p picture.
Audio uses sixteen distinct phase-continuous tones at 1000 + 250*n Hz,
48 kHz, 24-bit samples. This deliberate extension of the visual source
makes channel duplication, swapping and dropped samples detectable.
