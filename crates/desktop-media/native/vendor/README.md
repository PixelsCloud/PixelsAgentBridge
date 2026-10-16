# NVIDIA encode API header

`nvEncodeAPI.h` comes from `FFmpeg/nv-codec-headers`, tag `n12.1.14.0`:
https://github.com/FFmpeg/nv-codec-headers/blob/n12.1.14.0/include/ffnvcodec/nvEncodeAPI.h

SHA-256: `dbdaa8bcbc2b9325bd24bc68b36170a8432411518040e8d4c8421608cf217425`.
The NVIDIA MIT license/copyright notice is preserved in the header. This is an
API declaration, not a bundled driver. The installed NVIDIA driver supplies
`nvEncodeAPI64.dll`; missing/incompatible hardware returns media unavailable.

Our DXGI, encoder, decoder and presenter implementation was developed and
validated in this repository's remote-desktop experiments. Product builds use
the sources here and require no experiment directory, downloaded SDK or network.
