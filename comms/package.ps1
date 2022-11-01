# Expects the following environment variables to be set:
# DUMPBIN_PATH: path to the VS Build Tools "dumpbin.exe" binary

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

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

$dumpbin = [Environment]::GetEnvironmentVariable('DUMPBIN_PATH')
$global:handled_dependencies = @()
function CopyWithDependencies {
    param (
        $resource_path
    )

    echo "Copying $resource_path"
    cp $resource_path $package_dir

    $dumpbin_output = & $dumpbin /dependents $resource_path
    foreach ($line in $dumpbin_output) {
        if ((-not $line.EndsWith(".dll")) -or (-not $line.StartsWith("    "))) {
            continue
        }

        $dll_name = $line.Trim()
        if ($global:handled_dependencies.Contains($dll_name)) {
            continue
        }

        $global:handled_dependencies += $dll_name
        $ErrorActionPreference = "SilentlyContinue"
        $dll_path = where.exe $dll_name 2>$null
        $ErrorActionPreference = "Stop"

        if (($dll_path -eq $null) -or (-not $dll_path.Contains("gtk-build\gtk\x64"))) {
            continue
        }

        CopyWithDependencies $dll_path
    }
}

CopyWithDependencies $comms_path
CopyWithDependencies (where.exe gdbus.exe)

Remove-Item comms.exe -ErrorAction Ignore
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
