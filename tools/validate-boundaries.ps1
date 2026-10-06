param(
    [string]$ConsumerRoot
)

$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$failures = [Collections.Generic.List[string]]::new()

$forbidden = @(
    'writer[_-]',
    'office_oxide',
    'pdf_oxide',
    'ironcalc',
    'slides[_-]',
    'pptx',
    'docx',
    'rudra[_-]office[_-]app'
)
$ownedFiles = Get-ChildItem -Recurse -File -LiteralPath (Join-Path $root 'crates'), (Join-Path $root 'examples') |
    Where-Object { $_.Extension -in '.rs', '.toml' }
foreach ($file in $ownedFiles) {
    $text = [IO.File]::ReadAllText($file.FullName)
    foreach ($pattern in $forbidden) {
        if ($text -match $pattern) {
            $failures.Add("Forbidden domain dependency/token '$pattern' in $($file.FullName)")
        }
    }
}

$coreManifest = [IO.File]::ReadAllText((Join-Path $root 'crates\makepad-toolkit-core\Cargo.toml'))
if ($coreManifest -match 'makepad-widgets\s*=') {
    $failures.Add('Pure core manifest must not depend on Makepad')
}

$manifest = Get-Content -Raw -LiteralPath (Join-Path $root 'skills\manifest.json') | ConvertFrom-Json
foreach ($skill in $manifest.skills) {
    $path = Join-Path (Join-Path $root 'skills') $skill.path
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        $failures.Add("Missing skill path: $path")
    }
}

if (-not [string]::IsNullOrWhiteSpace($ConsumerRoot)) {
    $consumer = [IO.Path]::GetFullPath($ConsumerRoot)
    $legacyCanvas = Join-Path $consumer 'workspace\crates\office-ui-runtime\src\virtual_canvas.rs'
    $legacyRuntime = Join-Path $consumer 'workspace\crates\rudra-runtime\src\lib.rs'
    if (Test-Path -LiteralPath $legacyCanvas) {
        $failures.Add("Duplicate consumer VirtualCanvas implementation remains: $legacyCanvas")
    }
    if (Test-Path -LiteralPath $legacyRuntime) {
        $failures.Add("Duplicate consumer rudra-runtime implementation remains: $legacyRuntime")
    }
}

if ($failures.Count -gt 0) {
    $failures | ForEach-Object { Write-Error $_ }
    exit 1
}

[PSCustomObject]@{
    status = 'ok'
    checked_files = $ownedFiles.Count
    skills = $manifest.skills.Count
    consumer_checked = -not [string]::IsNullOrWhiteSpace($ConsumerRoot)
} | ConvertTo-Json -Compress
