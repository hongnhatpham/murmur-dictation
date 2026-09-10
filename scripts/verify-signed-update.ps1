$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Invoke-JsonCli {
    param(
        [Parameter(Mandatory)] [string] $Executable,
        [Parameter(Mandatory)] [string[]] $Arguments,
        [Parameter(Mandatory)] [string] $Description
    )

    $output = @(& $Executable @Arguments 2>&1)
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "$Description failed with exit code $exitCode`: $($output -join [Environment]::NewLine)"
    }
    if ($output.Count -eq 0) {
        throw "$Description returned no output."
    }
    try {
        $output[-1] | ConvertFrom-Json
    }
    catch {
        throw "$Description did not return valid JSON: $($output -join [Environment]::NewLine)"
    }
}

function Assert-TauriBase64 {
    param([string] $Value, [string] $Description)
    if ([string]::IsNullOrWhiteSpace($Value)) {
        throw "$Description is empty."
    }
    try {
        $decoded = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Value))
    }
    catch {
        throw "$Description is not a base64-wrapped minisign value."
    }
    if (-not $decoded.StartsWith("untrusted comment:")) {
        throw "$Description does not contain a base64-wrapped minisign value."
    }
}

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$tauriDirectory = Join-Path $repositoryRoot "src-tauri"
$bundleDirectory = Join-Path $tauriDirectory "target\release\bundle\nsis"
$appExecutable = Join-Path $tauriDirectory "target\release\murmur.exe"
$publicKeyPath = Join-Path $env:APPDATA "com.bynhat.murmur\signing\murmur.key.pub"
$configPath = Join-Path $tauriDirectory "tauri.conf.json"

foreach ($requiredFile in @($appExecutable, $publicKeyPath, $configPath)) {
    if (-not (Test-Path -LiteralPath $requiredFile -PathType Leaf)) {
        throw "Missing signed-update verification input: $requiredFile"
    }
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

$publicKey = [IO.File]::ReadAllText($publicKeyPath)
$signature = [IO.File]::ReadAllText($signaturePath)
Assert-TauriBase64 $publicKey "The updater public key"
Assert-TauriBase64 $signature "The installer signature"

$buildInfo = Invoke-JsonCli $appExecutable @("--setup", "build", "info") "Release build inspection"
if ($null -eq $buildInfo.PSObject.Properties["schemaVersion"] -or $null -eq $buildInfo.PSObject.Properties["ok"] -or $null -eq $buildInfo.PSObject.Properties["result"] -or $buildInfo.schemaVersion -ne 1 -or $buildInfo.ok -ne $true -or $null -eq $buildInfo.result) {
    throw "Release build inspection returned an unsuccessful response."
}
if ($buildInfo.result.version -ne $version) {
    throw "Built application version $($buildInfo.result.version) does not match configured version $version."
}
if ($buildInfo.result.embeddedFrontend -ne $true) {
    throw "Built application does not contain the production frontend."
}
if ($buildInfo.result.updaterKeyConfigured -ne $true) {
    throw "Built application does not contain an updater trust key."
}

$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
$verificationDirectory = [IO.Path]::GetFullPath((Join-Path $tempRoot ("murmur-update-verify-{0}" -f [guid]::NewGuid())))
if ((Split-Path -Parent $verificationDirectory) -ne $tempRoot -or -not (Split-Path -Leaf $verificationDirectory).StartsWith("murmur-update-verify-")) {
    throw "Refusing to use an unexpected temporary verification path."
}

try {
    New-Item -ItemType Directory -Path $verificationDirectory | Out-Null
    $stagedInstaller = Join-Path $verificationDirectory $installerName
    Copy-Item -LiteralPath $installerPath -Destination $stagedInstaller
    Copy-Item -LiteralPath $signaturePath -Destination "$stagedInstaller.sig"

    $handoff = [ordered]@{
        version = $version
        artifactPath = $stagedInstaller
        signature = $signature
        sha256 = (Get-FileHash -LiteralPath $stagedInstaller -Algorithm SHA256).Hash.ToLowerInvariant()
        downloadedAt = [DateTimeOffset]::UtcNow.ToString("o")
    }
    $readyStatePath = Join-Path $verificationDirectory "ready-update.json"
    [IO.File]::WriteAllText($readyStatePath, ($handoff | ConvertTo-Json), [Text.UTF8Encoding]::new($false))

    $response = Invoke-JsonCli $appExecutable @("--setup", "updates", "verify", $verificationDirectory) "Signed updater verification with the embedded trust key"
    if ($null -eq $response.PSObject.Properties["ok"] -or $null -eq $response.PSObject.Properties["result"] -or $response.ok -ne $true -or $null -eq $response.result) {
        throw "Signed updater verification returned an unsuccessful response."
    }
    if ($response.result.version -ne $version) {
        throw "Verified installer version $($response.result.version) does not match configured version $version."
    }

    [pscustomobject]@{
        verified = $true
        version = $response.result.version
        artifact = $installerName
        artifactSizeBytes = $response.result.artifactSizeBytes
        expectedSha256 = $response.result.expectedSha256
        embeddedFrontend = $true
        updaterKeyConfigured = $true
    } | ConvertTo-Json
}
finally {
    if (Test-Path -LiteralPath $verificationDirectory) {
        $resolvedDirectory = (Resolve-Path -LiteralPath $verificationDirectory).Path
        if ($resolvedDirectory -ne $verificationDirectory -or (Split-Path -Parent $resolvedDirectory) -ne $tempRoot -or -not (Split-Path -Leaf $resolvedDirectory).StartsWith("murmur-update-verify-")) {
            throw "Refusing to remove an unexpected verification directory."
        }
        Remove-Item -LiteralPath $resolvedDirectory -Recurse -Force
    }
}
