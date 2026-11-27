@echo off
rem The headset mode over SteamVR (README.md, "The VR mod"). -VRMode cinema: a flat screen; -VRMode immersive: the head turns the camera; -VREmu: no headset.
rem Keys: F6 next view, F7 centre, F8 / F9 the frame smaller / larger, Ctrl+F8 / Ctrl+F9 the world nearer / further, F10 hands or controllers.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\play.ps1" -VR %*
if errorlevel 1 pause
