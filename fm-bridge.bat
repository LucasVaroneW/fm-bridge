@echo off
setlocal
set SCRIPT_DIR=%~dp0
"%SCRIPT_DIR%target\debug\fm-bridge.exe" %*
