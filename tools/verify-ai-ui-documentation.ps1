param(
    [string]$Root = (Get-Location).Path
)

$resolvedRoot = (Resolve-Path -LiteralPath $Root).Path
$packet = @(
    'README.md',
    'docs/INDEX.md',
    'AI_UI_TEST_TOOL_QUICKSTART.md'
) | ForEach-Object {
    $path = Join-Path $resolvedRoot $_
    if (-not (Test-Path -LiteralPath $path)) {
        throw "Blind-read packet is missing: $_"
    }
    Get-Content -Raw -LiteralPath $path
}
$text = $packet -join "`n"
$topics = [ordered]@{
    'Toolkit identity' = 'Rust Makepad Toolkit'
    'Run a UI test' = 'ai-ui-test'
    'Physical drag' = 'drag_relative'
    'AI UI script' = '.aiui'
    'Generate tests' = 'generate'
    'Scheduler' = 'ai-ui-scheduler'
    'ContainerWorker scope' = 'Container Level 0'
    'Evidence location' = 'reports'
}
$missing = foreach ($topic in $topics.Keys) {
    if ($text -notmatch [regex]::Escape($topics[$topic])) {
        $topic
    }
}

if ($missing) {
    Write-Error "Documentation-only blind-read packet is missing: $($missing -join ', ')"
    exit 1
}

Write-Output 'AI_UI_DOCUMENTATION_CURRENT_PASSED'
Write-Output 'Blind-read packet covers Toolkit, UI test, physical drag, .aiui, generator, scheduler, Container Level 0 and evidence.'
