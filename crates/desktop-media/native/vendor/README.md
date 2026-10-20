# NVIDIA encode API header

`nvEncodeAPI.h` comes from `FFmpeg/nv-codec-headers`, tag `n12.1.14.0`:
https://github.com/FFmpeg/nv-codec-headers/blob/n12.1.14.0/include/ffnvcodec/nvEncodeAPI.h

SHA-256: `dbdaa8bcbc2b9325bd24bc68b36170a8432411518040e8d4c8421608cf217425`.
The NVIDIA MIT license/copyright notice is preserved in the header. This is an
API declaration, not a bundled driver. The installed NVIDIA driver supplies
`nvEncodeAPI64.dll`; missing/incompatible hardware selects the software encoder.

Our DXGI, encoder, decoder and presenter implementation was developed and
validated in this repository's remote-desktop experiments. Product builds use
the sources here and require no experiment directory, downloaded SDK or network.

## Bundled software H.264

Cargo pins `openh264` 0.9.8 and `openh264-sys2` 0.9.9 from crates.io. The latter
includes Cisco source revision `1bca62311e447fd9c39fc93de2c4b96cd5ce971c`
(its `upstream/VERSION`), rather than a downloaded binary. The wrapper revision
is `68f56e8443f1d619a9d96af79dd7f836442b6d76` in
https://github.com/ralfbiedert/openh264-rs. Both crates declare BSD-2-Clause;
the upstream license and binding attribution ship in
`packaging/desktop/windows/THIRD-PARTY-NOTICES.txt`, also included in Unix archives.
NASM is a build dependency on x86/x64; it is not shipped or required at runtime.

The wrapper's `num_threads` option alone retains single-slice output. We use
its documented raw API after lazy initialization to select four fixed slices,
preserving dimensions, profile, color and buffer contracts, then force IDR.
Small images retain one slice. Dimension changes create a fresh codec instance.
