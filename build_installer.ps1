Write-Host "Building Crane Setup.exe..." -ForegroundColor Cyan

# Ensure cargo-packager is installed
if (!(Get-Command "cargo-packager" -ErrorAction SilentlyContinue)) {
    Write-Host "Installing cargo-packager..."
    cargo install cargo-packager
}

# Build the installer
cargo packager --release -f nsis

Write-Host "Done! Check target/release/installer for the setup.exe file." -ForegroundColor Green
