param(
    [string]$Root = (Get-Location).Path
)

$resolvedRoot = (Resolve-Path -LiteralPath $Root).Path
$markdown = Get-ChildItem -LiteralPath $resolvedRoot -Recurse -File -Filter '*.md' |
    Where-Object { $_.FullName -notmatch '\\target\\|\\.deps\\' }
$pattern = '\[[^\]]*\]\((?<url>[^)\s]+)(?:\s+"[^"]*")?\)'
$broken = [System.Collections.Generic.List[object]]::new()

foreach ($file in $markdown) {
    $lineNumber = 0
    foreach ($line in Get-Content -LiteralPath $file.FullName) {
        $lineNumber++
        foreach ($match in [regex]::Matches($line, $pattern)) {
            $url = $match.Groups['url'].Value.Trim('<', '>')
            if ($url -match '^(https?://|mailto:|#|data:)') {
                continue
            }
            $relative = ($url -split '[#?]', 2)[0]
            if ([string]::IsNullOrWhiteSpace($relative)) {
                continue
            }
            $candidate = if ([System.IO.Path]::IsPathRooted($relative)) {
                $relative
            } else {
                Join-Path $file.DirectoryName $relative
            }
            if (-not (Test-Path -LiteralPath $candidate)) {
                $broken.Add([PSCustomObject]@{
                    file = $file.FullName.Substring($resolvedRoot.Length).TrimStart('\')
                    line = $lineNumber
                    link = $url
                })
            }
        }
    }
}

if ($broken.Count -gt 0) {
    $broken | Format-Table -AutoSize
    Write-Error "AI UI Markdown link audit found $($broken.Count) broken local link(s)."
    exit 1
}

Write-Output "AI_UI_LINK_AUDIT_PASSED"
Write-Output "Checked $($markdown.Count) Markdown files; no broken local links."
