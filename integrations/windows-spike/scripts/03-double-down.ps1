# Stage 3: double-down 1.1.0 against Embree 4 and the MOAB from stage 2.
. "$PSScriptRoot\common.ps1"
Get-Source double-down 'https://github.com/pshriwise/double-down.git' 'v1.1.0'
Set-Patches double-down 'double-down'
Set-CommonFlags
$cm = (Get-CmakeCommon "$Inst\dd") + @(
    "-DMOAB_DIR=$(Fwd "$Inst\moab\lib\cmake\MOAB")",
    "-DEMBREE_DIR=$(Fwd "$EmbreeRoot\lib\cmake\embree-$EmbreeVersion")"
)
Build-Cmake double-down $cm
foreach ($f in 'bin\dd.dll', 'lib\dd.lib', 'lib\cmake\dd\ddConfig.cmake') {
    if (-not (Test-Path "$Inst\dd\$f")) { throw "double-down install is missing $f" }
}
