# ARC published geometry: sources for FARIS upgrade

Compiled 2026-10-05 by web research. "READ" = read by me in the source (PDF text or rendered figure). "SECOND" = secondhand (search-result snippet only).

Sources read in full or in the relevant sections:
- [S15] Sorbom et al., Fusion Eng. Des. 100 (2015) 378, arXiv:1409.3540 (open access, READ, full text; page numbers are arXiv PDF pages).
- [K18] Kuang et al., Fusion Eng. Des. 137 (2018) 221, arXiv:1809.10555 (open access, READ; section/figure/table cited).
- [Seg20] Segantin et al., Fusion Eng. Des. 154 (2020) 111531, MIT PSFC/JA-20-56 (open-access postprint, READ, model-description section only).
- [Boc20] Bocci et al., FED 2020, PSFC/JA-20-54 (downloaded; only grepped, no geometry beyond what Seg20 gives; activation focus).
Not obtained: any "ARC 2.0"/updated ARC design paper (none found in searches); Segantin et al. Energy Policy lifetime paper (cited by Boc20, not read); ORNL "ARC reactor neutronics multi-code validation" (title seen only, not read).

## 1. Plasma

| Quantity | Value | Source / location | Status |
|---|---|---|---|
| R0 | 3.3 m | S15 Table 1 (p.4), abstract | READ |
| Minor radius a | 1.13 m (Table 1, "plasma semi-minor radius"); abstract says "1.1 m" | S15 Table 1 / abstract | READ. Inconsistency: 1.13 vs 1.1 (rounding). FARIS 0.1 uses a = 1.0 m, which is not the published value. |
| Elongation kappa | 1.84 | S15 Table 1 (symbol kappa; not stated whether kappa95 or kappa_x; Table 7 inter-machine also lists 1.84) | READ |
| Elongation (K18) | 1.8 | K18 Sec 3.2 ("elongation of 1.8"), quoting S15 | READ. Inconsistency 1.84 vs 1.8 (rounded) |
| Triangularity delta | 0.375 (single value, "triangularity"; upper/lower not distinguished; K18 equilibrium is balanced double null so up = down) | K18 Sec 3.2 ("triangularity of 0.375 [1]") | READ. Not found as an explicit number in the S15 text I extracted (S15 text only says "ARC's triangular plasma cross section"); treat as K18's quote of S15 |
| Plasma volume | 141 m^3 | S15 Table 1 | READ |
| Plasma volume (K18) | 137 m^3 | K18 Sec 2 ("525 MW ... in a 137 m3 plasma volume") | READ. Inconsistency 141 vs 137 m^3 |
| Plasma current | 7.8 MA (S15 Table 1); 8.0 MA (K18 Sec 3.2) | as given | READ. Inconsistency |
| B0 / peak on-coil B | 9.2 T / 23 T (inboard midplane) | S15 Table 1; Sec 4.1 | READ |
| Fusion power | 525 MW (thermal total 708 MW) | S15 Table 1 | READ |
| Neutron wall loading | 2.5 MW/m^2 ("fusion power wall loading Pf/Sb") | S15 Table 1; K18 Sec 2 says "global areal power density ~2.5 MW/m^2" | READ. This is fusion power over plasma surface area, not a peak neutron wall load |
| Neutron source rate | 2.2e20 n/s (K18 Sec 4); 1.86e20 n/s (Seg20 Sec 2) | | READ. Inconsistency (525 MW / 17.6 MeV = 1.86e20 n/s; 2.2e20 not self-consistent with 525 MW) |
| Power density | ~3.7 MW/m^3 | K18 Sec 2 | READ |
| X-points (K18 equilibrium) | primary X-points at R = 2.9 m; divertor (secondary) X-points at R = 3.7 m; flux expansion 1.3 | K18 Sec 3.2 | READ |

Sanity check on a: S15 Fig. 2 puts the plasma/SOL boundary near 223 cm and R0 at 330 cm, implying about 107 cm half-width (a = 1.07 m at the midplane inboard), versus Table 1's 1.13 m. Mild internal inconsistency (about 6 cm), possibly SOL/separatrix definition. Flagged.

## 2. Radial build

### Inboard midplane, S15 Fig. 2 (p.5, "The ARC reactor inboard radial build"), READ from the rendered figure
Axis ticks are radii from the machine axis (R) in cm. My assignment of labels to intervals is by reading the figure (tick marks vs. labels); the interval boundaries are the printed numbers, the layer-to-interval mapping is my interpretation (the figure uses leader lines).

| R interval (cm) | Component (my mapping) | Thickness |
|---|---|---|
| 0 - 45 | Composite (glass-filled epoxy) plug; Sec 4.2 says vertical axis area R = 0-0.45 m is epoxy plug | 45 cm |
| 45 - 70 | Central solenoid & bucking cylinder | 25 cm |
| 70 - 134 | TF coils (inboard leg: winding pack + case combined, no split given) | 64 cm radial |
| 134 - 135 | Vacuum gaps | 1 cm |
| 135 - 186 | Neutron shielding (TiH2) | 51 cm |
| 186 - 189 | Thermal shielding (aluminum silicate; Sec 5.4 "remaining 2 cm of inboard radial build allocated for thermal insulation" of the shield, i.e. approx. 2-3 cm) | 3 cm |
| 189 - 192 | Blanket tank (wall) | 3 cm |
| 192 - 212 | Bulk blanket (FLiBe) | 20 cm |
| 212 - 220 | Vacuum vessel (double wall incl. channel, see below) | 8 cm |
| 220 - 223 | Scrape-off layer | 3 cm |
| 223 - 330 | Plasma (to R0) | |

Cross-checks: S15 Sec 3.1 (text near Eq. 11) gives minimum inboard blanket thickness "Delta_b ~ 0.5 m" (0-D estimate; final design differs, S15 says so). K18 Sec 1 says ARC "has a 1-meter-thick FLiBe blanket surrounding its plasma core" (outboard/overall; 1 m also in Seg20). Inboard FLiBe in Fig. 2 is only ~20 cm plus the shield 51 cm and thin tank walls; the shield plus FLiBe totals ~0.8 m. Interpretation caution: "0.5 m" and "20 cm FLiBe + 51 cm TiH2" are not directly reconcilable; the 0-D value is superseded.

### Vacuum vessel layers (double-walled), READ
- S15 Sec 5.4.3 (p.25): 1 cm tungsten tiles on 1 cm inner VV (Inconel 718); 2 cm FLiBe channel; non-structural 1 cm Be multiplier; 3 cm outer VV (Inconel 718). Sum = 8 cm (consistent with Fig. 2's 212-220).
- S15 Sec 5.1 (p.23, neutronics model): first wall modeled as 1 cm W on 1 cm Inconel 718. Sec 5.3: "ARC design uses a 1 cm tungsten first wall to achieve TBR of 1.1". S15 Sec 5.3 also says ARC has "~5 cm of vacuum vessel" (structural material only).
- Seg20 Sec 2: 0.1 cm tungsten first wall (!), 1 cm Inconel (STR1), 2 cm FLiBe, 1 cm Be, 3 cm Inconel (STR2), about 1 m bulk FLiBe in tank. Inconsistency: W thickness 0.1 cm (Seg20, "a choice of the last ARC") vs 1 cm (S15). K18 Fig. 9 radial build used for MCNP: 1 cm Be on outer VV (consistent); K18 Sec 5 uses a minimum first wall 3 mm, 4 cm Inconel backbone for the divertor/first wall thermal design (different from the 8 cm VV).
- Tank/thermal: S15 Sec 4.4 lists three vacuum gaps between the TF coils and the neutron shield with thermal shielding (aluminum silicate); exact individual gap widths NOT given in text (Fig. 2 shows only a combined 1 cm).

### Outboard
- NOT found as a table. Outboard FLiBe thickness "~1 m" (Seg20; K18 Sec 1, text). S15 Fig. 27 (MCNP axisymmetric geometry) and Fig. 1/ Fig. 17 show the outboard geometry graphically only; no outboard thickness numbers in extracted text. S15 Sec 5.2 says the TiH2 shield has "additional thickness on the inboard side", so outboard shield is thinner than 51 cm; its value is a GAP.
- K18 Fig. 1 gives a scaled drawing (scale bar 1 m) of the FLiBe tank, VV and TF; I did not digitize it. Digitizing is possible if you need the outboard build (I viewed the page image: the TF outer envelope is elliptical/D, with the tank hugging the VV outline).

### Tank/coil gaps outboard: GAP.

## 3. TF coils

| Quantity | Value | Source | Status |
|---|---|---|---|
| Number | 18 demountable coils | S15 Sec 4.2 (p.17); also Sec 2 "18 support columns ... evenly spaced between the 18 TF coils"; K18 Sec 5 "between the 18 toroidal field coils" | READ |
| Shape | constant-tension Princeton D-shape | S15 Sec 4.2 | READ |
| Structure | stainless steel 316LN case; max stress 660 MPa (Fig. 24) | S15 Sec 4.2 | READ |
| Winding pack | 120 jacketed CICC cables, each 70 kA; 8.4 MA net per coil for 9.2 T; WP composition 45.9% Cu, 46.1% steel, 8.0% REBCO; WP current density 44 A/mm^2; 15 layers (Fig. 20) | S15 Sec 4.2, 4.2.1 | READ |
| Cable jacket | 40 mm x 40 mm square steel jacket | S15 Sec 4.2.1 | READ |
| Insulation | ~2 mm (Kapton + S2 glass) | S15 Sec 4.2.1 | READ |
| Inboard leg radial extent | R = 70 to 134 cm, i.e. 64 cm total (case + WP) | S15 Fig. 2 | READ from figure. Split between winding pack and case NOT stated in text. A 15-layer WP of 40 mm jackets would be ~60 cm radially, which fits 64 cm but this is my inference, not a published split. |
| Toroidal width of inboard leg; case wall thickness | NOT STATED | | GAP |
| Joints | demountable TF: removable upper leg + stationary lower leg; joints at the outer midplane (bolted, "outer joint") and at the top (steel tension ring); comb-style electrical joints, 240 sets in series per coil; each tooth 150 mm long | S15 Sec 4.2 (p.17), Sec 4.2.2, Fig. 17; K18 Fig. 1 labels "Demountable TF Joint" at upper inboard-top and outer midplane | READ. Note: S15 Sec 4.2 says "outer midplane and the top"; K18 Fig 1 draws the upper joint at the top of the inboard side. Outer-leg radius / overall coil height: not stated as numbers. |
| Alternative considered | "picture-frame" coils (S15 Fig. 26) | S15 Sec 4.5 | READ |
| Overall height and outer-leg radius | NOT STATED numerically in S15 text | | GAP. From K18 Fig. 1 scale bar (1 m), the TF outer envelope is roughly 9 m tall and its outer extent roughly 7 m from the axis (my eye estimate from a 110 dpi render; low confidence, treat as +/-0.5 m). K18 Table 1 PF coil positions give outer vertical-field coils at R = 6.13 and 6.57 m, i.e. outside the TF outer leg at about R of 6 m or less. |
| Cooling | 20 K, liquid H2 considered | S15 Sec 4.2, 4.4 | READ |

## 4. Divertor

- Original ARC (S15): divertor design deliberately left open. Neutronics model: "8 cm thick layer of tungsten covering 17% of the lower vacuum vessel surface area" (S15 Sec 5.1, p.23). So S15 geometry is single-null-like (lower only) for neutronics; S15 Fig. 1 caption: "physical divertor design was left for later study".
- Updated (K18): balanced double null; long-leg X-point target (XPT) divertors in upper and lower legs; achieved by "carving out an appropriate space in the FLiBe blanket", within the original TF envelope; core elongation/triangularity unchanged (1.8 / 0.375) (K18 abstract; Sec 1; Sec 3.2; Fig. 1). Divertor PF coils PF1-PF3 (REBCO) inside the TF, outside the FLiBe tank/VV: Table 1 gives R, Z (m): PF1 (1.80, 3.03), PF2 (4.85, 2.70), PF3 (4.85, 2.10); vertical field coils PF4 (6.13, 2.78) and PF5 (6.57, 1.70); each width 0.20 m, divertor-coil height 0.45 m (upper set; lower mirrored, Fig. 1 shows PF1L-PF3L). Currents 3.9, 5.2, -4.4 MA (PF1, PF2, PF3). READ.
- Divertor X-point separation: primary at R = 2.9 m, secondary at R = 3.7 m (K18 Sec 3.2). Plasma-wall gap ~20 x heat-flux width (8 mm at outer midplane) (K18 Sec 3.2).
- Divertor neutron shielding: ~1 m of FLiBe on line of sight to the divertor; flux at divertor target is ~30 times lower than outer midplane (K18 Sec 4.1, Fig. 8); 25 cm thick ZrH2 plates in front of PF1, PF2, PF3 (K18 Sec 4.2, Fig. 7). Tally text in the abstract says divertor targets are "~3-30 times lower" than first wall damage (K18 abstract).
- Divertor leg length (numeric): NOT STATED in text; only drawn in K18 Fig. 1 (1 m scale bar). Outer divertor foot appears to extend to roughly R about 4.9 m region near PF2/PF3 in the figure (inferred from coil placement in Table 1), low confidence.

## 5. Ports

- No port count/size numbers found. S15 Sec 2: the vessel hangs from the blanket tank by 18 curved support columns evenly spaced between the 18 TF coils; "All connections needed for in-vessel components (waveguides, vacuum ports, etc.) run through these columns", curved to limit neutron streaming. Sec 4 (RF section): launcher access through the hollow posts. K18 Sec 5.5/Table 11: FLiBe coolant pipes (Inconel 718, 10 mm walls, ~2 m/s) enter/exit tank vertically between the 18 TF coils; their area is ~5% of the usable tank area between TF coils ("~1.5 m^2 of ~30 m^2"). So the available port area between TF coils is about 30 m^2 total (about 1.7 m^2 per gap on average), with ~1.5 m^2 used by pipes. READ.
- LHCD: two inboard launchers (25 MW), ICRF ~13 MW antennae occupy "a very small fraction of the first wall" (S15 Sec 3, p.16). READ.
- Individual port dimensions: GAP.

## 6. Fluence and lifetime

| Item | Value | Source | Status |
|---|---|---|---|
| Fluence limit | 3e18 n/cm^2, neutrons > 0.1 MeV; onset of Nb3Sn critical-current degradation, "lower bound", REBCO expected at least as good | S15 Sec 5.2 (p.23); K18 Sec 4.2 | READ |
| TF lifetime | 9 FPY to reach 3e18 n/cm^2 "in any part of the magnet" after adding TiH2 shield (S15 Sec 5.2; Sec 2 says "at least 9 FPY"). Region: any part of the TF coil (not stated as inboard-leg specifically; Sec 2 says the shield protects the inboard leg "particularly space constrained") | S15 | READ |
| TF neutron flux reduction | blanket + shield reduce flux to the TF by a factor of 9e-5 | S15 Sec 2 (p.3); MIT snippet matches | READ |
| Scaling | small major-radius increase greatly cuts yearly TF fluence (S15 Fig. 28, using extra length for TiH2) | S15 | READ (figure not digitized) |
| K18 design target | PF/TF coil lifetime target 10 FPY | K18 Sec 4.2 | READ |
| PF coil lifetimes, 3e18 limit | PF1 2.32 -> 12.5 FPY; PF2 6.82 -> 76.2; PF3 1.10 -> 11.3 (without -> with 25 cm ZrH2) | K18 Table 6 | READ |
| TBR | >= 1.1 (S15 target, 90% 6Li); K18 final 1.08 +/- 0.004; Seg20 reference about 1.07 | | READ. Inconsistency 1.1 vs 1.08 vs 1.07 across papers (design target vs recomputed) |
| VV replacement | 1-2 FPY for the vessel | K18 abstract | READ |

## Key inconsistencies (resolved in docs/DEMO_ROADMAP.md, "First step: a more realistic transport geometry")
1. a: 1.13 m (S15 Table 1) vs 1.1 (abstract) vs about 1.07 m implied by S15 Fig. 2 (R0 - 223 cm). FARIS 0.1 uses 1.0 m.
2. kappa 1.84 vs 1.8; plasma volume 141 vs 137 m^3; Ip 7.8 vs 8.0 MA.
3. First-wall tungsten 1 cm (S15) vs 0.1 cm (Seg20); K18 uses 3 mm for thermal design.
4. Inboard FLiBe: 0-D estimate Delta_b = 0.5 m vs Fig. 2 final build (FLiBe 20 cm + TiH2 51 cm); ~1 m FLiBe refers to the general immersion blanket.
5. Neutron source 2.2e20 (K18) vs 1.86e20 n/s (Seg20, physically consistent with 525 MW).
6. Delta: only K18 states 0.375 (secondhand quote of S15). I did not locate the number in S15's own text.

## Gaps (nothing published that I found)
Outboard radial build; TF case/winding-pack split and toroidal width; TF height and outer leg radius (only graphic); upper vs lower triangularity separately; individual port dimensions and count; individual vacuum-gap widths; divertor leg length; Segantin/Bocci contain no extra geometry beyond the VV layers; ARC 2.0 paper not found.

## Paywall notes
S15 and K18 read via arXiv (open). Seg20 and Boc20 read via MIT PSFC open postprints. ScienceDirect versions paywalled and not used.
