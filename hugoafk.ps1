<#
    HugoAFKClient – Launcher (Windows / PowerShell)

    Zeigt ein Versionsmenü, wählt das passende Per-Version-Jar und startet es mit
    ressourcensparenden JVM-Argumenten (abgestimmt auf AMD Ryzen 9950X3D + 32 GB DDR5).

    Aufruf:   .\hugoafk.ps1 [server[:port]]
#>
[CmdletBinding()]
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$PassThru
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$libs = Join-Path $root 'build\libs'

# Reihenfolge = Anzeigereihenfolge im Menü.
$versions = @('1.21.11', '26.1', '1.8.9')

function Find-Jar($ver) {
    $p = Join-Path $libs "hugoafk-$ver.jar"
    if (Test-Path $p) { return $p } else { return $null }
}

# Prüft, ob die aktuelle JVM eine (experimentelle) Option akzeptiert.
function Test-JvmFlag([string[]]$flags) {
    try {
        & java @flags '-version' *> $null
        return ($LASTEXITCODE -eq 0)
    } catch { return $false }
}

if (-not (Get-Command java -ErrorAction SilentlyContinue)) {
    Write-Host "Java wurde nicht gefunden. Bitte ein JDK (21+) installieren." -ForegroundColor Red
    exit 1
}

# Verfügbare Versionen ermitteln.
$available = @()
foreach ($v in $versions) { if (Find-Jar $v) { $available += $v } }

Write-Host ""
Write-Host "  +---------------------------------------------+" -ForegroundColor Cyan
Write-Host "  |  HugoAFKClient   -   Version wählen          |" -ForegroundColor Cyan
Write-Host "  +---------------------------------------------+" -ForegroundColor Cyan

if ($available.Count -eq 0) {
    Write-Host "  Keine gebauten Jars in $libs gefunden." -ForegroundColor Red
    Write-Host "  Baue sie zuerst mit:  .\build-all.ps1" -ForegroundColor Yellow
    exit 1
}

for ($i = 0; $i -lt $available.Count; $i++) {
    Write-Host ("   {0}) Minecraft {1}" -f ($i + 1), $available[$i])
}
Write-Host "   q) Beenden"
Write-Host ""

$choice = Read-Host "  Auswahl"
if ($choice -eq 'q') { exit 0 }
$idx = 0
if (-not [int]::TryParse($choice, [ref]$idx) -or $idx -lt 1 -or $idx -gt $available.Count) {
    Write-Host "  Ungültige Auswahl." -ForegroundColor Red
    exit 1
}
$version = $available[$idx - 1]
$jar = Find-Jar $version

# ---- JVM-Argumente (kleiner Fußabdruck, 1 Verbindung, Ryzen 9950X3D) ----
# SerialGC = 1 GC-Thread; Netty auf 1 Event-Loop-Thread; winziger Heap.
$heap = if ($version -eq '1.8.9') { @('-Xms32m', '-Xmx320m', '-XX:MaxDirectMemorySize=64m') }
        else { @('-Xms16m', '-Xmx96m', '-XX:MaxDirectMemorySize=32m') }

$jvm = @(
    $heap
    '-XX:+UseSerialGC'
    '-XX:TieredStopAtLevel=1'
    '-Xss512k'
    '-Dio.netty.eventLoopThreads=1'
    '-Dio.netty.allocator.type=unpooled'
    '-Dio.netty.allocator.numHeapArenas=1'
    '-Dio.netty.allocator.numDirectArenas=1'
    '-Dio.netty.leakDetection.level=disabled'
    '-Dfile.encoding=UTF-8'
    "-Dhugoafk.variant=$version"
)

# Nur zuschalten, wenn die JVM es unterstützt (JDK 24+): kompakte Objekt-Header sparen RAM.
if (Test-JvmFlag @('-XX:+UnlockExperimentalVMOptions', '-XX:+UseCompactObjectHeaders')) {
    $jvm = @('-XX:+UnlockExperimentalVMOptions', '-XX:+UseCompactObjectHeaders') + $jvm
}

Write-Host ""
Write-Host "  Starte HugoAFKClient (MC $version) ..." -ForegroundColor Green
Write-Host ""

& java @jvm '-jar' $jar @PassThru
exit $LASTEXITCODE
