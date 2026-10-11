# Shared helpers for the Windows native spike build scripts. Dot-source this file.
$ErrorActionPreference = 'Stop'

$Spike = 'C:\spike'
$Src = "$Spike\src"
$Bld = "$Spike\build"
$Inst = "$Spike\inst"
$Logs = "$Spike\logs"
$Dist = "$Spike\dist"
$SpikeDir = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$PatchRoot = Join-Path $SpikeDir 'patches'

# Pinned versions (the workflow exports the same names so cache keys and scripts agree).
$Hdf5Version = '1.14.6'
$Hdf5Url = "https://github.com/HDFGroup/hdf5/releases/download/hdf5_$Hdf5Version/hdf5-$Hdf5Version-win-vs2022_cl.zip"
$EmbreeVersion = '4.3.3'
$EmbreeUrl = "https://github.com/RenderKit/embree/releases/download/v$EmbreeVersion/embree-$EmbreeVersion.x64.windows.zip"
$Hdf5Root = "$Inst\hdf5\HDF5-$Hdf5Version-win64"
$EmbreeRoot = "$Inst\embree"

foreach ($d in $Src, $Bld, $Inst, $Logs, $Dist) { New-Item -ItemType Directory -Force $d | Out-Null }

function Fwd([string]$Path) { return ($Path -replace '\\', '/') }

# Runs a native command, tees its output to a log file, throws on a non-zero exit code.
function Invoke-Logged {
    param([string]$Log, [string]$Exe, [string[]]$ArgList)
    $line = "> $Exe $($ArgList -join ' ')"
    Write-Host $line
    Add-Content -Path $Log -Value $line
    & $Exe @ArgList 2>&1 | Tee-Object -FilePath $Log -Append
    if ($LASTEXITCODE -ne 0) { throw "$Exe exited with code $LASTEXITCODE (log: $Log)" }
}

# Records "<component> <stage> <seconds>" so the report can quote runner build times.
function Add-Timing([string]$Name, [double]$Seconds) {
    Add-Content -Path "$Logs\timings.txt" -Value ("{0} {1:N0}s" -f $Name, $Seconds)
}

# Clones a pinned tag without line-ending conversion (the patches are LF).
function Get-Source {
    param([string]$Name, [string]$Url, [string]$Tag, [switch]$Submodules)
    $dir = "$Src\$Name"
    if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }
    $log = "$Logs\$Name-source.log"
    $gitArgs = @('-c', 'core.autocrlf=false', 'clone', '--depth', '1', '--branch', $Tag)
    if ($Submodules) { $gitArgs += @('--recurse-submodules', '--shallow-submodules') }
    $gitArgs += @($Url, $dir)
    Invoke-Logged $log git $gitArgs
    $sha = (& git -C $dir rev-parse HEAD)
    Add-Content -Path $log -Value "$Name $Tag resolved to $sha"
}

# Applies every patches/<component>/*.patch in name order.
function Set-Patches {
    param([string]$Name, [string]$Component)
    $dir = Join-Path $PatchRoot $Component
    if (-not (Test-Path $dir)) { return }
    foreach ($p in (Get-ChildItem $dir -Filter *.patch | Sort-Object Name)) {
        Invoke-Logged "$Logs\$Name-patches.log" git @('-C', "$Src\$Name", 'apply', '--verbose', $p.FullName)
    }
}

# Flags every build uses: hdf5 import library, math constants, no windows.h min/max macros.
function Set-CommonFlags {
    $env:CFLAGS = '/DH5_BUILT_AS_DYNAMIC_LIB /D_USE_MATH_DEFINES /DNOMINMAX /D_CRT_SECURE_NO_WARNINGS'
    $env:CXXFLAGS = '/DH5_BUILT_AS_DYNAMIC_LIB /D_USE_MATH_DEFINES /DNOMINMAX /D_CRT_SECURE_NO_WARNINGS /bigobj /FIiso646.h'
}

function Get-CmakeCommon([string]$Prefix, [string]$BuildType = 'RelWithDebInfo') {
    return @(
        '-G', 'Ninja', "-DCMAKE_BUILD_TYPE=$BuildType",
        # CMake 4 rejects cmake_minimum_required below 3.5, which double-down still declares
        '-DCMAKE_POLICY_VERSION_MINIMUM=3.5',
        '-DCMAKE_C_COMPILER=cl', '-DCMAKE_CXX_COMPILER=cl',
        "-DCMAKE_INSTALL_PREFIX=$(Fwd $Prefix)",
        # the official HDF5 package only ships RelWithDebInfo import targets
        '-DCMAKE_MAP_IMPORTED_CONFIG_RELEASE=RELWITHDEBINFO;RELEASE',
        "-DHDF5_DIR=$(Fwd "$Hdf5Root\cmake")",
        "-DCMAKE_PROJECT_INCLUDE=$(Fwd "$SpikeDir\cmake\preload-hdf5.cmake")"
    )
}

# configure, build, install with logs; reports the elapsed time.
function Build-Cmake {
    param([string]$Name, [string[]]$ConfigureArgs, [string]$SourceSubdir = '', [string]$Tag = '')
    if (-not $Tag) { $Tag = $Name }
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $b = "$Bld\$Tag"
    if (Test-Path $b) { Remove-Item -Recurse -Force $b }
    $s = "$Src\$Name$SourceSubdir"
    Invoke-Logged "$Logs\$Tag-configure.log" cmake (@('-S', (Fwd $s), '-B', (Fwd $b)) + $ConfigureArgs)
    Invoke-Logged "$Logs\$Tag-build.log" cmake @('--build', (Fwd $b))
    Invoke-Logged "$Logs\$Tag-install.log" cmake @('--install', (Fwd $b))
    Add-Timing $Tag $sw.Elapsed.TotalSeconds
}
