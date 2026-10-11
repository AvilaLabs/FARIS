# Passed as CMAKE_PROJECT_INCLUDE to MOAB-dependent builds. MOAB's exported target refers to the
# HDF5 imported targets by name, and MOABConfig.cmake only loads the HDF5 package when it sits at
# <HDF5_DIR>/share/cmake/hdf5, which is not where the official Windows package keeps it.
# Defining the targets here makes find_package(MOAB) work.
find_package(HDF5 REQUIRED COMPONENTS C HL)
message(STATUS "preload-hdf5: HDF5 ${HDF5_VERSION} from ${HDF5_DIR}; libraries: ${HDF5_C_LIBRARIES}")
