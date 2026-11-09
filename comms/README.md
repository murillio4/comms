# Comms client

## Packaging for Windows

1. Download [`warp`](https://gitlab.com/comms-app/warp) and put `warp-packer.exe` on your `$PATH`.
2. To have the icon set as well, you'll need [Resource Hacker](http://angusj.com/resourcehacker) on your `$PATH` as well.
3. Set `$EDITBIN_PATH` to point to your Visual Studio SDK's `editbin` tool.
   * For example: `$env:EDITBIN_PATH="C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC\14.33.31629\bin\Hostx64\x64\editbin.exe"`
4. Run `.\package.ps1` in PowerShell.
5. 🎉
