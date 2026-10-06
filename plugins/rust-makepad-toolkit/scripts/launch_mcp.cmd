@echo off
setlocal

set "TOOLKIT_ROOT=%RUST_MAKEPAD_TOOLKIT_ROOT%"
if not defined TOOLKIT_ROOT for %%I in ("%~dp0..\..\..") do set "TOOLKIT_ROOT=%%~fI"

if not exist "%TOOLKIT_ROOT%\Cargo.toml" (
    echo Rust_Makepad_Toolkit workspace was not found at "%TOOLKIT_ROOT%". 1>&2
    echo Set RUST_MAKEPAD_TOOLKIT_ROOT to your Toolkit checkout in the local MCP server environment. 1>&2
    exit /b 2
)

if not exist "%TOOLKIT_ROOT%\crates\toolkit-windows-automation\src\bin\toolkit-windows-gui-mcp.rs" (
    echo Existing toolkit-windows-gui-mcp binary source was not found. 1>&2
    exit /b 3
)

where cargo.exe >nul 2>&1
if errorlevel 1 (
    echo Rust Cargo was not found on PATH. Install the Rust toolchain before starting this MCP server. 1>&2
    exit /b 9009
)

cargo run --quiet --offline --locked --manifest-path "%TOOLKIT_ROOT%\Cargo.toml" --package toolkit-windows-automation --bin toolkit-windows-gui-mcp
exit /b %ERRORLEVEL%
