# Comms

[😎 Fetch the most recently built Comms for Windows here! 😎](https://gitlab.com/comms-app/comms/-/jobs/artifacts/main/raw/comms.exe?job=build_release_windows)

![screenshot of the connect pane](doc/screenshot-connect.png)
![screenshot of the user-list pane](doc/screenshot-user-list.png)

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

## Linux Build Instructions

1. Install `rust`, GTK4 libs, Libadwaita, and opus libs
2. From `comms`, run `cargo run`

## Windows Build Instructions

1. Install [`rust`](https://rustup.rs/). Follow the directions and install the Visual Studio tools Rust recommends.
2. Follow the [`gvsbuild` steps to build GTK3 with MSVC tools](https://github.com/wingtk/gvsbuild)
   * When the `gvsbuild` steps recommend setting `$env:Path`, make it a permanent change by doing the following:
      1. From the Start menu, go to the Control Panel entry for “Edit environment variables for your account”.
      2. Double-click the `Path` row in the top list of variables. Click “New” to add a new item to the list.
      3. Paste in `C:\gtk-build\gtk\x64\release\bin`
      4. Click "OK" twice.
3. Build Libadwaita:
   1. From the Windows start menu, run `x64 Native Tools Command Prompt`
   2. `mkdir C:\gtk-build\github`
   3. `cd C:\gtk-build\github`
   4. `git clone https://gitlab.gnome.org/GNOME/libadwaita.git --depth=1 --branch libadwaita-1-2`
   5. `cd libadwaita`
   6. `pipx install meson`
   7. `meson setup builddir -Dprefix=C:\gtk-build\gtk\x64\release -Dintrospection=disabled -Dvapi=false -Dexamples=false -Dtests=false`
   8. `meson install -C builddir`
4. Ensure Rust is using the `msvc` toolchain, if it isn't already: `rustup default stable-msvc`
5. From `comms`, run `cargo run --bin comms`

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

Use (if tshark is in your paths) `tshark -G plugins` to see currently installed plugins and their respective paths.

To use the Plugin one should be able to right click on the COMMS.

 Right click on packet (with e.g. port 18847) -> Decode As -> Current -> COMMS -> Save
