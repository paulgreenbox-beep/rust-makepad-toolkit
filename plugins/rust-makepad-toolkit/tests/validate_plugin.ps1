param([string]$ToolkitRoot)

$ErrorActionPreference = 'Stop'

$pluginRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$manifestPath = Join-Path $pluginRoot '.codex-plugin\plugin.json'
$mcpPath = Join-Path $pluginRoot '.mcp.json'
$launcherPath = Join-Path $pluginRoot 'scripts\launch_mcp.cmd'
$skillPath = Join-Path $pluginRoot 'skills\rust-makepad-windows-gui\SKILL.md'

$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$mcp = Get-Content -LiteralPath $mcpPath -Raw | ConvertFrom-Json
$server = $mcp.mcpServers.'rust-makepad-toolkit-windows-gui'
$skill = Get-Content -LiteralPath $skillPath -Raw

if ($manifest.name -ne 'rust-makepad-toolkit') { throw 'Unexpected plugin name.' }
if ([string]$manifest.version -notmatch '^\d+\.\d+\.\d+([+-][0-9A-Za-z.-]+)?$') { throw 'Plugin version must be semantic versioning.' }
if ([string]::IsNullOrWhiteSpace([string]$manifest.author.name)) { throw 'Plugin author.name is required.' }
if ($manifest.mcpServers -ne './.mcp.json') { throw 'Manifest does not reference the local MCP config.' }
if ($manifest.skills -ne './skills/') { throw 'Manifest does not reference the local skills directory.' }
foreach ($field in @('displayName', 'shortDescription', 'longDescription', 'developerName', 'category')) {
    if ([string]::IsNullOrWhiteSpace([string]$manifest.interface.$field)) { throw "Plugin interface.$field is required." }
}
if (@($manifest.interface.capabilities).Count -eq 0) { throw 'Plugin capabilities must describe implemented features.' }
if (@($manifest.interface.defaultPrompt).Count -eq 0) { throw 'At least one interface default prompt is required.' }
if (-not $server) { throw 'Windows GUI MCP server entry is missing.' }
if ($server.command -ne 'cmd.exe') { throw 'MCP server must launch through cmd.exe on Windows.' }
if ([IO.Path]::IsPathRooted([string]$server.cwd)) { throw 'MCP cwd must remain relative to the installed plugin directory.' }
if ([IO.Path]::GetFullPath((Join-Path $pluginRoot ([string]$server.cwd))).TrimEnd([char[]]@('\', '/')) -ne $pluginRoot) { throw 'MCP cwd must resolve to the plugin directory.' }
if (-not ($server.args -contains './scripts/launch_mcp.cmd')) { throw 'MCP config does not call the plugin launcher.' }
if ($server.default_tools_approval_mode -ne 'prompt') { throw 'GUI tools must keep prompt approval as the default.' }
if ([string]::IsNullOrWhiteSpace($ToolkitRoot)) { $ToolkitRoot = [string]$server.env.RUST_MAKEPAD_TOOLKIT_ROOT }
if ([string]::IsNullOrWhiteSpace($ToolkitRoot)) { $ToolkitRoot = $env:RUST_MAKEPAD_TOOLKIT_ROOT }
if ([string]::IsNullOrWhiteSpace($ToolkitRoot)) { $ToolkitRoot = Join-Path $pluginRoot '..\..' }
$workspaceRoot = [IO.Path]::GetFullPath($ToolkitRoot)
$crateManifestPath = Join-Path $workspaceRoot 'crates\toolkit-windows-automation\Cargo.toml'

$binarySource = Join-Path $workspaceRoot 'crates\toolkit-windows-automation\src\bin\toolkit-windows-gui-mcp.rs'
$cargoManifest = Join-Path $workspaceRoot 'Cargo.toml'
$crateManifest = Get-Content -LiteralPath $crateManifestPath -Raw
if (-not (Test-Path -LiteralPath $launcherPath -PathType Leaf)) { throw 'Plugin launcher is missing.' }
if (-not (Test-Path -LiteralPath $skillPath -PathType Leaf)) { throw 'Toolkit skill is missing.' }
if (-not (Test-Path -LiteralPath $binarySource -PathType Leaf)) { throw 'Existing MCP binary source is missing.' }
if (-not (Test-Path -LiteralPath $cargoManifest -PathType Leaf)) { throw 'Toolkit workspace Cargo.toml is missing.' }
if ($crateManifest -notmatch '(?m)^name\s*=\s*"toolkit-windows-automation"\s*$') { throw 'Existing MCP Cargo package name changed.' }
foreach ($targetText in @('name = "toolkit-windows-gui-mcp"', 'path = "src/bin/toolkit-windows-gui-mcp.rs"')) {
    if (-not $crateManifest.Contains($targetText)) { throw "Existing MCP Cargo target is missing: $targetText" }
}
if ((Test-Path -LiteralPath (Join-Path $pluginRoot 'Cargo.toml')) -or (Test-Path -LiteralPath (Join-Path $pluginRoot 'src'))) {
    throw 'Plugin must reference the existing backend instead of copying its source.'
}

$launcher = Get-Content -LiteralPath $launcherPath -Raw
foreach ($requiredText in @('--package toolkit-windows-automation', '--bin toolkit-windows-gui-mcp', '--locked', '--offline')) {
    if (-not $launcher.Contains($requiredText)) { throw "Launcher is missing expected Cargo argument: $requiredText" }
}
if (-not $launcher.Contains('RUST_MAKEPAD_TOOLKIT_ROOT')) { throw 'Launcher does not use the configured Toolkit checkout path.' }

$frontMatter = [regex]::Match($skill, '(?s)\A---\r?\n(?<front>.*?)\r?\n---\r?\n')
if (-not $frontMatter.Success) { throw 'Skill frontmatter is missing or malformed.' }
if ($frontMatter.Groups['front'].Value -notmatch '(?m)^name:\s*rust-makepad-windows-gui\s*$') { throw 'Skill name does not match its folder.' }
if ($frontMatter.Groups['front'].Value -notmatch '(?m)^description:\s*\S') { throw 'Skill description is required.' }
foreach ($requiredText in @('windows_find_window', 'windows_capture_window', 'windows_get_evidence', 'programmatic', 'project-level `rust-makepad-toolkit`')) {
    if (-not $skill.Contains($requiredText)) { throw "Skill is missing required Toolkit routing text: $requiredText" }
}

Write-Output 'PASS: plugin manifest, MCP launcher, existing target, and Toolkit skill references are consistent.'
Write-Output 'PASS: validation was static; no MCP server or physical GUI action was launched.'
