# Impurity specification sources for FARIS surrogate materials

Compiled 2026-10-05 from public web sources for the FARIS 0.2 activation plan (docs/DEMO_ROADMAP.md, "Decisions for 0.2"). No numbers invented. Each table is tagged:
- PRIMARY-TEXT = I read the numbers in a copy of the document itself (local PDF text extraction).
- SECONDARY = vendor sheet or paper quoting the standard.
- SEARCH-SNIPPET = value reported only by a web-search summary, not confirmed against document text by me. Treat as UNVERIFIED until checked.
"Silent" means the source's table has no row for that element; it does not mean the content is zero.

Co / Nb / Ag / U coverage summary (the activation-critical ones)
| Material | Co | Nb | Ag | U |
|---|---|---|---|---|
| W (ASTM B760 / ITER W spec) | silent | silent | silent | silent |
| W (Plansee ITER-grade vendor guarantee, SEARCH-SNIPPET) | 10 ug/g | 10 ug/g | 10 ug/g | not reported |
| FLiBe (MSRE Table 4) | silent | silent | silent | silent (Cd, rare earths total, Zr, B are limited) |
| TiH2 (Albemarle U/P) | silent | silent | silent | silent |
| Ti metal (ASTM B348 Gr 1/2) | silent (Co only listed as a column for other grades, "--" for Gr 1/2) | silent (column, "--") | silent | silent |
| Pure iron (ARMCO Grade 2 / 4) | 0.005 wt % max | silent | silent | silent |
| Iron (ASTM A848) | numeric table not obtained | - | - | - |
| OFE Cu (ASTM B170 Gr 1) | silent | silent | 25 ppm (SEARCH-SNIPPET) | silent |

---

## 1. First wall: natural tungsten

### 1a. ITER W Material Specification (based on ASTM B760) - SECONDARY quote
- Issuing body: ITER Organization ("ITER Material Specification for W"; the controlled document itself was not found public).
- Quoted in: T. Hirai, "ITER Tungsten divertor and status", ICFRM-17, Aachen, 11-16 Oct 2015, slide 13. https://amdis.iaea.org/media/presentations/F43021-RCM1-Hirai.pdf (also https://pdfs.semanticscholar.org/634a/8ce6882b0822a6a7b6313b0139487bf8b2a4.pdf; text read locally).
- Content quoted: min W 99.94 wt %; max impurity content (C, O, N, Fe, Ni, Si) 0.01 wt % each; density >= 19.0 g/cm3; HV30 >= 410.
- Note: the ITER spec is therefore the B760 chemistry. It limits only C, O, N, Fe, Ni, Si. Silent on Co, Nb, Ag, U, Mo, Ta, K.

### 1b. ASTM B760-07 "Tungsten Plate, Sheet, and Foil" - PRIMARY-TEXT
- Issuing body: ASTM International (Committee B10, B10.04). Edition read: B 760-07, approved 1 Dec 2007 (reapproved 2019, 2023 editions exist; I read the 2007 copy). Standard page: https://www.astm.org/Standards/B760.htm (paywalled; the table was read from a publicly hosted full-text copy).
- Table 1, heat analysis, max wt %:

| Element | Max (wt %) | Check-analysis tolerance |
|---|---|---|
| C | 0.010 | +/-0.002 |
| O | 0.010 (reported for information only when sampled from powder blend, section 6.2.2) | +10 % relative |
| N | 0.010 | +/-0.0005 |
| Fe | 0.010 | +/-0.001 |
| Ni | 0.010 | +/-0.001 |
| Si | 0.010 | +/-0.001 |

- Silent on everything else (Co, Nb, Ag, U, Mo, Ta, K, Cl, N is covered only as above). The specified total is implied by W >= 99.94 in the ITER spec, not in B760 Table 1.

### 1c. Plansee "ITER specification" W bar - SECONDARY (vendor limits quoted in papers)
- Source A (read directly): PMC8027420, "New insights into microstructure of neutron-irradiated tungsten" (Sci Rep 2021), https://pmc.ncbi.nlm.nih.gov/articles/PMC8027420/ . States the manufacturer (PLANSEE SE) lists upper-limit concentrations (typical in parentheses): C 30 (6), O 20 (2), N 5 (1), Fe 30 (8), Ni 20 (2), Si 20 (1), all ug/g.
- Source B (SEARCH-SNIPPET, UNVERIFIED): a ResearchGate table "List of impurities in 99.97 wt% pure tungsten manufactured by Plansee" (https://www.researchgate.net/figure/List-of-impurities-in-9997-wt-pure-tungsten-manufactured-by-Plansee_tbl2_383265549 , from the MDPI paper doi:10.3390/met15020172) was reported by a search summary as having guaranteed (typical) ug/g: Ag 10 (<5), Co 10 (<2), Cu 10 (<5), Fe 30 (10), K 10 (5), Nb 10 (<5); Mo 100 guaranteed; purity 99.97 wt % excluding Mo. Both pages returned 403 to me, so I could not read the table. Verify before use. Other values seen in snippets (Ta 20, Cr 20, P 20, S 5, H 5) are also unverified.
- Use suggestion: the only citable specification ceiling for Co/Nb/Ag in W is the vendor guarantee in 1c-B, once verified; ITER/B760 do not limit them.

---

## 2. Blanket: FLiBe (2LiF-BeF2)

### MSRE Table 4 "General chemical specifications" - PRIMARY-TEXT (a specification, not measured)
- Document: J. H. Shaffer, "Preparation and Handling of Salt Mixtures for the Molten Salt Reactor Experiment", ORNL-4616 (UC-80), Reactor Chemistry Division, Oak Ridge National Laboratory, January 1979 issue (covers 1965-1969 work). https://moltensalt.org/references/static/downloads/pdf/ORNL-4616.pdf (text read locally; OCR rendered S as "0.02s", taken as 0.025, which matches a second search report of the same table).
- Stated scope: maximum allowable impurities in the fluorides (LiF, BeF2, ZrF4 etc.) purchased for the MSRE. "Determined on a best commercially available BeF2 basis"; some limits were exceeded in practice (Fe 250 ppm in BeF2 and 500 ppm in LiF were allowed). It is a feed-salt specification for MSRE salts, not a FLiBe-specific commercial grade.
- Units as stated: wt % (1 ppm = 0.0001 wt %).

| Impurity | Allowable max (wt %) |
|---|---|
| Water | 0.1 |
| Cu | 0.005 |
| Fe | 0.01 |
| Ni | 0.0025 |
| S | 0.025 (OCR "0.02s") |
| Cr | 0.0025 |
| Al | 0.015 |
| Si | 0.01 |
| B | 0.0005 |
| Na | 0.05 |
| Ca | 0.01 |
| Mg | 0.01 |
| K | 0.01 |
| Li (natural, i.e. 6Li poison) | 0.005 |
| Zr (natural) | 0.025 |
| Cd | 0.001 |
| Rare earths (total) | 0.001 |

- Silent on Co, Nb, Ag, U, Mo, Cl, N, and on oxygen (water is limited instead).
- Other (measured/targets, not specification): oxygen and moisture below 1 and 2 ppm in some experimental FLiBe preparations (search summary, OSTI 1801197; not verified).

---

## 3. Shield: titanium hydride (TiH2)

### 3a. Albemarle Titanium Hydride Grade U and Grade P - manufacturer product spec (vendor, not a standard), read from product pages
- Issuing body: Albemarle Corp (vendor product specification; not a standard). Read 2026-10-05.
  - Grade U: https://www.albemarle.com/us/en/product/titanium-hydride-grade-u
  - Grade P: https://www.albemarle.com/us/en/product/titanium-hydride-grade-p

| Item | Grade U | Grade P |
|---|---|---|
| Ti total | min 95 % | min 95 % |
| Hydrogen | min 3.8 % | min 3.8 % |
| Fe | max 0.09 % | - |
| Cl | max 0.06 % | - |
| Ni | max 0.05 % | - |
| Si | max 0.15 % | - |
| N | - | max 0.3 % |
| Mg | - | max 0.04 % |

- The two grades list different elements. Silent on C, O, Co, Nb, Ag, U. Note Ti min 95 % leaves up to ~5 % unspecified balance (hydrogen is ~4 %), so stacking U and P limits is a judgment call for FARIS.

### 3b. Other vendor TiH2 sheets (typical or "max" per vendor; weaker provenance, from search summaries only, UNVERIFIED)
- Samaterials HD2807 (https://www.samaterials.com/hydride/2807-titanium-hydride-tih2.html): H 4.2, O 0.12, N 0.025, C 0.02, Fe 0.03 (%, spec vs typical not confirmed).
- US Nano TiH2 99.5 % (https://www.us-nano.com/inc/sdetail/42382): O 0.5, N 0.006, P 0.001, Fe 0.05 (%).
- No consensus ASTM standard for TiH2 powder was found (ASTM B299 covers Ti sponge, not the hydride; not retrieved).

### 3c. Fallback for Ti metal: ASTM B348/B348M (ASME SB-348, 2013 Section II Part B copy) - PRIMARY-TEXT
- Issuing body: ASTM International / ASME. Table 1 read from a publicly hosted copy of the ASME edition; wt % max.

| Element | Grade 1 | Grade 2 |
|---|---|---|
| C | 0.08 | 0.08 |
| O | 0.18 | 0.25 |
| N | 0.03 | 0.03 |
| H | 0.015 | 0.015 |
| Fe | 0.20 | 0.30 |
| Other elements, each | 0.1 | 0.1 |
| Other elements, total | 0.4 | 0.4 |

- Ni, Co, Mo, Cr, Zr, Nb, Sn, Si, Al, V appear only as alloy columns with "--" (no limit) for Gr 1/2. So Co/Nb/Ag/U are covered only by the "other elements 0.1 each / 0.4 total" catch-all (each element at most 0.1 wt %, residuals total 0.4 wt %). Mg, Cl not limited (Cl, Mg limits belong to Ti sponge/hydride specs).

---

## 4. Vacuum vessel: pure iron

### 4a. ARMCO Pure Iron Grade 2 and Grade 4 - SECONDARY (producer brochure, read directly)
- Issuing body: AK Steel International B.V. (ARMCO; now Cleveland-Cliffs). "ARMCO Pure Iron and Specialty Stainless Steels" brochure, undated (c. 2019-2020), p. 13. https://wlw-1-company-facts-media20191122174836234900000006.s3.eu-central-1.amazonaws.com/a7d323ca-1fdd-47b3-bcf6-3702295a0b24.pdf (text read locally). Basis: max analysis, wt %. This is a producer grade spec, not an ASTM/ISO standard.

| Element | Grade 2 max (wt %) | Grade 4 max (wt %) |
|---|---|---|
| C | 0.010 | 0.010 |
| Mn | 0.100 | 0.060 |
| P | 0.010 | 0.005 |
| S | 0.008 | 0.003 |
| N | 0.006 | 0.005 |
| Cu | 0.030 | 0.030 |
| Co | 0.005 | 0.005 |
| Sn | 0.010 | 0.005 |

- Silent on Nb, Ag, U, Ni, Cr, Mo, Si, Al. (Co limit is explicitly present, a useful point for activation.)

### 4b. ASTM A848 "Low-Carbon Magnetic Iron" (Type 1 low-P, Type 2 P-added) - NUMBERS NOT OBTAINED
- Issuing body: ASTM International; current edition A848-17 (earlier A848-01, reapproved 2011). Paywalled (https://store.astm.org/a0848-17.html). Public abstract (https://kikakumaster.com/astm/astm-a848/): C <= 0.015 %, remainder substantially iron; Table 1 chemistry covers C, Mn, Si, P, S, Cr, Ni, V, Ti, Al, Fe.
- Search-summary values for Type 1 conflicted between queries (e.g. C 0.020 / Mn 0.35 / Si 0.15 / P 0.030 / S 0.025 / Cr 0.20 / Ni 0.15 in one, other vendors quoting different numbers), so I do not report any of them. Obtain the standard text (or a vendor copy of Table 1) before citing A848. It does not list Co, Nb, Ag, U per its abstract.

---

## 5. Magnets: oxygen-free copper

### ASTM B170 Grade 1 (UNS C10100) - SEARCH-SNIPPET, not confirmed against document text
- Issuing body: ASTM International, B170 "Oxygen-Free Electrolytic Copper - Refinery Shapes" (B170-99, reapproved 2015 and 2020). Paywalled: https://store.astm.org/b0170-99r20.html . ASTM F68 (OFE copper for electron devices) invokes the B170 Grade 1 chemistry.
- I could not read the table directly (copper.org returned 403; other pages show only "Cu 99.99 min, O 0.0005 %"). Two independent web-search summaries returned the same list, and it matches the form of the published B170 Grade 1 table:

| Element | Max (ppm by weight) |
|---|---|
| Cu (counting Ag as impurity) | min 99.99 % |
| Sb | 4 |
| As | 5 |
| Bi | 1 |
| Cd | 1 |
| Fe | 10 |
| Pb | 5 |
| Mn | 0.5 |
| Ni | 10 |
| O | 5 |
| P | 3 |
| Se | 3 |
| Ag | 25 |
| S | 15 |
| Te | 2 |
| Sn | 2 |
| Zn | 1 |

- Also reported: Sb + Se + As + Te + Bi + Sn + Mn total <= 40 ppm (unverified detail).
- Grade 2 (UNS C10200): O <= 10 ppm (0.0010 %), Cu >= 99.95 % with Ag counted as Cu (public abstract).
- Silent on Co, Nb, U, Mo, K, Cl, N (not in the list above).
- Action needed: confirm this table against the ASTM text, Copper Development Association (alloys.copper.org/alloy/C10100) or a vendor datasheet citing B170 before it goes into the authored impurity list.

---

## Honest gaps
1. Primary-text verified: ASTM B760-07 (W), ITER W spec (via Hirai slide), MSRE ORNL-4616 Table 4 (FLiBe-feed), ASTM B348 (Ti metal), ARMCO Grade 2/4 brochure, Albemarle TiH2 U/P pages, Plansee C/O/N/Fe/Ni/Si limits via PMC8027420.
2. Unverified: Plansee Ag/Co/Nb/K/Cu 10 ug/g guarantees; B170 Grade 1 element table (consistent across two search summaries, 403 on the direct sources); every other-vendor TiH2 number.
3. Not found: A848 numeric table; any public commercial FLiBe grade spec (MSRE Table 4 is the only stated-limit document found; ARC/other FLiBe purity targets were not located as limits); any spec that limits U in any of the five materials.
4. The ITER controlled specification document itself (ITER IDM) was not found public.
