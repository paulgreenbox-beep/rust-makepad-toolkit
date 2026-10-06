[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Seal', 'Verify', 'Restore')]
    [string] $Action,
    [string] $ProjectRoot = (Get-Location).Path,
    [Parameter(Mandatory = $true)]
    [string] $ArchiveRoot,
    [string] $SelectionPath,
    [string] $ContainerId,
    [switch] $Evict,
    [string] $RestoreRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Get-NormalizedDirectory {
    param([Parameter(Mandatory = $true)][string] $Path, [switch] $Create)
    if ($Create -and -not (Test-Path -LiteralPath $Path)) {
        [System.IO.Directory]::CreateDirectory($Path) | Out-Null
    }
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        throw "Directory does not exist: $Path"
    }
    return [System.IO.Path]::GetFullPath((Resolve-Path -LiteralPath $Path).Path).TrimEnd('\', '/')
}

function Resolve-UnderRoot {
    param([Parameter(Mandatory = $true)][string] $Root, [Parameter(Mandatory = $true)][string] $RelativePath)
    $relative = $RelativePath.Replace('/', '\')
    if ([string]::IsNullOrWhiteSpace($relative) -or [System.IO.Path]::IsPathRooted($relative) -or ($relative.Split('\') -contains '..')) {
        throw "Unsafe relative path: $RelativePath"
    }
    $full = [System.IO.Path]::GetFullPath([System.IO.Path]::Combine($Root, $relative))
    $prefix = $Root + [System.IO.Path]::DirectorySeparatorChar
    if (-not $full.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Path escapes root: $RelativePath"
    }
    return $full
}

function Get-Sha256 {
    param([Parameter(Mandatory = $true)][string] $Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Read-JsonFile {
    param([Parameter(Mandatory = $true)][string] $Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "JSON file does not exist: $Path" }
    return Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
}

function Write-JsonFile {
    param([Parameter(Mandatory = $true)][string] $Path, [Parameter(Mandatory = $true)] $Value)
    [System.IO.Directory]::CreateDirectory((Split-Path -Parent $Path)) | Out-Null
    $temporary = "$Path.partial-$([guid]::NewGuid().ToString('N'))"
    $json = $Value | ConvertTo-Json -Depth 16
    [System.IO.File]::WriteAllText($temporary, $json + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))
    Move-Item -LiteralPath $temporary -Destination $Path -Force
}

function Get-ContainerPath {
    param([Parameter(Mandatory = $true)][string] $Archive, [Parameter(Mandatory = $true)][string] $Id)
    if ($Id -notmatch '^[a-z0-9][a-z0-9._-]{2,127}$') { throw "Unsafe container id: $Id" }
    return Resolve-UnderRoot -Root $Archive -RelativePath "containers/$Id.json"
}

function Get-Selection {
    param([Parameter(Mandatory = $true)][string] $SelectionFile, [Parameter(Mandatory = $true)][string] $Project)
    $selection = Read-JsonFile $SelectionFile
    if ($selection.version -ne 1) { throw 'Unsupported selection manifest version.' }
    $id = [string]$selection.container_id
    if ($id -notmatch '^[a-z0-9][a-z0-9._-]{2,127}$') { throw "Unsafe container id: $id" }
    $paths = @($selection.paths)
    if ($paths.Count -eq 0) { throw 'Selection manifest contains no paths.' }

    $seen = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
    $roots = [System.Collections.Generic.List[object]]::new()
    foreach ($entry in $paths) {
        $relative = [string]$entry
        $full = Resolve-UnderRoot -Root $Project -RelativePath $relative
        if (-not (Test-Path -LiteralPath $full -PathType Container)) { throw "Selected source directory does not exist: $relative" }
        $item = Get-Item -LiteralPath $full
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Selected source is a reparse point: $relative" }
        $normalized = $relative.Trim('\', '/').Replace('\', '/')
        if (-not $seen.Add($normalized)) { throw "Duplicate selected path: $relative" }
        $roots.Add([pscustomobject]@{ relative_path = $normalized; full_path = $full })
    }
    foreach ($left in $roots) {
        foreach ($right in $roots) {
            if ($left.relative_path -ne $right.relative_path -and $right.relative_path.StartsWith($left.relative_path + '/', [System.StringComparison]::OrdinalIgnoreCase)) {
                throw "Overlapping selected paths: $($left.relative_path) and $($right.relative_path)"
            }
        }
    }
    return [pscustomobject]@{ id = $id; roots = @($roots) }
}

function Ensure-CasObject {
    param([Parameter(Mandatory = $true)][string] $Archive, [Parameter(Mandatory = $true)][string] $Source, [Parameter(Mandatory = $true)][string] $Hash)
    $relativeObject = "objects/sha256/$($Hash.Substring(0, 2))/$Hash"
    $destination = Resolve-UnderRoot -Root $Archive -RelativePath $relativeObject
    [System.IO.Directory]::CreateDirectory((Split-Path -Parent $destination)) | Out-Null
    if (Test-Path -LiteralPath $destination -PathType Leaf) {
        if ((Get-Sha256 $destination) -ne $Hash) { throw "Existing CAS object hash mismatch: $relativeObject" }
        return $relativeObject
    }
    $temporary = "$destination.partial-$([guid]::NewGuid().ToString('N'))"
    Copy-Item -LiteralPath $Source -Destination $temporary -Force
    try {
        if ((Get-Sha256 $temporary) -ne $Hash) { throw "Copied CAS object hash mismatch: $relativeObject" }
        Move-Item -LiteralPath $temporary -Destination $destination
    }
    finally {
        if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary -Force }
    }
    return $relativeObject
}

function Verify-Container {
    param([Parameter(Mandatory = $true)][string] $Archive, [Parameter(Mandatory = $true)][string] $Id)
    $manifestPath = Get-ContainerPath -Archive $Archive -Id $Id
    $manifest = Read-JsonFile $manifestPath
    $files = @($manifest.files)
    $bytes = [int64]0
    foreach ($file in $files) {
        $object = Resolve-UnderRoot -Root $Archive -RelativePath ([string]$file.object)
        if (-not (Test-Path -LiteralPath $object -PathType Leaf)) { throw "Missing CAS object: $($file.object)" }
        if ((Get-Sha256 $object) -ne [string]$file.sha256) { throw "CAS object integrity failure: $($file.object)" }
        $bytes += [int64]$file.size_bytes
    }
    return [pscustomobject]@{ action = 'verify'; container_id = $Id; files = $files.Count; bytes = $bytes; manifest = $manifestPath; status = 'verified' }
}

function Seal-Container {
    param([Parameter(Mandatory = $true)][string] $Project, [Parameter(Mandatory = $true)][string] $Archive, [Parameter(Mandatory = $true)][string] $SelectionFile, [switch] $Evict)
    $selection = Get-Selection -SelectionFile $SelectionFile -Project $Project
    $entries = [System.Collections.Generic.List[object]]::new()
    $bytes = [int64]0
    foreach ($root in $selection.roots) {
        foreach ($file in @(Get-ChildItem -LiteralPath $root.full_path -Force -Recurse -File | Sort-Object FullName)) {
            if (($file.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Reparse-point file is not supported: $($file.FullName)" }
            $relative = $file.FullName.Substring($Project.Length).TrimStart('\', '/').Replace('\', '/')
            $hash = Get-Sha256 $file.FullName
            $object = Ensure-CasObject -Archive $Archive -Source $file.FullName -Hash $hash
            $entries.Add([ordered]@{ path = $relative; sha256 = $hash; size_bytes = [int64]$file.Length; last_write_utc = $file.LastWriteTimeUtc.ToString('o'); object = $object })
            $bytes += [int64]$file.Length
        }
    }
    $manifest = [ordered]@{ version = 1; format = 'sha256-cas-container'; container_id = $selection.id; sealed_at_utc = [DateTime]::UtcNow.ToString('o'); source_roots = @($selection.roots | ForEach-Object { $_.relative_path }); file_count = $entries.Count; byte_count = $bytes; files = @($entries) }
    $externalManifest = Get-ContainerPath -Archive $Archive -Id $selection.id
    Write-JsonFile -Path $externalManifest -Value $manifest
    $verification = Verify-Container -Archive $Archive -Id $selection.id
    $pointer = [ordered]@{ version = 1; format = 'sha256-cas-container-pointer'; container_id = $selection.id; state = $(if ($Evict) { 'sealed_and_evicted' } else { 'sealed_copy_only' }); archive_root = $Archive; external_manifest = "containers/$($selection.id).json"; external_manifest_sha256 = (Get-Sha256 $externalManifest); source_roots = $manifest.source_roots; file_count = $manifest.file_count; byte_count = $manifest.byte_count; sealed_at_utc = $manifest.sealed_at_utc }
    $projectPointer = Resolve-UnderRoot -Root $Project -RelativePath "archive/containers/$($selection.id).json"
    Write-JsonFile -Path $projectPointer -Value $pointer
    if ($Evict) {
        foreach ($root in $selection.roots) { if (-not (Test-Path -LiteralPath $root.full_path -PathType Container)) { throw "Source vanished before eviction: $($root.relative_path)" } }
        foreach ($root in $selection.roots) {
            Remove-Item -LiteralPath $root.full_path -Recurse -Force
            if (Test-Path -LiteralPath $root.full_path) { throw "Source eviction failed: $($root.relative_path)" }
        }
    }
    return [pscustomobject]@{ action = 'seal'; container_id = $selection.id; state = $pointer.state; files = $verification.files; bytes = $verification.bytes; external_manifest = $externalManifest; project_pointer = $projectPointer; status = 'verified' }
}

function Restore-Container {
    param([Parameter(Mandatory = $true)][string] $Archive, [Parameter(Mandatory = $true)][string] $Id, [Parameter(Mandatory = $true)][string] $Destination)
    $restore = Get-NormalizedDirectory -Path $Destination -Create
    $manifest = (Read-JsonFile (Get-ContainerPath -Archive $Archive -Id $Id))
    foreach ($file in @($manifest.files)) {
        $object = Resolve-UnderRoot -Root $Archive -RelativePath ([string]$file.object)
        $target = Resolve-UnderRoot -Root $restore -RelativePath ([string]$file.path)
        if (Test-Path -LiteralPath $target) { throw "Restore destination already exists: $target" }
        [System.IO.Directory]::CreateDirectory((Split-Path -Parent $target)) | Out-Null
        Copy-Item -LiteralPath $object -Destination $target
        if ((Get-Sha256 $target) -ne [string]$file.sha256) { Remove-Item -LiteralPath $target -Force; throw "Restored file hash mismatch: $($file.path)" }
    }
    return [pscustomobject]@{ action = 'restore'; container_id = $Id; restore_root = $restore; files = @($manifest.files).Count; status = 'verified' }
}

$project = Get-NormalizedDirectory -Path $ProjectRoot
$archive = Get-NormalizedDirectory -Path $ArchiveRoot -Create
switch ($Action) {
    'Seal' { if ([string]::IsNullOrWhiteSpace($SelectionPath)) { throw 'Seal requires -SelectionPath.' }; Seal-Container -Project $project -Archive $archive -SelectionFile $SelectionPath -Evict:$Evict | ConvertTo-Json -Compress }
    'Verify' { if ([string]::IsNullOrWhiteSpace($ContainerId)) { throw 'Verify requires -ContainerId.' }; Verify-Container -Archive $archive -Id $ContainerId | ConvertTo-Json -Compress }
    'Restore' { if ([string]::IsNullOrWhiteSpace($ContainerId) -or [string]::IsNullOrWhiteSpace($RestoreRoot)) { throw 'Restore requires -ContainerId and -RestoreRoot.' }; Restore-Container -Archive $archive -Id $ContainerId -Destination $RestoreRoot | ConvertTo-Json -Compress }
}