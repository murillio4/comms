Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$msys_dir = "C:\msys64"
if (-not (Test-Path -Path $msys_dir)) {
    echo "MSYS2 is required to package comms (MSYS2 is not installed)"
    exit 1
}
$msys_bin_dir = Join-Path $msys_dir -ChildPath "usr\bin"

$repo_dir = (Get-Item $MyInvocation.MyCommand.Path).Directory.Parent.FullName
Push-Location $repo_dir
cargo build --release --bin comms
if ($LASTEXITCODE -ne 0) {
    Pop-Location
    exit 1
}
Pop-Location

$comms_path = Join-Path -Path $repo_dir -ChildPath "target\release\comms.exe"
if (-not (Test-Path -Path $comms_path)) {
    echo "$comms_path does not exist"
    exit 1
}

$package_dir = Join-Path -Path $repo_dir -ChildPath "package"
Remove-Item $package_dir -Recurse -ErrorAction Ignore
[void](mkdir $package_dir)
cp $comms_path $package_dir
cp (Join-Path -Path $msys_dir -ChildPath "mingw64\bin\gdbus.exe") $package_dir
cp (Join-Path -Path $msys_dir -ChildPath "mingw64\lib\gdk-pixbuf-2.0\2.10.0\loaders\libpixbufloader-png.dll") $package_dir

[void](New-Item (Join-Path -Path $package_dir -ChildPath "loaders.cache") -ItemType File -Value @"
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


"@)

$ldd = Join-Path -Path $msys_bin_dir -ChildPath "ldd.exe"
$cygpath = Join-Path -Path $msys_bin_dir -ChildPath "cygpath.exe"
$ldd_output = & $ldd $comms_path
$ldd_output += & $ldd (Join-Path -Path $package_dir -ChildPath "gdbus.exe")
foreach ($line in $ldd_output) {
    $dll_path_with_address = ($line.Trim() -split " => ", 2)[1]
    $dll_path_cygwin = ($dll_path_with_address.Trim() -split " ")[0]
    if (-not $dll_path_cygwin.StartsWith("/c/")) {
        $dll_path = & $cygpath -w $dll_path_cygwin
        echo "Copying $dll_path"
        cp $dll_path $package_dir
    }
}
warp-packer --arch windows-x64 --input_dir $package_dir --exec comms.exe --output comms.exe
$editbin = [Environment]::GetEnvironmentVariable('EDITBIN_PATH')
if (-not $editbin) {
    echo "Not hiding startup console window as 'EDITBIN_PATH' environment variable isn't set"
} else {
    &$editbin /subsystem:windows comms.exe
}

if (Get-Command "ResourceHacker.exe" -ErrorAction SilentlyContinue) {
    ResourceHacker.exe -open comms.exe -save comms.exe -action addskip -res comms\resources\logo.ico -mask "ICONGROUP,MAINICON,"
    # ResourceHacker forks and dumps output after, which dirties the terminal. This "sleep and echo" ensures that
    # a clean prompt is available afterwards.
    sleep 1
    echo ""
} else {
    echo "Not setting packed executable's icon as 'ResourceHacker.exe' isn't in the $PATH"
}
