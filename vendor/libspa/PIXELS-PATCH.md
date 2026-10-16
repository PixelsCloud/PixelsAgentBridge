# Ubuntu 22.04 header compatibility

Upstream libspa 0.9.2, MIT license, from the locked Cargo registry archive.
This patch is selected only by the Linux GUI's Cargo manifest; the core/headless
workspace does not select it. Windows/macOS do not build this Linux dependency.

The raw-video flags field was added after Ubuntu 22.04's PipeWire 0.3.48 headers.
The build script detects whether the field exists in the actual installed header.
On older headers, constructing a zero-flags descriptor works, querying flags gives
empty flags, and requesting unsupported flags fails explicitly. Modifier access
preserves its bits across the old signed and newer unsigned C declarations.

No system header is replaced and no newer PipeWire runtime is bundled. All other
bindings/source are unchanged. Remove this patch when the upstream binding handles
both layouts or the GUI's oldest supported platform no longer needs it.
