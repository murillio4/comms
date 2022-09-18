# Comms client

Note: when developing in an IDE that's in a snap, you probably need to override the 
pulseaudio daemon with `/run/user/1000/pulse/native`.

## Packaging for Windows

1. Download [`warp`](https://github.com/dgiagio/warp) and put `warp-packer.exe` on your `$PATH`.
2. To have the icon set as well, you'll need [Resource Hacker](http://angusj.com/resourcehacker) on your `$PATH` as well. 
2. Run `.\package.ps1` in PowerShell.
3. 🎉
