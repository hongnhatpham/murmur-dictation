$ErrorActionPreference = "Stop"

$cargoDirectory = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path -LiteralPath $cargoDirectory) {
    $env:Path = "$cargoDirectory;$env:Path"
}

$signingDirectory = Join-Path $env:APPDATA "com.bynhat.murmur\signing"
$privateKey = Join-Path $signingDirectory "murmur.key"
$publicKey = "$privateKey.pub"
$passwordFile = Join-Path $signingDirectory "murmur.key.password.dpapi"

foreach ($path in @($privateKey, $publicKey, $passwordFile)) {
    if (-not (Test-Path -LiteralPath $path)) {
        throw "Missing updater signing material: $path"
    }
}

$securePassword = Get-Content -Raw -LiteralPath $passwordFile | ConvertTo-SecureString
$passwordPointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($securePassword)
$updaterConfigPath = Join-Path ([IO.Path]::GetTempPath()) (
    "murmur-tauri-updater-{0}.json" -f [guid]::NewGuid()
)

try {
    $env:TAURI_SIGNING_PRIVATE_KEY_PATH = $privateKey
    $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content -Raw -LiteralPath $privateKey
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($passwordPointer)
    $env:MURMUR_UPDATER_PUBLIC_KEY = (Get-Content -Raw -LiteralPath $publicKey).Trim()
    $env:CARGO_BUILD_JOBS = "1"
    $updaterConfig = @{
        plugins = @{
            updater = @{
                pubkey = $env:MURMUR_UPDATER_PUBLIC_KEY
                endpoints = @()
            }
        }
    } | ConvertTo-Json -Compress -Depth 4
    [IO.File]::WriteAllText(
        $updaterConfigPath,
        $updaterConfig,
        [Text.UTF8Encoding]::new($false)
    )
    corepack pnpm exec tauri build --config $updaterConfigPath
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
finally {
    [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($passwordPointer)
    Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY_PATH -ErrorAction SilentlyContinue
    Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY -ErrorAction SilentlyContinue
    Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
    Remove-Item Env:MURMUR_UPDATER_PUBLIC_KEY -ErrorAction SilentlyContinue
    Remove-Item Env:CARGO_BUILD_JOBS -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $updaterConfigPath) {
        Remove-Item -LiteralPath $updaterConfigPath -Force
    }
}
