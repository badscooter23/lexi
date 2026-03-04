# Lexi CLI installer for Windows
$ErrorActionPreference = "Stop"

Write-Host "Installing Lexi CLI..."

# Check Python
try {
    $pythonVersion = python --version 2>&1 | Select-String -Pattern '(\d+\.\d+)' | ForEach-Object { $_.Matches[0].Value }
    if ([version]$pythonVersion -lt [version]"3.9") {
        Write-Error "Python >= 3.9 is required (found $pythonVersion)."
        exit 1
    }
} catch {
    Write-Error "Python is required but not found."
    exit 1
}

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path

# Install CLI
Push-Location "$scriptDir\lexi-cli"
pip install -e . --quiet
Pop-Location

Write-Host "Lexi CLI installed. Run 'lexi' to start."

# Optionally install GUI
try {
    $nodeVersion = (node -v) -replace '^v', '' -replace '\..*', ''
    if ([int]$nodeVersion -ge 18) {
        Write-Host "Installing Lexi GUI..."
        Push-Location "$scriptDir\lexi-gui"
        npm install --quiet
        Pop-Location
        Write-Host "Lexi GUI installed. Run 'npm start' in lexi-gui\ to launch."
    } else {
        Write-Host "Skipping GUI install: Node >= 18 required."
    }
} catch {
    Write-Host "Skipping GUI install: Node.js not found."
}
