@echo off
rem Сборка zip-архива модуля tokenatra-connector для Foundry VTT.
rem Корень архива = папка tokenatra-connector/ (требование Foundry).
rem Стабильное имя tokenatra-connector.zip совпадает с download-URL в module.json.

setlocal
cd /d "%~dp0"

python build_studio.py

powershell -NoProfile -ExecutionPolicy Bypass -Command ^
  "$ErrorActionPreference='Stop';" ^
  "if (Test-Path 'dist\module') { Remove-Item 'dist\module' -Recurse -Force };" ^
  "New-Item -ItemType Directory -Path 'dist\module' -Force | Out-Null;" ^
  "Compress-Archive -Path 'foundry-module\tokenatra-connector' -DestinationPath 'dist\module\tokenatra-connector.zip' -Force;" ^
  "$m = Get-Content 'foundry-module\tokenatra-connector\module.json' -Raw | ConvertFrom-Json;" ^
  "Copy-Item 'dist\module\tokenatra-connector.zip' ('dist\module\tokenatra-connector-v' + $m.version + '.zip');" ^
  "Write-Host ('OK: dist\module\tokenatra-connector.zip (v' + $m.version + ')')"

endlocal
