# Distribute updates through private GitHub Releases

Murmur retrieves Tauri-signed artifacts from private GitHub Releases using a fine-grained `Contents: read` token scoped to this repository and stored in Windows Credential Manager. GitHub's permission also permits source reads, which is accepted for this personal tool to avoid operating a separate authenticated update service.
