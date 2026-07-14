<#
    Baut beide Module:
      java\  – Per-Version-Jars nach java\build\libs\   (Gradle)
      rust\  – hugoafk.exe nach rust\target\release\    (Cargo, nur MC 26.1)

    Gradle 8.14.3 läuft NICHT unter Java 25 – dieses Skript sucht daher automatisch ein
    JDK 21 (oder 17), um Gradle zu starten. Die fertigen Jars laufen davon unabhängig auf Java 25.

    Aufruf:  .\build-all.ps1 [-JavaHome "C:\Pfad\zum\jdk"] [-Only java|rust]
#>
[CmdletBinding()]
param(
    [string]$JavaHome,
    [ValidateSet('java', 'rust', 'both')]
    [string]$Only = 'both'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $root

# ===================== Java =====================

function Get-GradleJdk {
    param([string]$explicit)
    if ($explicit -and (Test-Path $explicit)) { return $explicit }
    # Kandidaten: bevorzugt 21, sonst 17.
    $candidates = @()
    foreach ($base in @("$env:ProgramFiles\Java", "$env:USERPROFILE\.jdks")) {
        if (Test-Path $base) {
            $candidates += Get-ChildItem $base -Directory -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -match '(^|[-_.])(21|17)([-_.]|$)' -or $_.Name -match 'jdk-?(21|17)' } |
                Select-Object -ExpandProperty FullName
        }
    }
    foreach ($c in $candidates) { if (Test-Path (Join-Path $c 'bin\java.exe')) { return $c } }
    return $null
}

function Build-Java {
    $jdk = Get-GradleJdk $JavaHome
    if (-not $jdk) {
        Write-Host "Kein JDK 21/17 für Gradle gefunden. Bitte -JavaHome angeben." -ForegroundColor Red
        exit 1
    }
    $env:JAVA_HOME = $jdk
    Write-Host "Gradle läuft mit JDK: $jdk" -ForegroundColor Cyan

    # Zu bauende Varianten. 1.8.9 nur, wenn die Via-Bridge vorhanden ist.
    $variants = @('26.1', '1.21.11')
    if (Test-Path (Join-Path $root 'java\src\via\java\net\gravijet\afk\via\ViaProtocolBridge.java')) {
        $variants += '1.8.9'
    } else {
        Write-Host "Hinweis: 1.8.9 wird übersprungen (Via-Bridge java\src\via\... fehlt noch)." -ForegroundColor Yellow
    }

    foreach ($v in $variants) {
        Write-Host "`n=== Java-Modul: Variante $v ===" -ForegroundColor Green
        & .\gradlew.bat :java:shadowJar "-Pvariant=$v"
        if ($LASTEXITCODE -ne 0) { Write-Host "Build für $v fehlgeschlagen." -ForegroundColor Red; exit 1 }
    }
}

# ===================== Rust =====================

function Build-Rust {
    Write-Host "`n=== Rust-Modul (MC 26.1) ===" -ForegroundColor Green
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Write-Host "cargo nicht gefunden. Rust installieren:  winget install Rustlang.Rustup" -ForegroundColor Red
        exit 1
    }
    # Die GNU-Toolchain braucht dlltool aus MinGW (WinLibs). Falls nicht im PATH: nachreichen.
    if (-not (Get-Command dlltool.exe -ErrorAction SilentlyContinue)) {
        $mingw = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages" -Recurse -Filter dlltool.exe -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($mingw) { $env:Path = "$($mingw.DirectoryName);$env:Path" }
    }
    Push-Location (Join-Path $root 'rust')
    try {
        & cargo build --release
        if ($LASTEXITCODE -ne 0) { Write-Host "Rust-Build fehlgeschlagen." -ForegroundColor Red; exit 1 }
    } finally {
        Pop-Location
    }
}

if ($Only -in @('java', 'both')) { Build-Java }
if ($Only -in @('rust', 'both')) { Build-Rust }

Write-Host "`n=== Ergebnisse ===" -ForegroundColor Cyan
Get-ChildItem (Join-Path $root 'java\build\libs\hugoafk-*.jar') -ErrorAction SilentlyContinue |
    ForEach-Object { "{0,7:N1} MB   java   {1}" -f ($_.Length / 1MB), $_.Name }
Get-Item (Join-Path $root 'rust\target\release\hugoafk.exe') -ErrorAction SilentlyContinue |
    ForEach-Object { "{0,7:N1} MB   rust   {1}" -f ($_.Length / 1MB), $_.Name }
