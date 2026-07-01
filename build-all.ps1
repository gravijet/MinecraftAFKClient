<#
    Baut alle vorhandenen HugoAFKClient-Varianten (je Version eine Jar) nach build\libs\.

    Gradle 8.14.3 läuft NICHT unter Java 25 – dieses Skript sucht daher automatisch ein
    JDK 21 (oder 17), um Gradle zu starten. Die fertigen Jars laufen davon unabhängig auf Java 25.

    Aufruf:  .\build-all.ps1 [-JavaHome "C:\Pfad\zum\jdk"]
#>
[CmdletBinding()]
param([string]$JavaHome)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $root

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

$jdk = Get-GradleJdk $JavaHome
if (-not $jdk) {
    Write-Host "Kein JDK 21/17 für Gradle gefunden. Bitte -JavaHome angeben." -ForegroundColor Red
    exit 1
}
$env:JAVA_HOME = $jdk
Write-Host "Gradle läuft mit JDK: $jdk" -ForegroundColor Cyan

# Zu bauende Varianten. 1.8.9 nur, wenn die Via-Bridge vorhanden ist.
$variants = @('26.1', '1.21.11')
if (Test-Path (Join-Path $root 'src\via\java\net\gravijet\afk\via\ViaProtocolBridge.java')) {
    $variants += '1.8.9'
} else {
    Write-Host "Hinweis: 1.8.9 wird übersprungen (Via-Bridge src\via\... fehlt noch)." -ForegroundColor Yellow
}

foreach ($v in $variants) {
    Write-Host "`n=== Baue Variante $v ===" -ForegroundColor Green
    & .\gradlew.bat shadowJar "-Pvariant=$v"
    if ($LASTEXITCODE -ne 0) { Write-Host "Build für $v fehlgeschlagen." -ForegroundColor Red; exit 1 }
}

Write-Host "`n=== Fertige Jars ===" -ForegroundColor Cyan
Get-ChildItem (Join-Path $root 'build\libs\hugoafk-*.jar') |
    ForEach-Object { "{0,7:N1} MB   {1}" -f ($_.Length / 1MB), $_.Name }
