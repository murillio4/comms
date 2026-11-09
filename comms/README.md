# Comms desktop client

The frontend uses Tauri 2, React, TypeScript, Vite, and shadcn/ui with Tailwind CSS. Rust owns audio,
Opus encoding, networking, peer discovery, session state, and preferences.
Audio samples never pass through JavaScript. Tauri is the desktop window
and command/event bridge; it uses WebView2 on Windows, WKWebView on macOS,
and WebKitGTK on Linux.

The compact interface opens at 360 × 420 and can resize down to 300 × 320.
Long participant lists, errors, and expanded audio settings scroll with the window.
The local shadcn components live in `frontend/components/ui`; theme tokens and
compact layout styles are in `frontend/style.css`. To add a component, run
`npx shadcn@latest add <component>` from this directory.

## Requirements

- Rust stable and Node.js 22.12+ (or a newer supported LTS).
- [Tauri's platform prerequisites](https://v2.tauri.app/start/prerequisites/).
  On Linux this includes WebKitGTK **4.1**, GTK **3**, build tools, and ALSA
  development headers. The `comms` client no longer needs GTK4 or Libadwaita.
  Windows needs the MSVC build tools and WebView2; macOS needs Xcode command
  line tools.
- CMake and a C/C++ compiler for the bundled Opus library.

## Develop

From this directory:

```sh
npm ci
npm run tauri dev
```

The Tauri CLI discovers `tauri.conf.json` and `Cargo.toml` here (there is no
nested `src-tauri` directory). It starts Vite on port 1420 and the `comms`
Rust binary. `npm run dev` alone serves the interface but requires the
Tauri runtime for native commands. `npm run build` builds frontend assets;
run it before building the client directly with Cargo.

```sh
npm run build
cargo run -p comms -- comms://localhost
```

The optional command-line server argument prefills the connection form.
The server continues to use TCP/UDP port 18847. Server hostnames/IPs are
supported; custom ports are not supported by the existing network backend.

## Package

```sh
npm run tauri build
```

Tauri builds native installers/bundles into `../target/release/bundle/`.
Build on each target OS; this does not provide automatic cross-compilation.
macOS bundles include the microphone usage description and audio-input
entitlement. Grant microphone access on first launch. For distribution, configure
Tauri’s [platform signing settings](https://v2.tauri.app/distribute/).
Windows `package.ps1` is a convenience wrapper for this command. GTK DLL
bundling, Warp, and Resource Hacker are no longer used. `sign.ps1 -FilePath`
can manually sign a Windows executable/installer using the previous
`SIGN_TOOL_PATH`, `CERTIFICATE`, and `CERTIFICATE_PASSWORD` environment variables.

## Behavior and migration

- Connect/disconnect, participant lists, speaking indicators, estimated
  one-way latency, direct peer connections, and join/leave sounds are retained.
- Each participant has a 0–400% volume slider; your own slider controls
  headphone volume. Settings use the previous `ProjectDirs("", "Comms", "Comms")`
  `local_storage` file, preserving usernames, server, UUID, and saved volumes.
- Audio devices are listed for reference. The audio backend still uses the
  system defaults; restart after changing devices. Initialization errors stay
  visible in the window instead of terminating the application.
- Session keepalive and peer-discovery tasks are cancelled on disconnect.
- Protocol version, packet identifiers, and serialization are unchanged.
- The old Windows updater downloaded GTK/Warp executables. It has been removed;
  install updated Tauri bundles manually until a signed Tauri updater is added.
- Tauri's deep-link plugin replaces the old Warp registry code. Installed
  bundles register `comms://hostname` links. Windows/Linux open a new process
  (as the GTK client did); macOS also delivers links to a running app. Links
  prefill the form; they do not automatically connect or switch an active call.
  OS link registration needs an installed bundle; `cargo run` does not install it.
- This migration targets Windows, macOS, and Linux desktop, not mobile.

## Validate

```sh
npm test
npm run build
cargo fmt --all -- --check
cargo test -p comms
cargo test -p comms-lib
cargo test -p comms-server
cargo test --workspace
cargo clippy -p comms --all-targets --no-deps -- -D warnings
```

A real microphone and a running server are needed for end-to-end voice
verification. Browser component tests mock the command/event bridge.
