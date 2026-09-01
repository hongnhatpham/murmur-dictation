$ErrorActionPreference = "Stop"

$signingDirectory = Join-Path $env:APPDATA "com.bynhat.murmur\signing"
$privateKey = Join-Path $signingDirectory "murmur.key"
$passwordFile = Join-Path $signingDirectory "murmur.key.password.dpapi"

if ((Test-Path -LiteralPath $privateKey) -or (Test-Path -LiteralPath "$privateKey.pub")) {
    throw "Updater signing material already exists at $signingDirectory. Refusing to replace it."
}

New-Item -ItemType Directory -Force -Path $signingDirectory | Out-Null
$bytes = New-Object byte[] 32
$random = [Security.Cryptography.RandomNumberGenerator]::Create()
try { $random.GetBytes($bytes) } finally { $random.Dispose() }
$password = [Convert]::ToBase64String($bytes)

corepack pnpm exec tauri signer generate --ci --password $password --write-keys $privateKey
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$protectedPassword = ConvertFrom-SecureString (ConvertTo-SecureString $password -AsPlainText -Force)
[IO.File]::WriteAllText($passwordFile, $protectedPassword)
Write-Output "Updater signing material created at $signingDirectory. Back up this directory securely."
