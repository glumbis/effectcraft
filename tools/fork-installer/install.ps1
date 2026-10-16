# Installs EffectCraft (OpenFX plug-ins + After Effects project import) for the current user.
# Run "Install EffectCraft.bat" from the unzipped download; nothing needs administrator rights.
#
#  1. Copies the app to %LOCALAPPDATA%\Programs\EffectCraft-OFX (closes nothing: asks you to quit
#     EffectCraft first if it's running).
#  2. Adds Start menu and desktop shortcuts.
#  3. Installs the small EC Test plug-in next to the app, so OpenFX can be checked without a
#     commercial plug-in.
#  4. For opening .aep / .aepx: finds Python 3 and installs the py-aep library; if Python is
#     missing it offers to install it with winget (you're asked first).
#
# -Yes answers every question with yes (for scripted installs).

param([switch]$Yes)

$ErrorActionPreference = 'Stop'
$src = $PSScriptRoot
$dst = Join-Path $env:LOCALAPPDATA 'Programs\EffectCraft-OFX'

function Say($text, $color = 'Gray') { Write-Host $text -ForegroundColor $color }
function Ask($question) {
    if ($Yes) { return $true }
    $a = Read-Host "$question [Y/n]"
    return ($a -eq '' -or $a -match '^[yY]')
}

Say ''
Say 'EffectCraft with OpenFX plug-ins and After Effects project import' 'Cyan'
Say "Installing to $dst"
Say ''

if (-not (Test-Path (Join-Path $src 'effectcraft.exe'))) {
    Say 'effectcraft.exe is not next to this installer: unzip the whole download first, then run it again.' 'Red'
    exit 1
}

# 1. The app.
while (Get-Process effectcraft -ErrorAction SilentlyContinue) {
    if ($Yes) { Say 'EffectCraft is running: close it and run the installer again.' 'Red'; exit 1 }
    Read-Host 'EffectCraft is running. Close it, then press Enter to continue' | Out-Null
}
New-Item -ItemType Directory -Force $dst | Out-Null
Copy-Item (Join-Path $src 'effectcraft.exe'), (Join-Path $src 'effectcraft-cli.exe') $dst -Force
foreach ($f in 'README.md', 'uninstall.ps1', 'Uninstall EffectCraft.bat') {
    if (Test-Path (Join-Path $src $f)) { Copy-Item (Join-Path $src $f) $dst -Force }
}
if (Test-Path (Join-Path $src 'aep-import')) {
    New-Item -ItemType Directory -Force (Join-Path $dst 'aep-import') | Out-Null
    Copy-Item (Join-Path $src 'aep-import\*') (Join-Path $dst 'aep-import') -Recurse -Force
}
Say '[ok] App installed' 'Green'

# 2. Shortcuts.
$ws = New-Object -ComObject WScript.Shell
$links = @(
    (Join-Path ([Environment]::GetFolderPath('Programs')) 'EffectCraft (OFX).lnk'),
    (Join-Path ([Environment]::GetFolderPath('Desktop')) 'EffectCraft (OFX).lnk')
)
foreach ($l in $links) {
    $s = $ws.CreateShortcut($l)
    $s.TargetPath = Join-Path $dst 'effectcraft.exe'
    $s.WorkingDirectory = $dst
    $s.Description = 'EffectCraft with OpenFX plug-ins and After Effects project import'
    $s.Save()
}
Say '[ok] Start menu and desktop shortcuts: "EffectCraft (OFX)"' 'Green'

# 3. The test plug-in, in OFX\ next to the app (EffectCraft scans it; other OFX hosts don't).
$bundle = Join-Path $src 'OFX\ec-test-ofx.ofx.bundle'
if (-not (Test-Path $bundle)) { $bundle = Join-Path $src 'ec-test-ofx.ofx.bundle' }  # older downloads
$ofx = Join-Path $dst 'OFX'
if (Test-Path $bundle) {
    New-Item -ItemType Directory -Force $ofx | Out-Null
    # An update replaces our own bundle (no stale files from an older one) and nothing else in OFX\:
    # plug-ins the user copied there stay.
    Remove-Item -LiteralPath (Join-Path $ofx 'ec-test-ofx.ofx.bundle') -Recurse -Force -ErrorAction SilentlyContinue
    Copy-Item $bundle $ofx -Recurse -Force
    Say '[ok] Test plug-in installed ("EC Test" effects)' 'Green'
}
# Older versions put that folder on OFX_PLUGIN_PATH, where Resolve, Vegas and Natron saw it too.
$cur = [Environment]::GetEnvironmentVariable('OFX_PLUGIN_PATH', 'User')
if ($cur -and (($cur -split ';') -contains $ofx)) {
    $rest = @($cur -split ';' | Where-Object { $_ -and $_ -ne $ofx })
    [Environment]::SetEnvironmentVariable('OFX_PLUGIN_PATH', $(if ($rest.Count) { $rest -join ';' } else { $null }), 'User')
}
Say '     Your own OpenFX plug-ins (RSMB, Twixtor, Sapphire...): install their OFX versions with their own installers,'
Say '     or copy the .ofx.bundle folder into %APPDATA%\EffectCraft\OFX (no administrator rights; kept when EffectCraft is uninstalled).'

# 4. Python + py-aep for .aep / .aepx import.
function Find-Python {
    $candidates = @(@('py', '-3'), @('python3'), @('python'))
    foreach ($c in $candidates) {
        $exe = $c[0]; $pre = @($c | Select-Object -Skip 1)
        if (-not (Get-Command $exe -ErrorAction SilentlyContinue)) { continue }
        try {
            $v = & $exe @pre -c 'import sys; print(sys.version_info[0])' 2>$null
            if ($LASTEXITCODE -eq 0 -and "$v".Trim() -eq '3') { return , (@($exe) + $pre) }
        } catch { }
    }
    return $null
}

$py = Find-Python
if (-not $py) {
    Say ''
    Say 'Python 3 is needed only to open After Effects projects (.aep / .aepx).' 'Yellow'
    if ((Get-Command winget -ErrorAction SilentlyContinue) -and (Ask 'Install Python 3 now with winget?')) {
        & winget install -e --id Python.Python.3.12 --scope user --accept-package-agreements --accept-source-agreements
        # The new Python isn't on this window's PATH yet.
        $env:Path = [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + [Environment]::GetEnvironmentVariable('Path', 'Machine')
        $py = Find-Python
    }
}
if ($py) {
    $exe = $py[0]; $pre = @($py | Select-Object -Skip 1)
    Say ''
    Say 'Installing py-aep (reads After Effects projects)...'
    & $exe @pre -m pip install --user --upgrade --disable-pip-version-check py-aep
    if ($LASTEXITCODE -eq 0) {
        Say '[ok] After Effects project import ready: File > Import > After Effects Project...' 'Green'
    } else {
        Say '[!] py-aep did not install. Try later in a terminal:  py -m pip install py-aep' 'Yellow'
    }
} else {
    Say '[--] Skipped .aep import setup. Later: install Python 3 from python.org, then run:  py -m pip install py-aep' 'Yellow'
}

Say ''
Say 'Done. Start EffectCraft from the "EffectCraft (OFX)" shortcut.' 'Cyan'
Say 'Windows may warn the first time ("Windows protected your PC"): More info > Run anyway (the app is not code-signed).'
