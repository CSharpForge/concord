#Requires -Version 7
<#
Smoke-tests the Windows installer end to end: silent per-user install, first
start (minimized, as at login), database created and migrated, a second launch
exits (single instance), then uninstall.

  pwsh scripts/smoke-test-installer.ps1 -Installer path\to\Concord_0.1.0_x64-setup.exe

Needs Python 3 (to read the SQLite database). It installs and removes Concord
for the CURRENT USER, so run it on a test machine or a CI runner, not on a
machine where you use Concord for real.
#>
param([Parameter(Mandatory)][string]$Installer)
$ErrorActionPreference = 'Stop'
function Step($m) { Write-Host "`n=== $m" }

Step 'Install silently (per user, no administrator rights)'
$p = Start-Process -FilePath $Installer -ArgumentList '/S' -PassThru -Wait
if ($p.ExitCode -ne 0) { throw "installer exited with code $($p.ExitCode)" }
$dir = Join-Path $env:LOCALAPPDATA 'Concord'
if (-not (Test-Path $dir)) {
    Get-ChildItem $env:LOCALAPPDATA -Directory | ForEach-Object Name
    throw "nothing was installed under $dir (a per-user install was expected)"
}
Get-ChildItem $dir | Format-Table Name, Length -AutoSize | Out-Host
$app = Get-ChildItem $dir -Filter '*.exe' | Where-Object { $_.Name -notmatch '^uninstall' } | Select-Object -First 1
if (-not $app) { throw "no application .exe found in $dir" }
Write-Host "installed: $($app.FullName)"

Step 'Start minimized (as when started at login)'
$proc = Start-Process -FilePath $app.FullName -ArgumentList '--minimized' -PassThru
Start-Sleep -Seconds 20
if ($proc.HasExited) { throw "Concord exited by itself with code $($proc.ExitCode)" }
Write-Host "running (pid $($proc.Id))"

Step 'Database created and migrated'
$db = Join-Path $env:APPDATA 'app.concord.desktop\concord.db'
if (-not (Test-Path $db)) { throw "no database at $db" }
python (Join-Path $PSScriptRoot 'check_db.py') $db
if ($LASTEXITCODE -ne 0) { throw 'database check failed' }

Step 'A second launch must exit (single instance)'
$second = Start-Process -FilePath $app.FullName -PassThru
if (-not $second.WaitForExit(20000)) {
    Stop-Process -Id $second.Id -Force
    throw 'a second copy kept running: single-instance is not working'
}
Write-Host "second copy exited with code $($second.ExitCode)"
$count = @(Get-Process -Name $app.BaseName -ErrorAction SilentlyContinue).Count
if ($count -ne 1) { throw "expected exactly 1 Concord process, found $count" }
if ($proc.HasExited) { throw 'the first copy died during the second launch' }

Step 'Uninstall'
Stop-Process -Id $proc.Id -Force
Start-Sleep -Seconds 2
$uninstaller = Join-Path $dir 'uninstall.exe'
if (-not (Test-Path $uninstaller)) { throw "no uninstaller at $uninstaller" }
Start-Process -FilePath $uninstaller -ArgumentList '/S' -Wait
for ($i = 0; $i -lt 40 -and (Test-Path $app.FullName); $i++) { Start-Sleep -Seconds 1 }
if (Test-Path $app.FullName) { throw 'uninstalling left the application behind' }

Write-Host "`nSmoke test passed."
