@echo off
rem Installs EffectCraft (OpenFX + After Effects import) for this user. See install.ps1.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
echo.
pause
