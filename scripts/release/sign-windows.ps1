<#
.SYNOPSIS
  Authenticode-signs files with a PFX certificate (SHA-256, RFC 3161 timestamp) and verifies them.
.DESCRIPTION
  Reads WINDOWS_CERTIFICATE_PFX_BASE64 (the base64 of the .pfx) and WINDOWS_CERTIFICATE_PASSWORD
  from the environment. Without both it prints a notice and signs nothing (exit 0); with both,
  any failure fails the script. The certificate is imported into the current user's store for the
  run only (so the password never appears on a command line) and removed again, together with the
  temporary .pfx file, on the way out. Nothing secret is printed.
.PARAMETER Files
  The files to sign.
.PARAMETER TimestampUrl
  The RFC 3161 timestamp server.
#>
param(
  [Parameter(Mandatory = $true, Position = 0)] [string[]] $Files,
  [string] $TimestampUrl = 'http://timestamp.digicert.com'
)
$ErrorActionPreference = 'Stop'

if (-not $env:WINDOWS_CERTIFICATE_PFX_BASE64 -or -not $env:WINDOWS_CERTIFICATE_PASSWORD) {
  Write-Host '::notice title=Windows signing skipped::WINDOWS_CERTIFICATE_PFX_BASE64 and WINDOWS_CERTIFICATE_PASSWORD are not set; the files stay unsigned.'
  exit 0
}

function Find-SignTool {
  $cmd = Get-Command signtool.exe -ErrorAction SilentlyContinue
  if ($cmd) { return $cmd.Source }
  $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
  $found = Get-ChildItem -Path $kits -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match '\\x64\\signtool\.exe$' } |
    Sort-Object { [version]($_.FullName -replace '^.*\\bin\\(\d+\.\d+\.\d+\.\d+)\\.*$', '$1') } -Descending |
    Select-Object -First 1
  if (-not $found) { throw 'signtool.exe was not found (the Windows SDK is not installed)' }
  return $found.FullName
}

$signtool = Find-SignTool
Write-Host "signtool: $signtool"
$tempDir = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }
$pfx = Join-Path $tempDir ("atomik-signing-" + [guid]::NewGuid().ToString('N') + '.pfx')
$thumbprint = $null
try {
  [System.IO.File]::WriteAllBytes($pfx, [System.Convert]::FromBase64String($env:WINDOWS_CERTIFICATE_PFX_BASE64))
  $password = ConvertTo-SecureString -String $env:WINDOWS_CERTIFICATE_PASSWORD -AsPlainText -Force
  $cert = Import-PfxCertificate -FilePath $pfx -CertStoreLocation Cert:\CurrentUser\My -Password $password
  $thumbprint = $cert.Thumbprint
  Remove-Item -Force $pfx
  Write-Host "certificate: $($cert.Subject)"
  foreach ($file in $Files) {
    Write-Host "signing $file"
    & $signtool sign /fd SHA256 /sha1 $thumbprint /s My /tr $TimestampUrl /td SHA256 $file
    if ($LASTEXITCODE -ne 0) { throw "signtool sign failed for $file" }
    & $signtool verify /pa $file
    if ($LASTEXITCODE -ne 0) { throw "signtool verify /pa failed for $file" }
  }
} finally {
  if (Test-Path $pfx) { Remove-Item -Force $pfx }
  if ($thumbprint) { Remove-Item -Force -ErrorAction SilentlyContinue "Cert:\CurrentUser\My\$thumbprint" }
}
