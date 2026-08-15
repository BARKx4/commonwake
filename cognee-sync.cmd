@echo off
setlocal
set "SCRIPT_DIR=%~dp0"
set "COGNEE_ROOT=%SCRIPT_DIR%cognee-local"
set "COGNEE_MANIFEST_PATH=%COGNEE_ROOT%\corpora.json"
set "COGNEE_CLI_PATH=E:\LLM Projects\Memory Stack\.venv-cognee\Scripts\cognee-cli.exe"
if not exist "E:\LLM Projects\Memory Stack\.venv-hybrid\Scripts\python.exe" (
  echo Shared sync Python not found at "E:\LLM Projects\Memory Stack\.venv-hybrid\Scripts\python.exe".
  exit /b 1
)
if not exist "E:\LLM Projects\Memory Stack\scripts\cognee_corpus_sync.py" (
  echo Shared sync script not found at "E:\LLM Projects\Memory Stack\scripts\cognee_corpus_sync.py".
  exit /b 1
)
"E:\LLM Projects\Memory Stack\.venv-hybrid\Scripts\python.exe" "E:\LLM Projects\Memory Stack\scripts\cognee_corpus_sync.py" %*
