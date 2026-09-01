$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$tauriDirectory = Join-Path $repositoryRoot "src-tauri"
$bundleDirectory = Join-Path $tauriDirectory "target\release\bundle\nsis"
$setupCli = Join-Path $tauriDirectory "target\release\murmur-setup.exe"
$publicKeyPath = Join-Path $env:APPDATA "com.bynhat.murmur\signing\murmur.key.pub"
$configPath = Join-Path $tauriDirectory "tauri.conf.json"

foreach ($requiredFile in @($setupCli, $publicKeyPath, $configPath)) {
    if (-not (Test-Path -LiteralPath $requiredFile -PathType Leaf)) {
        throw "Missing signed-update verification input: $requiredFile"
    }
}

$installers = @(Get-ChildItem -LiteralPath $bundleDirectory -File -Filter "Murmur_*_x64-setup.exe")
if ($installers.Count -ne 1) {
    throw "Expected exactly one NSIS installer in $bundleDirectory; found $($installers.Count)."
}

$installer = $installers[0]
$signaturePath = "$($installer.FullName).sig"
if (-not (Test-Path -LiteralPath $signaturePath -PathType Leaf)) {
    throw "Missing updater signature for $($installer.Name)."
}

$publicKey = (Get-Content -Raw -LiteralPath $publicKeyPath).Trim()
$signature = (Get-Content -Raw -LiteralPath $signaturePath).Trim()
if ([string]::IsNullOrWhiteSpace($publicKey) -or [string]::IsNullOrWhiteSpace($signature)) {
    throw "The updater public key or installer signature is empty."
}

$version = (Get-Content -Raw -LiteralPath $configPath | ConvertFrom-Json).version
$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
$verificationDirectory = Join-Path $tempRoot ("murmur-update-verify-{0}" -f [guid]::NewGuid())
$verificationDirectory = [IO.Path]::GetFullPath($verificationDirectory)
$verificationLeaf = Split-Path -Leaf $verificationDirectory
$verificationParent = Split-Path -Parent $verificationDirectory

if ($verificationParent -ne $tempRoot -or -not $verificationLeaf.StartsWith("murmur-update-verify-")) {
    throw "Refusing to use an unexpected temporary verification path."
}

try {
    New-Item -ItemType Directory -Path $verificationDirectory | Out-Null
    $stagedInstaller = Join-Path $verificationDirectory $installer.Name
    $stagedSignature = "$stagedInstaller.sig"
    Copy-Item -LiteralPath $installer.FullName -Destination $stagedInstaller
    Copy-Item -LiteralPath $signaturePath -Destination $stagedSignature

    $handoff = [ordered]@{
        version = $version
        artifactPath = $stagedInstaller
        signature = $signature
        sha256 = (Get-FileHash -LiteralPath $stagedInstaller -Algorithm SHA256).Hash.ToLowerInvariant()
        downloadedAt = [DateTimeOffset]::UtcNow.ToString("o")
    }
    $readyStatePath = Join-Path $verificationDirectory "ready-update.json"
    [IO.File]::WriteAllText(
        $readyStatePath,
        ($handoff | ConvertTo-Json),
        [Text.UTF8Encoding]::new($false)
    )

    $output = @(& $setupCli updates verify $verificationDirectory $publicKey 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "Signed updater verification failed: $($output -join [Environment]::NewLine)"
    }
    $response = $output[-1] | ConvertFrom-Json
    if (-not $response.ok -or -not $response.result) {
        throw "Signed updater verification returned an unsuccessful response."
    }

    [pscustomobject]@{
        verified = $true
        version = $response.result.version
        artifact = $installer.Name
        artifactSizeBytes = $response.result.artifactSizeBytes
        expectedSha256 = $response.result.expectedSha256
        stagedSignature = (Split-Path -Leaf $stagedSignature)
        readyState = (Split-Path -Leaf $readyStatePath)
    } | ConvertTo-Json
}
finally {
    if (Test-Path -LiteralPath $verificationDirectory) {
        $resolvedDirectory = (Resolve-Path -LiteralPath $verificationDirectory).Path
        if ($resolvedDirectory -ne $verificationDirectory -or
            (Split-Path -Parent $resolvedDirectory) -ne $tempRoot -or
            -not (Split-Path -Leaf $resolvedDirectory).StartsWith("murmur-update-verify-")) {
            throw "Refusing to remove an unexpected verification directory."
        }
        Remove-Item -LiteralPath $resolvedDirectory -Recurse -Force
    }
}
