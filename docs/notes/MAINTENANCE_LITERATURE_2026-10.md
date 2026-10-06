# Literature: maintenance downtime and activation, 2026-10

Recorded 2026-10-06 for the maintenance coupling validation (`docs/notes/MAINTENANCE_COUPLING_VALIDATION.md`). Three
web reviews were run on: published maintenance durations and cooldown rules; how systems codes and availability
studies set maintenance time; and how shutdown dose rate and decay heat change over a plant's life. Statements marked
**checked** were read in the source on 2026-10-06. Everything else is as reported by the reviews, and is to be checked
before it is used for anything.

## The question that matters: has the whole-life loop been done?

No published example was found of a loop where activation-derived cooldown sets outage lengths, the outages change the
irradiation history, and the two are iterated to consistency. What exists is one-way: a schedule is fixed first,
activation is computed from it, and the dose is used to judge access.

- **Checked.** PROCESS (`process/models/availability.py` at `a199a00`, 2026-10-05): maintenance duration is a fixed
  input (models 1 and 3) or a function of the number of remote-handling systems (model 2), with a constant one-month
  cooldown ("the +2.0 at the end is for the 1 month cooldown and pump down at either end"). Fluence and damage set
  how often outages happen, not how long they last.
- **Checked.** bluemira (`bluemira/fuel_cycle/lifecycle.py` at `6a972a8`, 2026-10-05): blanket and divertor
  maintenance durations are fixed inputs, 150 and 90 days by default ("Full replacement intervention duration").
  Availability varies over life only through learning-curve strategies and a statistical outage distribution.
- **Checked.** EU-DEMO maintenance duration estimate (Crofts and Harman, Fusion Eng. Des. 89 (2014),
  arXiv:1412.4008): bottom-up handling times with an assumed one-month cooling period.
- Reported, not checked: the UKAEA Maintenance Duration Estimator (30-day fixed cooldown), the PAMPAS availability
  model, FUSE (maintenance modelling "planned but not yet implemented"), and economic studies by Schwartz et al.
  (arXiv:2405.01514), all with fixed or sampled durations. SYCOMORE, FRESCO and non-English literature were not
  covered.

Confidence: the review that searched for this rated it medium-high. Paywalled papers, internal reports and private
tools could hold such a loop without being visible to a web search.

## Does activation at later outages grow over the plant's life?

No published study was found that reports shutdown dose rate or decay heat at later outages against earlier ones for
components that are never replaced. Indirect support only:

- **Checked.** Palermo et al., EU DCLL DEMO shutdown dose rate (EUROfusion preprint WPBB-PR(17) 17590): decay times
  of 1 day, 12 days and 1 year; at 12 days the dose is dominated by Co-58, Co-60, Mn-54, Ta-182 and Fe-59; component
  replacement was disregarded in that study.
- Reported, not checked: a 2026 Applied Sciences paper on DEMO in-vessel dose rate (doi 10.3390/app16041983)
  attributing dose increases from phase to phase to accumulation of Mn-54 and Co-60; a statement in the Yinsen study
  (arXiv:2605.04190) that earlier points in vessel life would give lower decay heat and dose, with no values.

## Published durations and cooldown rules

- **Checked.** Crofts and Harman, Table 1: all EU-DEMO blankets and divertor cassettes take 22, 11, 5.9, 4.4 and
  3.2 months with 1, 2, 4, 6 and 8 remote-handling systems; one month of cooling before handling and one month of
  conditioning and pump-down after; a 62-month cycle with 77 % availability.
- **Checked.** ARIES-AT (Waganer, maintenance system report): about 24 hours for cooldown and for radiation inside
  the power core to fall, about 8 days of scheduled maintenance per year.
- **Checked.** Frosi et al., EUROfusion WPPMI-CPR(18) 20248: blanket temperatures evaluated with decay heat one month
  after shutdown, against a 100 °C limit at the remote-handling interface. It does not give the 15-day and 85-day
  cooling figures that the coupling test's protocol attributed to it (corrected in that protocol's Amendment 3).
- Reported, not checked: ITER replacement requirements (6 months for all divertor cassettes, 2 months for one, 8
  weeks for a blanket module); STEP magnet access (about 2 weeks to de-energise, 1 month to warm, 3 months to open);
  the ARC vessel replaced every 1 to 2 full-power years, with a press-interview figure of "a couple of months at
  most" for the swap; EU-DEMO hands-on dose classes (1, 10 and 100 µSv/h and 5 mSv/h).

## Which limit decides when handling starts?

No source settles it. Gamma dose to the remote-handling equipment looks like the practical limit for in-vessel work,
and decay heat the limit for moving and storing removed components (reported: active cooling of removed components
for up to 18 months, Loving et al., arXiv:1309.7194). The validation tests both: decay heat in the main runs, contact
gamma dose in variant V1.

## What this means for the claim

What fixed-duration models cannot show is a cooldown that grows with plant age and differs between designs, and the
resulting change in design answers. The codes that were checked fix the duration. No published study was found that
shows the age trend directly, and none that iterates the schedule. The claim to make is narrow: as far as an open
literature search shows, FARIS is the first design tool to compute maintenance durations from activation over the
whole life and to show that this changes design rankings. It stands until a paywalled or internal source shows
otherwise.
