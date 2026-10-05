@echo off
rem "termwall ..." in any terminal: the installer puts this folder on PATH.
rem Its own Python when installed, the one on PATH in a checkout.
if exist "%~dp0python\python.exe" (
  "%~dp0python\python.exe" "%~dp0termwall_api.py" %*
) else (
  python "%~dp0termwall_api.py" %*
)
