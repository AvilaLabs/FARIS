# Reading the numbers

Every value in FARIS carries a badge that says what kind of number it is. Hover a badge to see why it applies and what would settle it.

## Kind labels

| Badge | Meaning |
| --- | --- |
| calculated | Calculated by the FARIS engine or a recorded solver run. |
| checked | A numerical control or check passed within its declared scope. |
| authored | An assumption written for this scenario. Tunable, not measured. |
| literature | Taken from cited literature. Citing a source does not qualify the value. |
| conditional | Valid only under stated conditions, such as the cold-data surrogate or the authored operating assumptions. |
| partial | A precision or coverage goal is not met. Use with care. |
| not evaluated | FARIS makes no claim either way. |
| failed | A check failed, or there was an error. |

The five you meet most are calculated, authored, literature, conditional and not evaluated. Transport results are *calculated*. Plant inputs, service limits, outage durations and efficiencies are *authored* or *literature*. Fuel, maintenance, replacements and electricity are *conditional* on them.

## Not evaluated explains itself

FARIS never leaves a bare unknown. Every value that is not evaluated, and every missing range, says why it is not evaluated and what the next step is. Hover shows it. It is also written as text on the screen and in exports.

A transport result carries the label "cold-data surrogate · NOT_EVALUATED". Scientific qualification is not evaluated, even for completed transport. [Scope and limits](scope.md) says why.

## Standard errors

Transport values appear as a mean with its Monte Carlo standard error, the sampling error of that one run. A smaller standard error means the run sampled that quantity better. It says nothing about whether the model is right.

The recorded runs have standard errors below 0.04 % for tritium production and heating. For the regional magnet fast flux they are 5 % to 24 %, and up to 30 % for the port-sector region of the port-free controls. Small regions are poorly sampled, which is why regional magnet results, and the replacement counts that follow from them, carry the most noise.

## The 2σ flags

In [Compare](compare.md), each transport difference carries a flag. FARIS compares the difference with 2·√(SE₁² + SE₂²). "Beyond 2σ sampling noise" means the difference exceeds that. "Within 2σ sampling noise" means it does not. The runs use different seeds and their covariance is not modelled, so this is a screening flag, not a significance test. It says nothing about nuclear data, geometry or model form.

## Ranges

Ensemble ranges are the median and the 5 to 95 per cent range (P5 to P95), with the share of samples for each event. They carry only the Monte Carlo sampling uncertainty of the transport. The true uncertainty is larger. See [Uncertainty ensembles](uncertainty.md).

## Service limits

Service limits, such as the magnet's 3 × 10²² n/m² fast fluence, are screening values from the literature or authored. They are not material allowables. The limit compares with a regional average, not a local peak.

## The research-screening statement

"Research screening only. These results are not a licensing, safety or design basis." is in the bottom bar of every step, on every PDF page, as the first line of every CSV, on every saved chart and in the export manifest.

Next: [Method](method.md).
