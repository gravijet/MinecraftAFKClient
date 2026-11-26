<#
    Baut beide Bauformen von AFKSystems und legt sie in dist\ ab:
    afk-windows.exe (Standard) und pov-afk-windows.exe (mit Live-Ansicht).
    Jede Datei spricht Minecraft 1.8.9, 1.21.1, 1.21.11, 26.1, 26.2, 26.3 und 26.4.

    Aufruf:  .\build-all.ps1
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $root

# Windows PowerShell wertet jede Zeile auf stderr als Fehler, obwohl Cargo dort normalen
# Fortschritt schreibt. Deshalb entscheidet allein der Rueckgabewert des Prozesses.
function Invoke-Native {
    param([scriptblock]$Block, [string]$What)
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & $Block } finally { $ErrorActionPreference = $previous }
    if ($LASTEXITCODE -ne 0) { throw "$What fehlgeschlagen." }
}

$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force -Path $dist | Out-Null
# Alte JARs aus frueheren Releases gehoeren nicht mehr in die Rust-only-Ausgabe.
Get-ChildItem -LiteralPath $dist -Filter '*.jar' -File -ErrorAction SilentlyContinue |
    Remove-Item -Force

Write-Host "Rust-Build ..." -ForegroundColor Cyan
Push-Location "$root\rust"
try {
    Invoke-Native { & cargo build --locked --release } "Rust-Build"
    Copy-Item 'target\release\afk.exe' (Join-Path $dist 'afk-windows.exe') -Force

    Invoke-Native { & cargo build --locked --release --features pov } "Rust-Build POV"
    Copy-Item 'target\release\afk.exe' (Join-Path $dist 'pov-afk-windows.exe') -Force
} finally {
    Pop-Location
}

Write-Host ""
Write-Host "Fertig in $dist" -ForegroundColor Green
Get-ChildItem -LiteralPath $dist -File |
    Select-Object Name, @{n = 'MB'; e = { [math]::Round($_.Length / 1MB, 2) } } |
    Format-Table
