# Encoded audio fixture

`eac3-5.1-48k.eac3` contains eight independently decodable E-AC-3 syncframes:
48 kHz, 384 kbit/s, six blocks per frame, 5.1 channels. It encodes synthetic
tones and is distributed under this repository's MIT license. There is no
third-party programme audio. Reproduce it with FFmpeg:

```sh
ffmpeg -f lavfi -i 'aevalsrc=0.2*sin(2*PI*1000*t)|0.2*sin(2*PI*1250*t)|0.2*sin(2*PI*1500*t)|0.2*sin(2*PI*80*t)|0.2*sin(2*PI*1750*t)|0.2*sin(2*PI*2000*t):s=48000:c=5.1:d=0.256' -c:a eac3 -b:a 384k -bitexact -f eac3 eac3-5.1-48k.eac3
ffprobe -v error -count_frames -show_streams eac3-5.1-48k.eac3
ffmpeg -v error -i eac3-5.1-48k.eac3 -f null -
```

The validator embeds these frames; building and running need no FFmpeg.
`--eac3` uses a 48 kHz AES3 stereo carrier, ST 340 data type 16 and a
1536-sample burst interval. The ST 337 `Pd` length is in bits. This is
distinct from consumer IEC 61937 E-AC-3 type 21, with byte lengths and a
different carrier. Six encoded programme channels occupy two physical
audio slots; `--channels 2` checks just those slots, and higher values add
PCM tones in the remaining slots.

The receiver checks the expected compressed bytes across video-frame and
fixture-cycle boundaries, including preamble, type, bit length and padding.
It checks transport integrity without implementing an E-AC-3 decoder.

References: [SMPTE ST 337](https://pub.smpte.org/latest/st337/st0337-2015.pdf)
and [SMPTE ST 340](https://pub.smpte.org/doc/st340/20080604-pub/st0340-2008.pdf).
