# Remote desktop reliable sessions

This crate implements the transport/session portion of P2. Native capture,
encoding, decoding, presentation and input are separate adapters. The current
tests use deterministic bytes; they do not demonstrate network video playback.

## Integration

`AuthenticatedDeviceConnection::open_remote_desktop` probes the additive
`remote_desktop_schema_version` capability before opening streams over the
existing authenticated `PabConnection`. There is no second credential exchange.
Old peers and executors without a registered backend remain unsupported.

Register a `DesktopBackend` using
`DeviceSessionAcceptor::with_remote_desktop_backend` after configuring its task
service. Register once before accepting connections: each connection gets its
own identity but shares the device's single-viewer slot. Do not construct a new
host per connection. The normal session handler supplies the authenticated
operator/account and owns the cleanup guard.

The viewer calls `next_frame`, decodes frames in sequence, and calls
`report_presented` after consumption/display. Presentation reports are cumulative
and coalesced asynchronously; the render loop does not await one network RTT per
frame. A successful call confirms local queuing, not delivery to the executor.
At most four frames can be outstanding. Encoded reference frames are never
discarded arbitrarily to recover latency.

## Wire and lifetime

- Open/close requests use ordinary task streams. Control and video each use a
  separate reliable bidirectional stream. The reverse video direction stays
  open as a lifetime signal and carries no application data.
- JSON carries bounded metadata only. Each encoded frame has a validated header
  followed by ordered binary chunks of at most 64 KiB, using the existing frame
  transport. A frame is at most 16 MiB; the old 256 KiB transport limit is unchanged.
- Session ID, generation and exact frame sequence are checked before allocation.
  Each generation starts with a keyframe; the media provider must supply Annex B
  H.264 with the SPS/PPS and IDR needed to initialize a decoder. Framing validation
  does not parse or validate the H.264 bitstream itself.
- `select_screen` stops the old source before publishing a new generation and
  attaching its video stream. Selecting the same ID restarts a stalled stream.
  Layout includes stable screen ID, negative origins, native coordinate size,
  pixel size, DPI scale and rotation. Normalized input maps to the selected
  desktop rectangle; coordinates already describe the visibly rotated screen.
- A partial frame read/write that times out or is cancelled permanently consumes
  that stream handle. The next call fails rather than parsing leftover payload
  as another header. Recover with a new generation or a new viewer.
- Heartbeat is 5 seconds and abandonment lease is 15 seconds. Control loss,
  connection loss, verified account change and explicit close release the viewer.
  A closing slot remains occupied until the active source has been dropped.
  Cleanup from an old session cannot remove a replacement session.
- Dropping a client only ends its desktop streams. The shared connection and
  existing task, terminal and transfer streams remain owned by their callers.

## Native adapter contract

`screens` must report the current desktop layout. `start` is called only after
the video stream is attached, once per generation. `DesktopSource::next_frame`
must be cancellation-safe: dropping the source or its pending operation must
stop/release capture and encoding resources. Any native worker queue must be
bounded; transport backpressure does not bound an adapter's private queue.

The adapter owns frame pacing, capture timestamps, codec/color compatibility and
encoder resource lifetime. GPU textures should pass directly to compatible
hardware encoders. This crate carries compressed bytes, not raw pixel textures.
Do not log/archive frame payloads. No application configuration is introduced.

## Input contract (schema v2)

Input starts disabled, including after every generation change. A backend opts
in using `supports_input` and creates a `DesktopInputSink` only after the first
video frame has been sent and the viewer explicitly enables input. Native
backends currently retain the default unsupported implementation.

`input_control()` returns a generation-bound handle which can send controls
while `next_frame_when_changed()` is pending. Never cancel the pending frame to
handle mouse/keyboard events: a partial video read cannot be resumed. An old
handle cannot operate a new generation or disable its input. The bounded
existing control stream carries input; there is no new device authentication.

The sink must release its own held keys/buttons on drop, including cancellation
of `apply`. A timeout retires the sink without replaying the uncertain event.
Native implementations must bind to the capture helper/session, check layout
changes and provide crash recovery; the session tests use fake sinks and do
not demonstrate operating-system injection or recovery. Input Debug output is
redacted. Never log serialized input or password text.

## Independent cursor framing (integration in progress)

`DesktopCursorReader` / `DesktopCursorWriter` provide bounded framing on an
existing reliable stream. `DesktopHost` now attaches it to the existing owner,
connection and generation, and `DesktopBackend::cursor` supplies snapshots.
Only backends opting in with `supports_cursor` advertise the optional
`remote_desktop_cursor_schema_version` environment field. The Bridge passes
this capability to the session; unknown/absent versions retain existing v2
video/input behavior and do not open cursor streams. Native helper adapters
and renderers are still pending, so production backends currently keep the
default unsupported cursor capability.

Each update binds session, generation and exact sequence. A new image has a
monotonic generation-local ID; subsequent position/visibility updates reference
only the latest image and contain no pixel payload. Replacing a generation
requires an initial image again. Images are at most 512 x 512: premultiplied BGRA
or interleaved AND/XOR BGRA for classic inverted cursors, at most 2 MiB. The
existing 64 KiB binary chunks and transport limits are retained.

Positions describe the hotspot in the selected screen's desktop coordinate
space, including negative origins. Image pixels use the already-oriented
visible capture space; renderers must apply viewport scaling and hotspot offset
without rotating the shape twice. The future capture integration must explicitly
select embedded/separate/unavailable cursor handling to prevent double drawing.

Static cursor streams may remain idle. Partial data has deadlines; failed or
cancelled IO retires the cursor stream without closing the shared connection.
Shape metadata is validated before allocating a payload. Cursor images have no
Debug implementation and must not be archived in logs.

`RemoteDesktopSession::open_cursor` returns an independent reader with
`next_update`. It consumes and acknowledges each update with an 8-byte
little-endian sequence in the reverse framed direction. This confirms receipt
and validation, not display. The host permits only one outstanding update;
the source keeps its latest snapshot while waiting, independent of video
credit. There is one cursor stream per video generation. Cursor stream errors
preserve video and input; restart the generation to obtain a new cursor stream.
Screen selection, close and session drop invalidate retained readers. Closing
or changing a generation waits for both native video and cursor sources to end.

Remaining integration: bind the source to the selected native helper, collect
platform cursor metadata, and overlay it in all native renderers with clipping,
scaling and hidden-state cleanup. Tests using generated snapshots do not
demonstrate any visible cursor in the app.

## Verification

Run compilation sequentially, with no other build active and at most 10 total
build jobs (including native builds):

```text
cargo test -p pab-protocol -p pab-remote-desktop -j 8 -- --test-threads=2
cargo test -p pab-executor -p pab-bridge remote_desktop -j 8 -- --test-threads=2
```

Real local QUIC fixtures cover large-frame chunking, stalled-consumer backpressure,
concurrent legacy requests, generation replacement, atomic single ownership,
connection loss, cancellation and malformed headers. Executor/Bridge tests cover
capability compatibility and task dispatch. Native network video, forced Relay,
bandwidth/latency adaptation, screen input and login desktops still require their
own integration and machine acceptance runs.
