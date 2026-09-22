[CmdletBinding()]
param(
    [string] $OutputDirectory = "dist"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if (-not $IsWindows -or [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne "X64") {
    throw "Snail's Windows package must be built on x86-64 Windows."
}

$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$manifest = Get-Content -LiteralPath (Join-Path $projectRoot "Cargo.toml") -Raw
$versionMatch = [regex]::Match($manifest, '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"')
if (-not $versionMatch.Success) {
    throw "Could not read the workspace package version."
}
$version = $versionMatch.Groups[1].Value
$packageName = "snail-$version-windows-x86_64"
$outputRoot = [IO.Path]::GetFullPath((Join-Path $projectRoot $OutputDirectory))
$stageRoot = [IO.Path]::GetFullPath((Join-Path $outputRoot $packageName))
if ($stageRoot -eq $projectRoot -or -not $stageRoot.StartsWith($outputRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing unsafe staging path: $stageRoot"
}

Push-Location $projectRoot
try {
    cargo build --release --locked --target x86_64-pc-windows-msvc --bin snail --bin snail-cli
    if ($LASTEXITCODE -ne 0) { throw "Release build failed." }

    $targetRoot = Join-Path $projectRoot "target\x86_64-pc-windows-msvc\release"
    $executables = @(
        (Join-Path $targetRoot "snail.exe"),
        (Join-Path $targetRoot "snail-cli.exe")
    )
    foreach ($executable in $executables) {
        if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
            throw "Expected release executable is missing: $executable"
        }
    }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path -LiteralPath $vswhere)) { throw "Visual Studio vswhere.exe is missing." }
    $vsRoot = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    $dumpbin = Get-ChildItem -LiteralPath (Join-Path $vsRoot "VC\Tools\MSVC") -Filter dumpbin.exe -Recurse |
        Where-Object FullName -Match '\\bin\\Hostx64\\x64\\dumpbin\.exe$' |
        Sort-Object FullName -Descending |
        Select-Object -First 1
    if ($null -eq $dumpbin) { throw "Could not find the x64 dumpbin.exe dependency inspector." }

    foreach ($executable in $executables) {
        $imports = & $dumpbin.FullName /nologo /dependents $executable |
            ForEach-Object { if ($_ -match '^\s+([A-Za-z0-9_.-]+\.dll)\s*$') { $Matches[1] } } |
            Sort-Object -Unique
        foreach ($import in $imports) {
            $apiSetContract = $import -match '^(api|ext)-ms-win-.*\.dll$'
            if (-not $apiSetContract -and -not (Test-Path -LiteralPath (Join-Path $env:WINDIR "System32\$import"))) {
                throw "Unexpected non-system runtime dependency in $([IO.Path]::GetFileName($executable)): $import"
            }
        }
    }

    New-Item -ItemType Directory -Force -Path $outputRoot | Out-Null
    if (Test-Path -LiteralPath $stageRoot) { Remove-Item -LiteralPath $stageRoot -Recurse -Force }
    New-Item -ItemType Directory -Path $stageRoot | Out-Null
    Copy-Item -LiteralPath $executables[0] -Destination $stageRoot
    Copy-Item -LiteralPath $executables[1] -Destination $stageRoot
    Copy-Item -LiteralPath (Join-Path $projectRoot "docs\WINDOWS.md") -Destination (Join-Path $stageRoot "README-WINDOWS.md")

    $unexpected = Get-ChildItem -LiteralPath $stageRoot -File | Where-Object Name -NotIn @(
        "snail.exe", "snail-cli.exe", "README-WINDOWS.md"
    )
    if ($unexpected) { throw "Unexpected loose package file: $($unexpected.Name -join ', ')" }

    $zip = Join-Path $outputRoot "$packageName.zip"
    if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
    Compress-Archive -LiteralPath $stageRoot -DestinationPath $zip -CompressionLevel Optimal
    $checksum = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    $checksumPath = "$zip.sha256"
    Set-Content -LiteralPath $checksumPath -Value "$checksum  $([IO.Path]::GetFileName($zip))" -Encoding ascii

    Write-Output $zip
    Write-Output $checksumPath
}
finally {
    Pop-Location
}
