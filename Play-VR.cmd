@echo off
rem The headset mode over SteamVR (README.md, "VR"). Play-VR.cmd -VRMode cinema for a flat screen; -VREmu with no headset.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\play.ps1" -VR %*
if errorlevel 1 pause
