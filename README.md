# Comms

> **About this fork:** This is a fork of [Comms](https://gitlab.com/comms-app/comms)
> by Mitchell Hentges, based on commit `c13b2343` (2023-10-21). Per the original
> project's delayed-license grant, all work over two years old is licensed under
> the GNU AGPLv3, so this fork is distributed under the
> [GNU AGPLv3](LICENSE).

![screenshot of the connect pane](doc/screenshot-connect.png)
![screenshot of the user-list pane](doc/screenshot-user-list.png)

## Features

* Lightweight app, consumes < 15MB of memory
* Optimized audio pipelines to keep latency low
* Uses peer-to-peer connections to reduce latency further

## Latency benchmarks

Round-trip audio latency, as casually measured from residential internet:

```
[From ~Toronto]
+-----------------------------+-------+
| Comms (Local Network)       | 66ms  |
+-----------------------------+-------+
| TeamSpeak 3 (Local Network) | 99ms  |
+-----------------------------+-------+
| Discord (<closest>)         | 207ms |
+-----------------------------+-------+
|                             |       |
+-----------------------------+-------+
| Comms (West Coast NA)       | 121ms |
+-----------------------------+-------+
| TeamSpeak 3 (West Coast NA) | 179ms |
+-----------------------------+-------+
| Discord (West Coast NA)     | 248ms |
+-----------------------------+-------+
```

## Audio backend

`audio-device` uses CPAL 0.15 for native audio I/O (ALSA on Linux,
WASAPI on Windows, CoreAudio on macOS). It opens the default input and
output devices, converts their native sample formats and rates to/from
48 kHz mono, and keeps application processing off the audio callback threads
using bounded queues. Capture overflow drops incoming samples; playback
underflow produces silence.

The shared network protocol is unchanged. The former PulseAudio/WASAPI
backend modules are no longer exported; use `audio_device::cpal` or
`create_minimal_event_loop`. `PULSEAUDIO_SERVER` no longer selects the backend.
Device IDs in listings are CPAL device names and may not be unique. Restart
the audio session after a device disconnect or default-device change;
automatic device switching is not currently implemented.

## Desktop client (Windows, macOS, Linux)

The `comms` frontend uses Tauri 2 and React, with native Rust audio and
networking. Install Rust, Node.js, and the
[Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/), then:

```sh
cd comms
npm ci
npm run tauri dev
```

Use `npm run tauri build` for native application bundles/installers.
See [the client README](comms/README.md) for platform dependencies,
packaging, migration details, and validation commands.

## Wireshark Decoder
See `extras/comms-dissector.lua` for a script that can be used in Wireshark to parse the internet traffic.

Put the `comms-dissector.lua` script in `<Path to Wireshark folder>\Wireshark\plugins`

On Windows it is typically `C:\Program Files\Wireshark\plugins`.

Use (if tshark is in your paths) `tshark -G plugins` to see currently installed plugins and their respective paths. If the Comms lua decoder is properly installed it should show up in the list as e.g:
```
comms-dissector.lua             lua script      C:\Program Files\Wireshark\plugins\comms-dissector.lua
```
This plugin should also show up in Wireshark: Help -> About Wireshark -> Plugins.

To decode a packet, right click on packet (with e.g. port 18847) -> Decode As -> Current -> COMMS -> Save
