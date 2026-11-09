# Optional manual signing helper for a Tauri-built Windows executable/installer.
# SIGN_TOOL_PATH: Windows SDK SignTool path
# CERTIFICATE: base64-encoded Windows .pfx
# CERTIFICATE_PASSWORD: .pfx password
param(
    [Parameter(Mandatory = $true)]
    [string]$FilePath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$artifact = (Resolve-Path $FilePath).Path
$certificatePath = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName() + ".pfx")
try {
    [IO.File]::WriteAllBytes($certificatePath, [Convert]::FromBase64String($env:CERTIFICATE))
    & "$env:SIGN_TOOL_PATH" sign /tr http://timestamp.digicert.com /td SHA256 /fd SHA256 /f $certificatePath /p $env:CERTIFICATE_PASSWORD $artifact
    if ($LASTEXITCODE -ne 0) { throw "SignTool failed with exit code $LASTEXITCODE" }
} finally {
    Remove-Item $certificatePath -ErrorAction SilentlyContinue
}
