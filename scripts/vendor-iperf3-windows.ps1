param(
    [string]$Version = "3.21"
)

$ErrorActionPreference = "Stop"
$assetName = "iperf-$Version-win64.zip"
$downloadUrl = "https://github.com/ar51an/iperf3-win-builds/releases/download/$Version/$assetName"
$expectedSha256 = "9b73b7e0e0326347b5f4ac4f6a1fc34fe60a5966e5fd172c7bfcd0e1cc93e709"
$projectRoot = Split-Path -Parent $PSScriptRoot
$destination = Join-Path $projectRoot "src-tauri\resources\iperf3\windows-x86_64"
$resolvedRoot = [IO.Path]::GetFullPath($projectRoot)
$resolvedDestination = [IO.Path]::GetFullPath($destination)
if (-not $resolvedDestination.StartsWith($resolvedRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "拒绝写入项目目录之外的路径：$resolvedDestination"
}
$work = Join-Path ([IO.Path]::GetTempPath()) ("iperf3-gui-vendor-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $work | Out-Null
try {
    $archive = Join-Path $work $assetName
    Invoke-WebRequest -Uri $downloadUrl -OutFile $archive
    $actualSha256 = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualSha256 -ne $expectedSha256) {
        throw "iperf3 压缩包校验失败。期望 $expectedSha256，实际 $actualSha256"
    }

    $expanded = Join-Path $work "expanded"
    Expand-Archive -LiteralPath $archive -DestinationPath $expanded
    $executable = Get-ChildItem -LiteralPath $expanded -Filter "iperf3.exe" -File -Recurse | Select-Object -First 1
    if (-not $executable) { throw "下载包中没有找到 iperf3.exe" }

    if (Test-Path -LiteralPath $destination) { Remove-Item -LiteralPath $destination -Recurse -Force }
    New-Item -ItemType Directory -Path $destination | Out-Null
    Copy-Item -Path (Join-Path $executable.Directory.FullName "*") -Destination $destination -Recurse -Force
    Invoke-WebRequest -Uri "https://raw.githubusercontent.com/esnet/iperf/$Version/LICENSE" -OutFile (Join-Path $destination "LICENSE-IPERF3.txt")
    Invoke-WebRequest -Uri "https://raw.githubusercontent.com/ar51an/iperf3-win-builds/main/LICENSE" -OutFile (Join-Path $destination "LICENSE-WINDOWS-BUILD.txt")

    & (Join-Path $destination "iperf3.exe") --version
    Write-Host "已内置 iperf3 $Version：$destination"
}
finally {
    if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
}
