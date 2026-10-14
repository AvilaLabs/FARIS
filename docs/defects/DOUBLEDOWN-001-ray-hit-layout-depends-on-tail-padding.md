<!-- kind: reference -->
# DOUBLEDOWN-001: the intersection callback reads one buffer as two struct types whose `hit` offsets agree only through tail-padding reuse

Status: drafted, not reported. Verification and significance: see [README.md](README.md). Found by the Windows native
spike ([WINDOWS_NATIVE_SPIKE.md](../notes/WINDOWS_NATIVE_SPIKE.md), "The double-down crash"). Draft for an issue against
`pshriwise/double-down`.

## Versions

| | Result |
| --- | --- |
| double-down 1.1.0, MSVC 19.51 (Visual Studio 18, toolset 14.51, GitHub `windows-latest` runner), Embree 4.3.3 | OpenMC 0.15.3 with a DAGMC model crashes in batch 1 with an access violation in `GeomQueryTool::RayHistory::in_history`, called from `backface_cull` |
| double-down as shipped in conda-forge `openmc016` (GCC 15.2 probe against the installed headers, Embree 4) | layouts agree; no crash |
| double-down `develop` at 437ad20 (2026-08-29) | the same declarations as 1.1.0 (`MOABRay.h` lines 29–66, `ray.h` line 20) |

## What happens

`MBDblTriIntersectFunc` (`src/MOABRay.cpp`) reads Embree's `args->rayhit` as `MBRayHit` and passes the same memory to
`DblTriIntersectFunc` (`src/primitives.cpp`), which reads it as `RTCDRayHit` and writes the hit there. `MBRayHit` is
`{MBRay ray; MBHit hit;}` and `RTCDRayHit` is `{RTCDRay ray; RTCDHit hit;}`. `MBRay` derives from `RTCDRay` and adds
`orientation` and `rh`; `MBRayAccumulate` adds `sum` and `num_hit`. So `hit` is at the same offset in both types only if
the added members take no extra space.

Under the Itanium C++ ABI (GCC, Clang on Linux and macOS) they take none: `RTCDRay` is not POD for layout purposes, and its
24 bytes of tail padding hold the four added members. On MSVC, which does not reuse a base class's tail padding, `hit` in
`MBRayHit` is 16 bytes further on. The hit written through `RTCDRayHit` lands on `rh`, and the next history lookup follows
a pointer whose upper 32 bits are zero.

Reading one object through two unrelated struct types is also undefined behaviour in C++ on every compiler; it works on
Linux because the layouts happen to coincide.

## Layout probe (Linux, GCC 15.2, headers from conda-forge `openmc016`)

```cpp
#include <cstddef>
#include <cstdio>
#include "double_down/MOABRay.h"
#include "double_down/ray.h"
using namespace double_down;
int main() {
  std::printf("sizeof RTCDRay %zu  sizeof MBRay %zu  sizeof MBRayAccumulate %zu\n",
              sizeof(RTCDRay), sizeof(MBRay), sizeof(MBRayAccumulate));
  std::printf("offsetof RTCDRayHit.hit %zu  MBRayHit.hit %zu  MBRayHitAccumulate.hit %zu\n",
              offsetof(RTCDRayHit, hit), offsetof(MBRayHit, hit), offsetof(MBRayHitAccumulate, hit));
  std::printf("offsetof MBRay.orientation %zu  MBRay.rh %zu  MBRayAccumulate.sum %zu num_hit %zu\n",
              offsetof(MBRay, orientation), offsetof(MBRay, rh), offsetof(MBRayAccumulate, sum),
              offsetof(MBRayAccumulate, num_hit));
}
```

`g++ -std=c++17 -w -DEMBREE4 -I$CONDA_PREFIX/include -I<eigen-3.4.0> probe.cpp` prints:

```text
sizeof RTCDRay 160  sizeof MBRay 160  sizeof MBRayAccumulate 160
offsetof RTCDRayHit.hit 160  MBRayHit.hit 160  MBRayHitAccumulate.hit 160
offsetof MBRay.orientation 136  MBRay.rh 144  MBRayAccumulate.sum 152 num_hit 156
```

The four added members sit at bytes 136–159, inside `RTCDRay`'s 160 bytes. One more 8-byte member in `MBRay` or
`MBRayAccumulate`, or a change that shrinks `RTCDRay`'s padding, would move `hit` on Linux too.

## Fix used by FARIS

`integrations/windows-spike/patches/double-down/0002-ray-struct-layout.patch`: the four members move into `RTCDRay`, so
`RTCDRay`, `MBRay` and `MBRayAccumulate` have one size on every compiler. With it, the Windows DAGMC smoke model gives the
same leakage fraction as Linux (0.65328 ± 0.00086) and matches it in all 21 tally bins. A `static_assert` that
`offsetof(MBRayHit, hit) == offsetof(RTCDRayHit, hit)` would catch a recurrence at compile time.
