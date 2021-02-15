# Comms client

Note: when developing in an IDE that's in a snap, you probably need to override the 
pulseaudio daemon with `/run/user/1000/pulse/native`.

## Packaging for Windows

1. `cargo build --release`
2. Run `comms.exe`, check Process Explorer to see what DLLs are in use.
3. Copy the DLLs , `gdbus.exe`, and `comms.exe` to a folder.
4. Copy `lib\gdk-pixbuf-2.0\2.10.0\loaders\libpixbufloader-png.dll` to the folder.
5. Create a file called `loaders.cache`:
```
# GdkPixbuf Image Loader Modules file
# Automatically generated file, do not edit
# Created by gdk-pixbuf-query-loaders from gdk-pixbuf-2.42.2
#
# LoaderDir = C:\msys64\mingw64\lib\gdk-pixbuf-2.0\2.10.0\loaders
#
"libpixbufloader-png.dll"
"png" 5 "gdk-pixbuf" "PNG" "LGPL"
"image/png" ""
"png" ""
"\211PNG\r\n\032\n" "" 100


```
6. Create a file called `launch.cmd`:
```
@ECHO OFF

SETLOCAL
SET GDK_PIXBUF_MODULE_FILE=%~dp0\loaders.cache

CALL %~dp0\comms.exe %*
EXIT /B %ERRORLEVEL%
```
7. Download [`warp`](https://github.com/dgiagio/warp), then do `warp-packer.exe --arch windows-x64 --input_dir $folder --exec launch.cmd --output comms.exe`.
8. 🎉
