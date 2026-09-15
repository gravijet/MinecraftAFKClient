<#
    Baut alle acht Rust-Bauformen von AFKSystems und legt sie in dist\ ab.
    Jede Datei unterstuetzt Minecraft 1.8.9, 1.21.1, 1.21.11, 26.1 und 26.2.

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

    $variants = @(
        @{ Features = 'movement';      File = 'afk-windows-move.exe';          Label = 'Bewegung' },
        @{ Features = 'items';         File = 'items-afk-windows.exe';          Label = 'Items' },
        @{ Features = 'web-menu';      File = 'items-web-afk-windows.exe';     Label = 'Items + Browser-Menü' },
        @{ Features = 'premium';       File = 'premium-afk-windows.exe';        Label = 'Premium' },
        @{ Features = 'premium,items'; File = 'premium-items-afk-windows.exe'; Label = 'Premium + Items' },
        @{ Features = 'pov-client';    File = 'pov-afk-windows.exe';            Label = 'POV' },
        @{ Features = 'ultra';         File = 'ultra-afk-windows.exe';          Label = 'Ultra' }
    )
    foreach ($variant in $variants) {
        Invoke-Native { & cargo build --locked --release --features $variant.Features } "Rust-Build $($variant.Label)"
        Copy-Item 'target\release\afk.exe' (Join-Path $dist $variant.File) -Force
    }
} finally {
    Pop-Location
}

Write-Host ""
Write-Host "Fertig in $dist" -ForegroundColor Green
Get-ChildItem -LiteralPath $dist -File |
    Select-Object Name, @{n = 'MB'; e = { [math]::Round($_.Length / 1MB, 2) } } |
    Format-Table
