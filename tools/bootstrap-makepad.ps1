param(
    [string]$MakepadPath,
    [ValidatePattern('^[0-9a-fA-F]{40}$')]
    [string]$Revision = '105158e08c89d47f57af4621420ed68eb3a147ce'
)

$ErrorActionPreference = 'Stop'
$toolkitRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$deps = Join-Path $toolkitRoot '.deps'
$link = Join-Path $deps 'makepad'
$widgetsManifest = Join-Path $link 'widgets/Cargo.toml'

if (Test-Path -LiteralPath $widgetsManifest -PathType Leaf) {
    if (-not [string]::IsNullOrWhiteSpace($MakepadPath)) {
        $source = [IO.Path]::GetFullPath($MakepadPath).TrimEnd([char[]]@('\', '/'))
        $item = Get-Item -LiteralPath $link
        $existing = if ($item.Target) { [IO.Path]::GetFullPath([string]$item.Target) } else { $item.FullName }
        if ($existing.TrimEnd([char[]]@('\', '/')) -ne $source) {
            throw 'Existing Makepad dependency points elsewhere; it was left unchanged.'
        }
    }
    Write-Output 'Existing operator-managed Makepad dependency retained.'
    Write-Output 'A vendored directory without its own Git metadata does not prove a revision.'
    exit 0
}

if (Test-Path -LiteralPath $link) {
    throw 'Incomplete Makepad dependency directory exists; inspect it before retrying.'
}

New-Item -ItemType Directory -Force -Path $deps | Out-Null
if (-not [string]::IsNullOrWhiteSpace($MakepadPath)) {
    $source = [IO.Path]::GetFullPath($MakepadPath)
    if (-not (Test-Path -LiteralPath (Join-Path $source 'widgets/Cargo.toml') -PathType Leaf)) {
        throw 'The supplied checkout has no Makepad widgets manifest.'
    }
    $linkType = if ($IsWindows -or $env:OS -eq 'Windows_NT') { 'Junction' } else { 'SymbolicLink' }
    New-Item -ItemType $linkType -Path $link -Target $source | Out-Null
    Write-Output 'Operator-selected Makepad checkout linked.'
    exit 0
}

& git clone --depth 1 --no-checkout https://github.com/makepad/makepad.git $link
if ($LASTEXITCODE -ne 0) { throw 'Public Makepad clone failed.' }
$actualRoot = & git -C $link rev-parse --show-toplevel
if ($LASTEXITCODE -ne 0 -or [IO.Path]::GetFullPath($actualRoot).TrimEnd([char[]]@('\', '/')) -ne [IO.Path]::GetFullPath($link)) {
    throw 'Dependency Git root does not match the intended checkout.'
}
& git -C $link fetch --depth 1 origin $Revision
if ($LASTEXITCODE -ne 0) { throw 'Public Makepad revision fetch failed.' }
& git -C $link checkout --detach $Revision
if ($LASTEXITCODE -ne 0) { throw 'Public Makepad revision checkout failed.' }
$actualRevision = & git -C $link rev-parse HEAD
if ($LASTEXITCODE -ne 0 -or $actualRevision -ne $Revision) { throw 'Makepad revision verification failed.' }
if (-not (Test-Path -LiteralPath $widgetsManifest -PathType Leaf)) { throw 'Pinned revision has no widgets manifest.' }
Write-Output "Public Makepad dependency ready at revision $actualRevision"
