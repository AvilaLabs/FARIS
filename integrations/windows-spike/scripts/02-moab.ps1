# Stage 2: MOAB 5.6.0, shared, HDF5 on, no MPI, no Fortran, no tests.
. "$PSScriptRoot\common.ps1"
Get-Source moab 'https://bitbucket.org/fathomteam/moab.git' '5.6.0'
Set-Patches moab 'moab'
Set-CommonFlags
$cm = (Get-CmakeCommon "$Inst\moab") + @(
    '-DBUILD_SHARED_LIBS=ON', '-DENABLE_HDF5=ON', '-DENABLE_MPI=OFF', '-DENABLE_FORTRAN=OFF',
    '-DENABLE_BLASLAPACK=OFF', '-DENABLE_TESTING=OFF', '-DENABLE_PYMOAB=OFF', '-DENABLE_NETCDF=OFF',
    '-DENABLE_IMESH=OFF'
)
Build-Cmake moab $cm
foreach ($f in 'bin\MOAB.dll', 'lib\MOAB.lib', 'lib\cmake\MOAB\MOABConfig.cmake') {
    if (-not (Test-Path "$Inst\moab\$f")) { throw "MOAB install is missing $f" }
}
