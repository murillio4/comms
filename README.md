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

1. Install `rust`, GTK3 libs, opus libs
2. From `comms`, run `cargo run`

## Windows Build Instructions

1. Install [`rust`](https://rustup.rs/). Follow the directions and install the Visual Studio tools Rust recommends.
2. Install [`msys2`](http://www.msys2.org/)
3. From the msys shell, install packages: `pacman -S mingw-w64-x86_64-gtk3 mingw-w64-x86_64-toolchain mingw-w64-x86_64-opus`
4. Adjust your environment variables:
    * `GTK_LIB_DIR` should be `C:\msys64\mingw64\lib`
    * `PATH` should now include `C:\msys64\mingw64\bin`
5. Tell Rust to use the `gnu` toolchain so it can find GTK and opus: `rustup default stable-gnu`
6. From `comms`, run `cargo run --bin comms`

### Windows _Faster_ Build Instructions

When the Windows package of Comms is created, it uses the "GNU" toolchain and the resources installed via `msys2` as
details in the previous section. However, iterative builds with the "GNU" toolchain are a little slow.

To improve build times (and allow the use of Windows investigative development tools), you can use the "MSVC" toolchain
instead. At the time of writing, this reduced build times from 10s to 3s on my machine.

1. Follow the [`gvsbuild` steps to build GTK3 with MSVC tools](https://github.com/wingtk/gvsbuild)
   * When adding the `C:\gtk-build\...` directories to your `PATH`, do it from the "System Properties" menu:
     `$env:Path` (as recommended in the `gvsbuild` docs) only affects the shell that runs that command
   * Remember to remove the `GTK_LIB_DIR` environment variable and `PATH` modification from the "GNU"-toolchain-based
     instructions in the previous section, if you added them
2. Run `rustup default stable-msvc`
3. Verify that the MSVC toolchain is working: from `comms`, run `cargo run --bin comms`
   * If you get an error about `gtk-3.lib`, your adjustments to the `PATH` variable aren't being picked up properly:
     try rebooting you computer, and/or run `cargo clean`, then try again.
4. Tell Rust to use a faster linker:
   1. Download and run the most recent `LLVM-$version-win64.exe` [from the LLVM releases page](https://github.com/llvm/llvm-project/releases)
   2. Add `C:\Program Files\LLVM\bin` to your `PATH`
   3. Edit `$env:USERPROFILE\.cargo\config.toml` and add the following contents:
```
[target.x86_64-pc-windows-msvc]
linker = "lld-link.exe"
```
5. Run `cargo clean`, then `cargo build --bin comms`

The only behavioural difference I found between the "GNU" and "MSVC" builds is the theming of the title bar.

## Wireshark Decoder
See `extras/comms-dissector.lua` for a script that can be used in Wireshark to parse the internet traffic.

Put the `comms-dissector.lua` script in `<Path to Wireshark folder>\Wireshark\plugins`

On Windows it is typically `C:\Program Files\Wireshark\plugins`.

Use (if tshark is in your paths) `tshark -G plugins` to see currently installed plugins and their respective paths.

To use the Plugin one should be able to right click on the COMMS.

 Right click on packet (with e.g. port 18847) -> Decode As -> Current -> COMMS -> Save
