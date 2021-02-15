# Comms

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
5. Tell Rust to use the `gnu` toolchain so it can find GTK and opus: `rustup default stable-x86_64-pc-windows-gnu`
6. From `comms`, run `cargo run`
