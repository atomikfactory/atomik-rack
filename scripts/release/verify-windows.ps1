<#
.SYNOPSIS
  Checks that every given file has a valid Authenticode signature (signtool verify /pa).
.PARAMETER Files
  The files to check.
#>
param([Parameter(Mandatory = $true, Position = 0)] [string[]] $Files)
$ErrorActionPreference = 'Stop'

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
foreach ($file in $Files) {
  & $signtool verify /pa $file
  if ($LASTEXITCODE -ne 0) { throw "signtool verify /pa failed for $file" }
}
Write-Host "signatures ok: $($Files.Count) file(s)"
