param(
  [string]$ExpectedRoot = "C:\Program Files\Qexow CAM"
)

$ErrorActionPreference = "Stop"

$camNames = @(
  "cam.exe",
  "qexow-cam-gui.exe",
  "cam-tray.exe",
  "qexow-tray-proof.exe",
  "tray_windows_release.exe",
  "cam-core.exe"
)

$staleRoots = @(
  (Join-Path $env:LOCALAPPDATA "Programs\Qexow CAM"),
  (Join-Path $env:LOCALAPPDATA "Programs\Codex Agent Manager"),
  (Join-Path $env:LOCALAPPDATA "Qexow CAM"),
  "C:\Program Files (x86)\Qexow CAM"
)

$problems = New-Object System.Collections.Generic.List[string]

$processes = Get-CimInstance Win32_Process | Where-Object {
  $camNames -contains $_.Name -and (
    ($_.ExecutablePath -and -not $_.ExecutablePath.StartsWith($ExpectedRoot, [StringComparison]::OrdinalIgnoreCase)) -or
    ($_.CommandLine -and ($_.CommandLine -like "*Qexow CAM*" -or $_.CommandLine -like "*Codex Agent Manager*") -and $_.CommandLine -notlike "*$ExpectedRoot*")
  )
}

foreach ($process in $processes) {
  $problems.Add("stale process pid=$($process.ProcessId) name=$($process.Name) path=$($process.ExecutablePath) cmd=$($process.CommandLine)")
}

foreach ($root in $staleRoots) {
  if (-not $root -or -not (Test-Path -LiteralPath $root)) {
    continue
  }

  $staleExe = Get-ChildItem -LiteralPath $root -Recurse -File -ErrorAction SilentlyContinue |
    Where-Object { $camNames -contains $_.Name } |
    Select-Object -First 1

  if ($staleExe) {
    $problems.Add("stale CAM executable remains under $root at $($staleExe.FullName)")
  }
}

$startup = [Environment]::GetFolderPath("Startup")
foreach ($name in @("CodexAgentManager.cmd", "QexowCam.cmd", "Codex Agent Manager.cmd")) {
  $path = Join-Path $startup $name
  if (Test-Path -LiteralPath $path) {
    $problems.Add("stale startup command remains at $path")
  }
}

foreach ($runKey in @("HKCU:\Software\Microsoft\Windows\CurrentVersion\Run", "HKLM:\Software\Microsoft\Windows\CurrentVersion\Run")) {
  foreach ($runName in @("Qexow CAM GUI", "Qexow CAM Tray Proof", "Codex Agent Manager", "Codex Agent Manager Tray")) {
    try {
      $value = (Get-ItemProperty -Path $runKey -Name $runName -ErrorAction Stop).$runName
      if ($value) {
        $problems.Add("stale Run entry $runKey\$runName -> $value")
      }
    } catch {
    }
  }
}

if ($problems.Count -gt 0) {
  $problems | ForEach-Object { Write-Host "ERROR: $_" }
  exit 1
}

Write-Host "No stale CAM installs, processes, startup commands, or old Run entries detected outside $ExpectedRoot."
