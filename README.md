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
