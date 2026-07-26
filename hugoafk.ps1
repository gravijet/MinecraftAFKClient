<#
    HugoAFKClient – Launcher (Windows / PowerShell)

    Zeigt ein Menü über beide Module:
      * Rust-Client (MC 26.1)  – nativ, ~1 MB RAM, startet sofort
      * Java-Clients           – ein Jar je Minecraft-Version, mit sparsamen JVM-Argumenten

    Aufruf:   .\hugoafk.ps1 [server[:port]]
#>
[CmdletBinding()]
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$PassThru
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$libs = Join-Path $root 'java\build\libs'
$rustExe = Join-Path $root 'rust\target\release\hugoafk.exe'

# Reihenfolge = Anzeigereihenfolge im Menü.
$javaVersions = @('1.21.11', '26.1', '1.8.9')

# Prüft, ob die aktuelle JVM eine (experimentelle) Option akzeptiert.
function Test-JvmFlag([string[]]$flags) {
    try {
        & java @flags '-version' *> $null
        return ($LASTEXITCODE -eq 0)
    } catch { return $false }
}

# Verfügbare Einträge sammeln: erst Rust, dann die gebauten Jars.
$entries = @()
if (Test-Path $rustExe) {
    $entries += [pscustomobject]@{ Kind = 'rust'; Version = '26.1'; Label = 'Minecraft 26.1   (Rust – nativ, ~1 MB RAM)' }
}
foreach ($v in $javaVersions) {
    $jar = Join-Path $libs "hugoafk-$v.jar"
    if (Test-Path $jar) {
        $entries += [pscustomobject]@{ Kind = 'java'; Version = $v; Label = "Minecraft $v   (Java)"; Jar = $jar }
    }
}

Write-Host ""
Write-Host "  +---------------------------------------------+" -ForegroundColor Cyan
Write-Host "  |  HugoAFKClient   -   Client wählen           |" -ForegroundColor Cyan
Write-Host "  +---------------------------------------------+" -ForegroundColor Cyan

if ($entries.Count -eq 0) {
    Write-Host "  Nichts gebaut gefunden." -ForegroundColor Red
    Write-Host "  Baue zuerst mit:  .\build-all.ps1" -ForegroundColor Yellow
    exit 1
}

for ($i = 0; $i -lt $entries.Count; $i++) {
    Write-Host ("   {0}) {1}" -f ($i + 1), $entries[$i].Label)
}
Write-Host "   q) Beenden"
Write-Host ""

$choice = Read-Host "  Auswahl"
if ($choice -eq 'q') { exit 0 }
$idx = 0
if (-not [int]::TryParse($choice, [ref]$idx) -or $idx -lt 1 -or $idx -gt $entries.Count) {
    Write-Host "  Ungültige Auswahl." -ForegroundColor Red
    exit 1
}
$entry = $entries[$idx - 1]

# ---- Rust: einfach starten, es gibt nichts zu tunen ----
if ($entry.Kind -eq 'rust') {
    Write-Host ""
    Write-Host "  Starte HugoAFKClient (Rust, MC 26.1) ..." -ForegroundColor Green
    Write-Host ""
    & $rustExe @PassThru
    exit $LASTEXITCODE
}

# ---- Java: JVM-Argumente für kleinen Fußabdruck (1 Verbindung, Ryzen 9950X3D) ----
if (-not (Get-Command java -ErrorAction SilentlyContinue)) {
    Write-Host "Java wurde nicht gefunden. Bitte ein JDK (21+) installieren." -ForegroundColor Red
    exit 1
}

$version = $entry.Version
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
Write-Host "  Starte HugoAFKClient (Java, MC $version) ..." -ForegroundColor Green
Write-Host ""

& java @jvm '-jar' $entry.Jar @PassThru
exit $LASTEXITCODE
