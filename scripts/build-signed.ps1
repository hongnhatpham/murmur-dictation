$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$cargoDirectory = Join-Path $env:USERPROFILE ".cargo\bin"
$signingDirectory = Join-Path $env:APPDATA "com.bynhat.murmur\signing"
$privateKey = Join-Path $signingDirectory "murmur.key"
$publicKey = "$privateKey.pub"
$passwordFile = Join-Path $signingDirectory "murmur.key.password.dpapi"
$updaterConfigPath = Join-Path ([IO.Path]::GetTempPath()) ("murmur-tauri-updater-{0}.json" -f [guid]::NewGuid())

foreach ($path in @($privateKey, $publicKey, $passwordFile)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Missing updater signing material: $path"
    }
}

$environmentNames = @("Path", "TAURI_SIGNING_PRIVATE_KEY_PATH", "TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD", "MURMUR_UPDATER_PUBLIC_KEY", "CARGO_BUILD_JOBS")
$previousEnvironment = @{}
foreach ($name in $environmentNames) {
    $item = Get-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue
    $previousEnvironment[$name] = @{
        Exists = $null -ne $item
        Value = if ($null -ne $item) { $item.Value } else { $null }
    }
}

$passwordPointer = [IntPtr]::Zero
try {
    if (Test-Path -LiteralPath $cargoDirectory -PathType Container) {
        $env:Path = "$cargoDirectory;$env:Path"
    }

    $securePassword = Get-Content -Raw -LiteralPath $passwordFile | ConvertTo-SecureString
    $passwordPointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($securePassword)
    $env:TAURI_SIGNING_PRIVATE_KEY_PATH = $privateKey
    $env:TAURI_SIGNING_PRIVATE_KEY = [IO.File]::ReadAllText($privateKey)
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($passwordPointer)
    $env:MURMUR_UPDATER_PUBLIC_KEY = [IO.File]::ReadAllText($publicKey)
    $env:CARGO_BUILD_JOBS = "1"

    $updaterConfig = @{ plugins = @{ updater = @{ pubkey = $env:MURMUR_UPDATER_PUBLIC_KEY; endpoints = @() } } } | ConvertTo-Json -Compress -Depth 4
    [IO.File]::WriteAllText($updaterConfigPath, $updaterConfig, [Text.UTF8Encoding]::new($false))

    Push-Location -LiteralPath $repositoryRoot
    try {
        & pnpm exec tauri build --config $updaterConfigPath
        $buildExitCode = $LASTEXITCODE
    }
    finally {
        Pop-Location
    }
    if ($buildExitCode -ne 0) {
        throw "pnpm tauri build failed with exit code $buildExitCode."
    }

    & (Join-Path $PSScriptRoot "verify-signed-update.ps1")
    & (Join-Path $PSScriptRoot "write-update-manifest.ps1")
}
finally {
    if ($passwordPointer -ne [IntPtr]::Zero) {
        [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($passwordPointer)
    }
    foreach ($name in $environmentNames) {
        $previous = $previousEnvironment[$name]
        if ($previous.Exists) {
            [Environment]::SetEnvironmentVariable($name, $previous.Value, "Process")
        }
        else {
            [Environment]::SetEnvironmentVariable($name, $null, "Process")
        }
    }
    if (Test-Path -LiteralPath $updaterConfigPath) {
        Remove-Item -LiteralPath $updaterConfigPath -Force
    }
}
