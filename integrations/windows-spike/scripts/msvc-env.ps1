# Loads the x64 MSVC environment into GITHUB_ENV so every later step can call cl, link and ninja.
$ErrorActionPreference = 'Stop'
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw 'no Visual Studio with the x64 C++ tools found' }
Write-Host "Visual Studio: $vs"
$bat = Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat'
$vars = cmd /c "`"$bat`" >nul 2>&1 && set"
foreach ($line in $vars) {
    if ($line -match '^([^=]+)=(.*)$') {
        $name = $Matches[1]
        if ($name -in 'GITHUB_ENV', 'GITHUB_PATH', 'GITHUB_OUTPUT', 'GITHUB_STATE', 'GITHUB_STEP_SUMMARY') { continue }
        Add-Content -Path $env:GITHUB_ENV -Value "$name=$($Matches[2])" -Encoding utf8
    }
}
