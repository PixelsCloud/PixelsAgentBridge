# Generated codec fixture

`apple-low-latency.pabvideo` contains eight 1920×1080 H.264 frames generated on
Apple M4 by this project's `capture ... synthetic` diagnostic. It contains no
captured desktop, user data, credentials or third-party artwork. Each of four
quadrants changes RGB every frame using the native worker's test pattern.

The encoder uses VideoToolbox's hardware low-latency mode, High profile,
no reordering, one temporal base layer, no LTR and one reference buffer.
Both Windows hardware and bundled OpenH264 decoding were checked against all
four colors. Ordinary VideoToolbox mode stalled the bounded Windows decoder;
the low-latency mode's default reference count also exposed stale software
images. This fixture guards the tested interoperable stream, not arbitrary
H.264 profiles or delayed/reordered output.

Layout is diagnostic runtime data, little endian: `PABMAC1\0`, four u32 fields
(width, height, FPS, frame count), then per-frame (u32 compressed size, u64 index,
u64 sampled microseconds, u32 keyframe flag) followed by Annex-B bytes.

Source worker SHA-256:
`cbe37ce18d7adb3a6660586d5bbb941c59488d9cadd7d1aa0dd8793acd53f2bc`.
The complete source run is documented in the macOS codec acceptance record.
