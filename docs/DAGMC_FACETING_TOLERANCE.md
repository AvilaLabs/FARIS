# DAGMC faceting tolerance metadata

The installed DAGMC reader looks up the sparse double tag `FACETING_TOL` on
MOAB entity handle `0`. The producer used by the CAD conversion currently
writes `FACETING_TOLERANCE` with class `MB_TAG_DENSE` and a value estimated
from the mesh bounding box. That is not the tag name or storage class queried
by DAGMC, and the estimate is not the mesher's declared tolerance.

`integrations/cad/dagmc_faceting_tolerance.py` preserves the producer H5M and
creates a separate transport H5M with `FACETING_TOL`, sparse class `1`, root
handle `0`, and the exact declared mesher input tolerance in centimeters.
The report includes both file hashes and semantic hashes for every unchanged
HDF5 dataset. This records the requested tessellation input; it does not claim
that the input is a certified upper bound on geometric deviation. Deviation
qualification remains a separate CAD evidence check.

The representation is confirmed by the installed MOAB enum in
`Types.hpp` (`MB_TAG_SPARSE = 1 << 0`) and DAGMC's `finish_loading` lookup,
which requests `FACETING_TOL` with `MB_TAG_SPARSE` and `MB_TYPE_DOUBLE`.
