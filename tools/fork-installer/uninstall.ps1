# Removes EffectCraft (OpenFX) installed by install.ps1: the app folder, its shortcuts and its entry
# on OFX_PLUGIN_PATH. Your projects, settings (%APPDATA%\EffectCraft), OpenFX plug-ins, Python and
# py-aep are left alone.

param([switch]$Yes)

$dst = Join-Path $env:LOCALAPPDATA 'Programs\EffectCraft-OFX'
if (-not $Yes) {
    $a = Read-Host "Remove EffectCraft (OFX) from $dst ? [y/N]"
    if ($a -notmatch '^[yY]') { exit 0 }
}
if (Get-Process effectcraft -ErrorAction SilentlyContinue) {
    Write-Host 'EffectCraft is running: close it first.' -ForegroundColor Red
    exit 1
}
foreach ($l in @(
        (Join-Path ([Environment]::GetFolderPath('Programs')) 'EffectCraft (OFX).lnk'),
        (Join-Path ([Environment]::GetFolderPath('Desktop')) 'EffectCraft (OFX).lnk'))) {
    Remove-Item $l -ErrorAction SilentlyContinue
}
$ofx = Join-Path $dst 'OFX'
$cur = [Environment]::GetEnvironmentVariable('OFX_PLUGIN_PATH', 'User')
if ($cur) {
    $rest = @($cur -split ';' | Where-Object { $_ -and $_ -ne $ofx })
    [Environment]::SetEnvironmentVariable('OFX_PLUGIN_PATH', $(if ($rest.Count) { $rest -join ';' } else { $null }), 'User')
}
# This script may be running from inside the folder: remove the folder after it exits.
Start-Process cmd.exe -ArgumentList '/c', "timeout /t 2 >nul & rmdir /s /q `"$dst`"" -WindowStyle Hidden
Write-Host 'EffectCraft (OFX) removed.' -ForegroundColor Green
