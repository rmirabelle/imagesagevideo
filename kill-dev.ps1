<#
.SYNOPSIS
  Stop the ImageSageVideo dev app: tree-kill imagesagevideo.exe (and the engine it started) and free the Vite port.
#>
$ErrorActionPreference = "Continue"

Get-Process -Name imagesagevideo -ErrorAction SilentlyContinue | ForEach-Object {
  Write-Host "Stopping imagesagevideo.exe (PID $($_.Id))"
  & taskkill.exe /PID $_.Id /T /F | Out-Null
}

foreach ($port in 14420, 14421) {
  Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue |
    Select-Object -ExpandProperty OwningProcess -Unique |
    ForEach-Object {
      Write-Host "Stopping PID $_ (port $port)"
      & taskkill.exe /PID $_ /T /F | Out-Null
    }
}
