@echo off
setlocal
set "SCRIPT_DIR=%~dp0"
set "COGNEE_ROOT=%SCRIPT_DIR%cognee-local"
if not exist "E:\LLM Projects\Memory Stack\.venv-cognee\Scripts\cognee-cli.exe" (
  echo Shared Cognee runtime not found at "E:\LLM Projects\Memory Stack\.venv-cognee\Scripts\cognee-cli.exe".
  exit /b 1
)
cd /d "%COGNEE_ROOT%"
set "SYSTEM_ROOT_DIRECTORY=%COGNEE_ROOT%\.cognee_system"
set "DATA_ROOT_DIRECTORY=%COGNEE_ROOT%\.data_storage"
set "CACHE_ROOT_DIRECTORY=%COGNEE_ROOT%\.cognee_cache"
set "COGNEE_LOGS_DIR=%COGNEE_ROOT%\logs"
set "COGNEE_SKIP_CONNECTION_TEST=true"
"E:\LLM Projects\Memory Stack\.venv-cognee\Scripts\cognee-cli.exe" %*
