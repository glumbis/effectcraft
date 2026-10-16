# Removes EffectCraft (OpenFX) installed by install.ps1: the app folder, its shortcuts and its entry
# on OFX_PLUGIN_PATH. Your projects, settings (%APPDATA%\EffectCraft), OpenFX plug-ins, Python and
# py-aep are left alone. Of the app folder's OFX\ folder only the bundled test plug-in
# (ec-test-ofx.ofx.bundle) goes: when other plug-ins are in it, it stays and the script says where it
# is. (Plug-ins in %APPDATA%\EffectCraft\OFX, the per-user folder, are never touched.)

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
# The bundled test plug-in goes; everything else in OFX\ was put there by the user and stays.
Remove-Item -LiteralPath (Join-Path $ofx 'ec-test-ofx.ofx.bundle') -Recurse -Force -ErrorAction SilentlyContinue
$kept = @()
if (Test-Path -LiteralPath $ofx) { $kept = @(Get-ChildItem -LiteralPath $ofx -Force -ErrorAction SilentlyContinue | Where-Object { $_.Name -notin 'desktop.ini', 'Thumbs.db' }) }
if ($kept.Count -eq 0) {
    # This script may be running from inside the folder: remove the folder after it exits.
    Start-Process cmd.exe -ArgumentList '/c', "timeout /t 2 >nul & rmdir /s /q `"$dst`"" -WindowStyle Hidden
    Write-Host 'EffectCraft (OFX) removed.' -ForegroundColor Green
} else {
    # Everything but OFX\ goes now, except the two files that are running at this moment (this
    # script and its .bat); they follow once this script has exited.
    foreach ($item in @(Get-ChildItem -LiteralPath $dst -Force -ErrorAction SilentlyContinue)) {
        if ($item.Name -in 'OFX', 'uninstall.ps1', 'Uninstall EffectCraft.bat') { continue }
        Remove-Item -LiteralPath $item.FullName -Recurse -Force -ErrorAction SilentlyContinue
    }
    Start-Process cmd.exe -ArgumentList '/c', "timeout /t 2 >nul & del /q `"$dst\uninstall.ps1`" `"$dst\Uninstall EffectCraft.bat`"" -WindowStyle Hidden
    Write-Host 'EffectCraft (OFX) removed, except the OpenFX plug-ins you added:' -ForegroundColor Green
    Write-Host "  $ofx" -ForegroundColor Yellow
    foreach ($k in $kept) { Write-Host "    $($k.Name)" -ForegroundColor Yellow }
    Write-Host '  Delete that folder yourself if you do not need them.' -ForegroundColor Yellow
}
