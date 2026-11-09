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

## Development environment

Install [mise](https://mise.jdx.dev/installing-mise.html), then run these commands
from the repository root:

```sh
mise trust
mise install
mise run check
```

The root `mise.toml` pins Rust 1.91.0 and installs rustfmt and Clippy through
[mise's Rust support](https://mise.jdx.dev/lang/rust.html). Tasks run from the
repository root, including when invoked from a crate directory. Shell activation
is optional: use `mise exec -- cargo run -p comms` to run the client with the
configured toolchain.

Available checks:

| Command | Check |
| --- | --- |
| `mise run fmt` | Workspace formatting |
| `mise run test` | Shared library and server tests |
| `mise run check` | Formatting, shared library and server tests |
| `mise run test:workspace` | Tests for every crate |
| `mise run clippy` | Clippy for every crate and target, denying warnings |

Install native build tools and libraries separately; `mise install` manages the
Rust toolchain. The desktop client requires GTK4, Libadwaita 1.2 or newer, Opus,
and `pkg-config`. Linux audio also requires ALSA development libraries. Follow
the platform instructions below before building the client or running the
workspace checks.

On macOS, install the Xcode Command Line Tools (`xcode-select --install`) and the
native libraries with Homebrew:

```sh
brew install gtk4 libadwaita opus pkg-config
```

## Linux Build Instructions

1. Set up Rust with mise as described above, and install GTK4, Opus, ALSA development libraries (`libasound2-dev` on Debian/Ubuntu), and `pkg-config`
2. Install Libadwaita 1.2 (likely by building it from source)
3. Run `mise exec -- cargo run -p comms` from the repository root

## Windows Build Instructions

1. Install the Visual Studio C++ build tools and set up Rust with mise as described above. Use the MSVC Rust host toolchain.
2. Follow the [`gvsbuild` steps to build GTK4 with MSVC tools](https://github.com/wingtk/gvsbuild)
   * When the `gvsbuild` steps recommend setting `$env:Path`, make it a permanent change by doing the following:
      1. From the Start menu, go to the Control Panel entry for “Edit environment variables for your account”.
      2. Double-click the `Path` row in the top list of variables. Click “New” to add a new item to the list.
      3. Paste in `C:\gtk-build\gtk\x64\release\bin`
      4. Click "OK" twice.
3. Build Libadwaita with `gvsbuild`: `gvsbuild build libadwaita`
4. Ensure Rust is using the `msvc` toolchain, if it isn't already: `rustup default stable-msvc`
5. Run `mise exec -- cargo run -p comms` from the repository root

### Windows _Faster_ Build Instructions

Incremental builds of Comms can be a little slow - one of the main delays here is linking.
You can speed this up by using the LLVM linker instead of the default MSVC one:

1. Download and run the most recent `LLVM-$version-win64.exe` [from the LLVM releases page](https://github.com/llvm/llvm-project/releases)
2. Add `C:\Program Files\LLVM\bin` to your `PATH`
3. Edit `$env:USERPROFILE\.cargo\config.toml` and add the following contents:
    ```
    [target.x86_64-pc-windows-msvc]
    linker = "lld-link.exe"
    ```
4. Run `cargo clean`, then `cargo build --bin comms`

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
