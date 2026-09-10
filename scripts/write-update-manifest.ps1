param([string] $Notes = "See the GitHub release notes for details.")

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$tauriDirectory = Join-Path $repositoryRoot "src-tauri"
$bundleDirectory = Join-Path $tauriDirectory "target\release\bundle\nsis"
$configPath = Join-Path $tauriDirectory "tauri.conf.json"

if (-not (Test-Path -LiteralPath $configPath -PathType Leaf)) {
    throw "Missing Tauri configuration: $configPath"
}
$config = Get-Content -Raw -LiteralPath $configPath | ConvertFrom-Json
$version = [string]$config.version
$productName = [string]$config.productName
if ($version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$') {
    throw "Invalid release version in $configPath`: $version"
}

$installerName = "${productName}_${version}_x64-setup.exe"
$installerPath = Join-Path $bundleDirectory $installerName
$signaturePath = "$installerPath.sig"
foreach ($requiredFile in @($installerPath, $signaturePath)) {
    if (-not (Test-Path -LiteralPath $requiredFile -PathType Leaf)) {
        throw "Missing version $version updater artifact: $requiredFile"
    }
}

$signature = [IO.File]::ReadAllText($signaturePath)
if ([string]::IsNullOrWhiteSpace($signature)) {
    throw "The updater signature is empty: $signaturePath"
}
try {
    $decodedSignature = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($signature))
}
catch {
    throw "The updater signature is not a base64-wrapped minisign value: $signaturePath"
}
if (-not $decodedSignature.StartsWith("untrusted comment:")) {
    throw "The updater signature does not contain a base64-wrapped minisign value: $signaturePath"
}

$downloadUrl = "https://github.com/hongnhatpham/murmur-dictation/releases/download/v$version/$installerName"
$manifest = [ordered]@{
    version = $version
    notes = $Notes
    pub_date = [DateTimeOffset]::UtcNow.ToString("yyyy-MM-ddTHH:mm:ssZ")
    platforms = [ordered]@{
        "windows-x86_64" = [ordered]@{ signature = $signature; url = $downloadUrl }
    }
}
$manifestPath = Join-Path $bundleDirectory "latest.json"
[IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 4), [Text.UTF8Encoding]::new($false))

[pscustomobject]@{
    generated = $true
    version = $version
    artifact = $installerName
    signature = (Split-Path -Leaf $signaturePath)
    manifest = (Split-Path -Leaf $manifestPath)
    url = $downloadUrl
} | ConvertTo-Json
