#Requires -Version 7.0
<#
.SYNOPSIS
    Runs the Ensemble test suite on Linux via Podman, for pre-release verification.
.DESCRIPTION
    Builds a Linux container image containing the workspace and runs the full
    test suite inside it. This catches Linux-only build/test failures (such as
    the MIDI bridge's ALSA dependency) before tagging a release.
.PARAMETER Release
    Additionally verify release-mode builds with `cargo build --workspace --release`.
.EXAMPLE
    ./scripts/linux-test.ps1
.EXAMPLE
    ./scripts/linux-test.ps1 -Release
#>
[CmdletBinding()]
param(
    [switch]$Release
)

$ErrorActionPreference = 'Stop'

$imageName = 'ensemble-linux-test'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

# Ensure the Podman machine is running.
$machineRunning = podman machine list --format '{{.Running}}' 2>$null |
    Where-Object { $_ -eq 'true' }
if (-not $machineRunning) {
    Write-Host 'Starting Podman machine...'
    podman machine start | Out-Null
}

Write-Host 'Building Linux test image...'
podman build -t $imageName -f (Join-Path $repoRoot 'containers/Containerfile') $repoRoot
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host 'Running workspace test suite on Linux...'
# No ENTRYPOINT in the image, so extra args replace the default test CMD.
podman run --rm $imageName
$verifyExit = $LASTEXITCODE

if ($Release -and $verifyExit -eq 0) {
    Write-Host 'Verifying release-mode build...'
    podman run --rm $imageName cargo build --workspace --release
    $verifyExit = $LASTEXITCODE
}

if ($verifyExit -eq 0) {
    Write-Host 'Linux verification PASSED.'
}
else {
    Write-Host "Linux verification FAILED (exit $verifyExit)."
}
exit $verifyExit
