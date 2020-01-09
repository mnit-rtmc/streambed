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

The streambed configuration file `streambed.muon` is in [MuON] format.  On
Linux, it is located in the `~/.config/streambed/` directory.

Use `streambed --help` for instructions on how to configure the service.

## Control Protocol

If `control-port` is specified, _streambed_ listens for connections on that TCP
port.  No more than a single connection is accepted at a time.

A message consists of a **command**, followed by any number of **parameters**,
ending with an ASCII _group separator_ `U+001D`.  Each parameter begins with a
_record separator_ `U+001E` followed by the parameter **name**, a _unit
separator_ `U+001F`, then the **value**.

Any parameter not specified in a message is left unchanged.

When an `IN` message is received, it is acknowledged with an `OUT` response
containing the **command** and no parameters.

### Config (`IN`)

A `config` message sets global values.

Parameter      | Description
---------------|-----------------------------------------------------
`acceleration` | Video acceleration method: `NONE`, `VAAPI`, or `OMX`
`flows`        | Total number of flows: `0` to `255`
`grid`         | Flows in window grid: `0` to `16`

### Flow (`IN`)

A `flow` message sets values for one flow.

Parameter         | Description
------------------|----------------------------
`number`          | `0` to `flows` minus one
`location`        | source location URI
`rtsp-transport`  | `ANY`, `UDP`, `MCAST`, `TCP` (use `TCP` when packet loss is high)
`source-encoding` | `PNG`, `MJPEG`, `MPEG2`, `MPEG4`, `H264`, `VP8`, `VP9`
`timeout`         | source timeout in seconds
`latency`         | buffering latency in milliseconds
`overlay-text`    | overlay text
`address`         | sink UDP address
`port`            | sink UDP port
`sink-encoding`   | only set if different than `source-encoding`
`title-bar`       | `HIDE` or `SHOW`
`title-accent`    | title bar accent color (rgb hex: `000000` -> black)
`font-size`       | title bar font size (pt)
`monitor-id`      | title bar monitor ID
`camera-id`       | title bar camera ID
`title-text`      | title bar text
`title-extra`     | title bar extra text
`aspect-ratio`    | `FILL` or `PRESERVE`
`matrix-width`    | `1` to `8`
`matrix-x`        | `0` to `matrix-width` minus one
`matrix-height`   | `1` to `8`
`matrix-Y`        | `0` to `matrix-height` minus one
`matrix-hgap`     | `0` to `10000` (hundredths of percent of window)
`matrix-vgap`     | `0` to `10000` (hundredths of percent of window)

### Status (`OUT`)

A `status` message is sent on each flow state change or statistics update.

Parameter  | Description
-----------|----------------------------
`number`   | `0` to `flows` minus one
`location` | source location URI
`state`    | `STARTING`, `PLAYING`, `FAILED`
`pushed`   | pushed packet count
`lost`     | lost packet count
`late`     | late packet count


[MuON]: https://github.com/muon-data/muon
