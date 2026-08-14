<#
    Baut AFKSystems lokal und legt alles fertig benannt in dist\ ab:

      dist\afk-1.21.1.jar   dist\afk-1.21.11.jar   dist\afk-26.1.jar   dist\afk-26.2.jar
      dist\afk-windows.exe                     (Rust, alle vier Versionen in einer Datei)

    Mit -Move zusätzlich die Bewegungs-Bauform (afk-<ver>-move.jar bzw. afk-windows-move.exe).
    Mit -Premium zusätzlich den Premium-Client (dist\premium-afk-windows.exe) – nur Rust.

    Gradle läuft NICHT unter Java 25 – das Skript sucht daher automatisch ein JDK 21 (oder 17).
    Die fertigen Jars laufen davon unabhängig auf jedem Java ab 21.

    Aufruf:  .\build-all.ps1 [-Only java|rust|both] [-Move] [-Premium] [-JavaHome "C:\Pfad\zum\jdk"]
#>
[CmdletBinding()]
param(
    [ValidateSet('java', 'rust', 'both')]
    [string]$Only = 'both',
    [switch]$Move,
    [switch]$Premium,
    [string]$JavaHome
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $root

# Windows PowerShell wertet JEDE Zeile auf der Fehlerausgabe eines externen Programms als Fehler –
# cargo und gradle schreiben dort aber ihren normalen Fortschritt hin. Deshalb laufen externe
# Aufrufe hier durch diesen Helfer, der allein den Rueckgabewert zaehlt.
function Invoke-Native {
    param([scriptblock]$Block, [string]$What)
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & $Block } finally { $ErrorActionPreference = $previous }
    if ($LASTEXITCODE -ne 0) { throw "$What fehlgeschlagen." }
}

$versions = @('1.21.1', '1.21.11', '26.1', '26.2')
$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force -Path $dist | Out-Null

# ===================== Java =====================

function Get-GradleJdk {
    param([string]$explicit)
    if ($explicit -and (Test-Path $explicit)) { return $explicit }
    foreach ($wanted in @('21', '17')) {
        foreach ($base in @("$env:ProgramFiles\Java", "$env:USERPROFILE\.jdks")) {
            if (-not (Test-Path $base)) { continue }
            $hit = Get-ChildItem $base -Directory -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -match "-?$wanted(\.|$)" -and (Test-Path (Join-Path $_.FullName 'bin\javac.exe')) } |
                Select-Object -First 1
            if ($hit) { return $hit.FullName }
        }
    }
    throw "Kein JDK 21 gefunden. Mit -JavaHome den Pfad angeben."
}

if ($Only -in @('java', 'both')) {
    $env:JAVA_HOME = Get-GradleJdk $JavaHome
    Write-Host "Java-Build mit $env:JAVA_HOME" -ForegroundColor Cyan
    foreach ($v in $versions) {
        Write-Host "  afk-$v.jar ..." -ForegroundColor Gray
        Invoke-Native { & "$root\gradlew.bat" :java:shadowJar "-Pmc=$v" --console=plain -q } "Java-Build fuer $v"
        if ($Move) {
            Invoke-Native { & "$root\gradlew.bat" :java:shadowJar "-Pmc=$v" '-Pmove=true' --console=plain -q } "Bewegungs-Build fuer $v"
        }
    }
    Copy-Item "$root\java\build\libs\afk-*.jar" $dist -Force
}

# ===================== Rust =====================

if ($Only -in @('rust', 'both')) {
    Write-Host "Rust-Build ..." -ForegroundColor Cyan
    Push-Location "$root\rust"
    try {
        Invoke-Native { & cargo build --release } "Rust-Build"
        Copy-Item 'target\release\afk.exe' (Join-Path $dist 'afk-windows.exe') -Force

        if ($Move) {
            # Eigenes Zielverzeichnis, sonst ueberschreibt die Bewegungsvariante die schlanke Datei.
            Invoke-Native { & cargo build --release --features movement --target-dir target\movement } "Rust-Bewegungs-Build"
            Copy-Item 'target\movement\release\afk.exe' (Join-Path $dist 'afk-windows-move.exe') -Force
        }

        if ($Premium) {
            # Ebenfalls eigenes Zielverzeichnis: premium enthaelt movement, ist aber eine dritte Datei.
            Invoke-Native { & cargo build --release --features premium --target-dir target\premium } "Rust-Premium-Build"
            Copy-Item 'target\premium\release\afk.exe' (Join-Path $dist 'premium-afk-windows.exe') -Force
        }
    } finally {
        Pop-Location
    }
}

Write-Host ""
Write-Host "Fertig in $dist" -ForegroundColor Green
Get-ChildItem $dist | Select-Object Name, @{n = 'MB'; e = { [math]::Round($_.Length / 1MB, 2) } } | Format-Table
