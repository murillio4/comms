# Expects the following environment variables to be set:
# SIGN_TOOL_PATH: path to the Windows SDK "SignTool" binary
# CERTIFICATE: Windows .pfx file encoded as base64
# CERTIFICATE_PASSWORD: password for the .pfx file

$bytes = [Convert]::FromBase64String($env:CERTIFICATE)
[IO.File]::WriteAllBytes("$PWD\certificate.pfx", $bytes)
& $env:SIGN_TOOL_PATH sign /tr http://timestamp.digicert.com /td SHA256 /fd SHA256 /f certificate.pfx /p $env:CERTIFICATE_PASSWORD comms.exe
rm .\certificate.pfx
