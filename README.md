# Streambed

**Streambed** is a real-time streaming service for video surveillance.  It
handles multiple _flows_ of video, which can be controlled from a separate
system using a simple network protocol.

Each _flow_ has a _source_, which can be RTSP, RTP or HTTP.  The flow can
optionally be transcoded or have a text overlay applied, then sent to a _sink_.
Typically, RTP on a UDP multicast address is used, to allow many clients to view
the video.

## Building

Streambed is written in _Rust_ (1.40+), and depends on **GStreamer** (1.16+).

```
cargo build --release
```

The resulting file will be located at `./target/release/streambed`

## Configuration

The streambed configuration file `~/.config/streambed/streambed.muon` is in
[MuON] format.

Use `streambed --help` for instructions on how to configure the service.

## Control Protocol

A single connection is accepted on TCP port 7001.

Each message ends with an ASCII record separator (`U+001E`).  Within each
message, fields are separated by the ASCII unit separator (`U+001F`).  If all
fields are not present, default values are used.

`IN` messages are acknowledged with an `OUT` response containing only the first
field.

### Config (`IN`)

1. `config`
2. Hardware acceleration (`NONE`, `VAAPI`, or `OMX`)
3. Flow count (`0` to `255`)
4. Grid count (`0` to flow count)

### Flow (`IN`)

1. `flow`
2. Flow index (`0` to flow count minus one)
3. Source location URI
4. Source encoding: `PNG`, `MJPEG`, `MPEG2`, `MPEG4`, `H264`, `H265`, `VP8`,
   `VP9`
5. Source timeout (sec)
6. Buffering latency (ms)
7. Overlay text
8. Sink encoding
9. Sink address
10. Sink port
11. Title bar: accent color (rgb hex: `000000` -> black)
12. Title bar: font size (pt)
13. Title bar: Monitor ID
14. Title bar: Camera ID
15. Title bar: Camera Title
16. Title bar: Extra label
17. Aspect ratio (`FILL` or `PRESERVE`)
18. Matrix X (`0` to width minus one)
19. Matrix width (`1` to `8`)
20. Matrix Y (`0` to height minus one)
21. Matrix height (`1` to `8`)
22. Matrix horizontal gap (`0` to `10000` -- hundredths of percent of window)
23. Matrix vertical gap (`0` to `10000` -- hundredths of percent of window)

### Status (`OUT`)

Sent whenever a flow changes state and on statistics updates.

1. `status`
2. Flow index (`0` to flow count minus one)
3. Stream status (`STARTING`, `PLAYING`, `FAILED`)
4. Pushed packet count
5. Lost packet count
6. Late packet count


[MuON]: https://github.com/muon-data/muon
