<!-- kind: reference -->
# Units and number formats

FARIS keeps every stored value in SI base units (metres, kilograms, seconds, kelvin, joules, becquerel) with a decimal point and ISO dates. What you see and type can be in other units and formats. Changing them changes only the text on screen or in a printed report: a saved study, a result file and every result hash stay exactly as they were.

## What you can choose

- **A unit system**, `units.system`: `si` (the default) or `fusion`. The fusion practical set shows power in MW, particle energy in keV, length in cm, plant energy in GWd, displacement damage in dpa, heat flux in MW/m2, power density in W/cm3, particle flux in n/cm2/s, fluence in n/cm2, damage rate in dpa/FPY, dose rate in Sv/h, mass density in g/cm3, burn-up in MWd/kg, impurity fractions in appm, outage durations in days and time in service in years.
- **A unit per quantity**, `units.<group>` (for example `units.energy_particle=MeV`). It beats the system for that quantity. `system` means follow `units.system`. A unit is accepted by symbol or by an alias (`MW/m^2`, `MW/m²`, `µSv/h`); it is stored under its symbol.
- **Number and date formats**, `format.decimal` (`point` or `comma`), `format.grouping` (`none`, `thin-space`, `comma`, `point`, `apostrophe`; used from five integer digits up, and never the same character as the decimal separator) and `format.date` (`iso`, `dmy`, `mdy`).

Set them with `faris config set KEY VALUE`, for one run with `--setting KEY=VALUE`, or the unit system alone with `--units si|fusion` on any command. The environment variable `FARIS_UNITS` sets the system. Handbook table of every key: [Settings](settings.md).

## Commands that honour them

- `faris maintenance report` (markdown) and the one-line summaries of `faris maintenance run`: durations, times in service, plant energy and the cooldown threshold q* follow the unit choices; numbers and the availability and ratio columns follow the number format. With no choice made the report prints what it always has (days, years, MWh, W/m3), so a default run is unchanged; a chosen unit is printed with four significant digits. The JSON form of the report is the stored data and is never converted.
- `faris config list` and `faris config get` show the settings themselves.

Not converted yet: the other commands print file paths, hashes and verdicts, or JSON that is stored data; the desktop panels (the desktop calls the functions listed below); the CSV and PDF exports, which stay in the units named in their column headers.

## Entering a quantity

A typed quantity is a number and, optionally, a unit: `30 cm`, `14.1 MeV`, `1e14 n/cm²/s`. A bare number is taken in the unit currently shown for that quantity. A unit of the right kind is converted; a unit of another kind is refused with the cause and the units that would be accepted, for example `5 MeV` where a length is expected. Units that look alike but mean different things are kept apart: `Sv` does not convert to `Gy`, and `dpa` does not convert to a plain fraction or to `appm`.

The number is read in the chosen format (a decimal comma under `format.decimal=comma`); a pasted point is also accepted unless it is the grouping character.

## Exact conversions

Each unit is defined once, in `crates/faris-model/src/units.rs`, by its exponents of the SI base quantities and an exact factor written as a decimal fraction (so 1 eV is held as 1602176634 x 10^-28 J, the number the 2019 SI defines, not a rounded float). A conversion multiplies by one factor. Round trips agree to far better than the 1e-12 relative error the tests demand, for every pair of units of the same kind. The degree Celsius is the one offset unit; its round trip is measured against the size of the values it adds, because the same point is 0 degC and 273.15 K. No other crate carries its own copy of a conversion factor: a test scans the calculation crates and fails on a second copy of the seconds per day, the Julian year, the electronvolt or the other registry factors.

Sources: the SI Brochure (BIPM, 9th edition, 2019), NIST Special Publication 811 (2008 edition, Appendix B) for the units outside the SI (curie, rem, rad, barn, bar, atmosphere, gauss), and FARIS definitions where a convention is fixed here:

- **dpa** counts displacements per lattice atom and has its own axis; it never converts to a fraction.
- **appm** is atoms of a species per million atoms of the material: 1e-6 as a fraction.
- **Year** (`yr`) is the Julian year of 365.25 days, 31 557 600 s. **FPY**, the full-power year, is the same duration of full-power operation; the capacity factor is applied by the operating history, never by the unit.
- **MWd/kg** is thermal energy per mass of heavy metal, 8.64e10 J/kg; it has the same dimension as a dose, so the registry cannot tell the two apart, and they are different quantity groups only by name.

## Every unit

The table is checked against the registry by a test.

| Group | Unit | Name | Factor to SI | Source |
| --- | --- | --- | --- | --- |
| Length (`units.length`) | `m` | metre | 1 | SI Brochure 9th ed. (2019) |
| Length (`units.length`) | `cm` | centimetre | 1 x 10^-2 | SI Brochure 9th ed. (2019) |
| Length (`units.length`) | `mm` | millimetre | 1 x 10^-3 | SI Brochure 9th ed. (2019) |
| Length (`units.length`) | `km` | kilometre | 1 x 10^3 | SI Brochure 9th ed. (2019) |
| Area and cross-section (`units.area`) | `m2` | square metre | 1 | SI Brochure 9th ed. (2019) |
| Area and cross-section (`units.area`) | `cm2` | square centimetre | 1 x 10^-4 | SI Brochure 9th ed. (2019) |
| Area and cross-section (`units.area`) | `barn` | barn | 1 x 10^-28 | SP 811 Appendix B: 1 b = 1e-28 m2 exactly |
| Mass (`units.mass`) | `kg` | kilogram | 1 | SI Brochure 9th ed. (2019) |
| Mass (`units.mass`) | `g` | gram | 1 x 10^-3 | SI Brochure 9th ed. (2019) |
| Mass (`units.mass`) | `t` | tonne | 1 x 10^3 | SI 2019 Table 8: 1 t = 1000 kg |
| Mass density (`units.density`) | `kg/m3` | kilogram per cubic metre | 1 | SI Brochure 9th ed. (2019) |
| Mass density (`units.density`) | `g/cm3` | gram per cubic centimetre | 1 x 10^3 | SI Brochure 9th ed. (2019) |
| Duration (`units.time`) | `s` | second | 1 | SI Brochure 9th ed. (2019) |
| Duration (`units.time`) | `min` | minute | 60 | SI 2019 Table 8: 1 min = 60 s |
| Duration (`units.time`) | `h` | hour | 3600 | SI 2019 Table 8: 1 h = 3600 s |
| Duration (`units.time`) | `d` | day | 86400 | SI 2019 Table 8: 1 d = 86 400 s |
| Time in service (`units.service_time`) | `s` | second | 1 | SI Brochure 9th ed. (2019) |
| Time in service (`units.service_time`) | `d` | day | 86400 | SI 2019 Table 8: 1 d = 86 400 s |
| Time in service (`units.service_time`) | `yr` | year (Julian) | 31557600 | FARIS definition: Julian year of 365.25 d = 31 557 600 s |
| Time in service (`units.service_time`) | `FPY` | full-power year | 31557600 | FARIS definition: 1 FPY is 31 557 600 s of full-power operation; the capacity factor is applied by the operating history, not here |
| Temperature (`units.temperature`) | `K` | kelvin | 1 | SI Brochure 9th ed. (2019) |
| Temperature (`units.temperature`) | `degC` | degree Celsius | 1 + 273.15 K | SI 2019: 0 degC = 273.15 K |
| Power (`units.power`) | `W` | watt | 1 | SI Brochure 9th ed. (2019) |
| Power (`units.power`) | `kW` | kilowatt | 1 x 10^3 | SI Brochure 9th ed. (2019) |
| Power (`units.power`) | `MW` | megawatt | 1 x 10^6 | SI Brochure 9th ed. (2019) |
| Power (`units.power`) | `GW` | gigawatt | 1 x 10^9 | SI Brochure 9th ed. (2019) |
| Particle energy (`units.energy_particle`) | `J` | joule | 1 | SI Brochure 9th ed. (2019) |
| Particle energy (`units.energy_particle`) | `eV` | electronvolt | 1602176634 x 10^-28 | SI 2019: e = 1.602176634e-19 C exactly, so 1 eV = 1.602176634e-19 J |
| Particle energy (`units.energy_particle`) | `keV` | kiloelectronvolt | 1602176634 x 10^-25 | SI 2019: e = 1.602176634e-19 C exactly, so 1 eV = 1.602176634e-19 J |
| Particle energy (`units.energy_particle`) | `MeV` | megaelectronvolt | 1602176634 x 10^-22 | SI 2019: e = 1.602176634e-19 C exactly, so 1 eV = 1.602176634e-19 J |
| Plant energy (`units.energy_plant`) | `J` | joule | 1 | SI Brochure 9th ed. (2019) |
| Plant energy (`units.energy_plant`) | `kWh` | kilowatt-hour | 36 x 10^5 | 1 kWh = 1000 W * 3600 s = 3.6e6 J (SI 2019) |
| Plant energy (`units.energy_plant`) | `MWh` | megawatt-hour | 36 x 10^8 | 1 MWh = 3.6e9 J (SI 2019) |
| Plant energy (`units.energy_plant`) | `GWh` | gigawatt-hour | 36 x 10^11 | 1 GWh = 3.6e12 J (SI 2019) |
| Plant energy (`units.energy_plant`) | `MWd` | megawatt-day | 864 x 10^8 | 1 MWd = 1e6 W * 86 400 s = 8.64e10 J (SI 2019) |
| Plant energy (`units.energy_plant`) | `GWd` | gigawatt-day | 864 x 10^11 | 1 GWd = 1e9 W * 86 400 s = 8.64e13 J (SI 2019) |
| Power density (`units.power_density`) | `W/m3` | watt per cubic metre | 1 | SI Brochure 9th ed. (2019) |
| Power density (`units.power_density`) | `W/cm3` | watt per cubic centimetre | 1 x 10^6 | SI Brochure 9th ed. (2019) |
| Power density (`units.power_density`) | `MW/m3` | megawatt per cubic metre | 1 x 10^6 | SI Brochure 9th ed. (2019) |
| Surface power (`units.heat_flux`) | `W/m2` | watt per square metre | 1 | SI Brochure 9th ed. (2019) |
| Surface power (`units.heat_flux`) | `kW/m2` | kilowatt per square metre | 1 x 10^3 | SI Brochure 9th ed. (2019) |
| Surface power (`units.heat_flux`) | `MW/m2` | megawatt per square metre | 1 x 10^6 | SI Brochure 9th ed. (2019) |
| Surface power (`units.heat_flux`) | `W/cm2` | watt per square centimetre | 1 x 10^4 | SI Brochure 9th ed. (2019) |
| Particle flux (`units.particle_flux`) | `n/m2/s` | neutrons per square metre per second | 1 | count per area per time; the neutron is a count, not a dimension |
| Particle flux (`units.particle_flux`) | `n/cm2/s` | neutrons per square centimetre per second | 1 x 10^4 | 1 cm-2 = 1e4 m-2 exactly |
| Fluence (`units.fluence`) | `n/m2` | neutrons per square metre | 1 | count per area |
| Fluence (`units.fluence`) | `n/cm2` | neutrons per square centimetre | 1 x 10^4 | 1 cm-2 = 1e4 m-2 exactly |
| Displacement damage (`units.damage`) | `dpa` | displacements per atom | 1 | FARIS definition: dpa counts displacements per lattice atom; its own axis, so it never converts to a plain fraction |
| Damage rate (`units.damage_rate`) | `dpa/s` | dpa per second | 1 | FARIS definition (this page) |
| Damage rate (`units.damage_rate`) | `dpa/yr` | dpa per Julian year | 1/31557600 | FARIS definition: Julian year of 365.25 d = 31 557 600 s |
| Damage rate (`units.damage_rate`) | `dpa/FPY` | dpa per full-power year | 1/31557600 | FARIS definition: per 31 557 600 s of full-power operation |
| Dose equivalent (`units.dose`) | `Sv` | sievert | 1 | SI Brochure 9th ed. (2019) |
| Dose equivalent (`units.dose`) | `mSv` | millisievert | 1 x 10^-3 | SI Brochure 9th ed. (2019) |
| Dose equivalent (`units.dose`) | `rem` | rem | 1 x 10^-2 | SP 811 Appendix B: 1 rem = 0.01 Sv exactly |
| Dose-equivalent rate (`units.dose_rate`) | `Sv/s` | sievert per second | 1 | SI Brochure 9th ed. (2019) |
| Dose-equivalent rate (`units.dose_rate`) | `Sv/h` | sievert per hour | 1/3600 | SI 2019 Table 8: 1 h = 3600 s |
| Dose-equivalent rate (`units.dose_rate`) | `mSv/h` | millisievert per hour | 1/3600 x 10^-3 | SI 2019 Table 8: 1 h = 3600 s |
| Dose-equivalent rate (`units.dose_rate`) | `uSv/h` | microsievert per hour | 1/3600 x 10^-6 | SI 2019 Table 8: 1 h = 3600 s |
| Dose-equivalent rate (`units.dose_rate`) | `rem/h` | rem per hour | 1/3600 x 10^-2 | NIST SP 811 (2008), Appendix B |
| Absorbed dose (`units.absorbed_dose`) | `Gy` | gray | 1 | SI Brochure 9th ed. (2019) |
| Absorbed dose (`units.absorbed_dose`) | `mGy` | milligray | 1 x 10^-3 | SI Brochure 9th ed. (2019) |
| Absorbed dose (`units.absorbed_dose`) | `rad` | rad | 1 x 10^-2 | SP 811 Appendix B: 1 rad = 0.01 Gy exactly |
| Absorbed-dose rate (`units.absorbed_dose_rate`) | `Gy/s` | gray per second | 1 | SI Brochure 9th ed. (2019) |
| Absorbed-dose rate (`units.absorbed_dose_rate`) | `Gy/h` | gray per hour | 1/3600 | SI 2019 Table 8: 1 h = 3600 s |
| Absorbed-dose rate (`units.absorbed_dose_rate`) | `mGy/h` | milligray per hour | 1/3600 x 10^-3 | SI 2019 Table 8: 1 h = 3600 s |
| Activity (`units.activity`) | `Bq` | becquerel | 1 | SI Brochure 9th ed. (2019) |
| Activity (`units.activity`) | `kBq` | kilobecquerel | 1 x 10^3 | SI Brochure 9th ed. (2019) |
| Activity (`units.activity`) | `MBq` | megabecquerel | 1 x 10^6 | SI Brochure 9th ed. (2019) |
| Activity (`units.activity`) | `GBq` | gigabecquerel | 1 x 10^9 | SI Brochure 9th ed. (2019) |
| Activity (`units.activity`) | `TBq` | terabecquerel | 1 x 10^12 | SI Brochure 9th ed. (2019) |
| Activity (`units.activity`) | `mCi` | millicurie | 37 x 10^6 | SP 811 Appendix B: 1 Ci = 3.7e10 Bq exactly |
| Activity (`units.activity`) | `Ci` | curie | 37 x 10^9 | SP 811 Appendix B: 1 Ci = 3.7e10 Bq exactly |
| Fraction (`units.fraction`) | `1` | fraction | 1 | a pure number |
| Fraction (`units.fraction`) | `%` | percent | 1 x 10^-2 | 1 % = 0.01 |
| Fraction (`units.fraction`) | `appm` | atomic parts per million | 1 x 10^-6 | FARIS definition: atoms of the species per 1e6 atoms of the material |
| Burn-up (`units.burnup`) | `J/kg` | joule per kilogram | 1 | SI Brochure 9th ed. (2019) |
| Burn-up (`units.burnup`) | `MWd/kg` | megawatt-day per kilogram | 864 x 10^8 | 1 MWd/kg = 8.64e10 J/kg (SI 2019); thermal energy per mass of heavy metal |
| Burn-up (`units.burnup`) | `GWd/t` | gigawatt-day per tonne | 864 x 10^8 | 1 GWd/t = 1e9 W * 86 400 s / 1000 kg = 8.64e10 J/kg (SI 2019); equal to 1 MWd/kg |
| Magnetic flux density (`units.magnetic_field`) | `T` | tesla | 1 | SI Brochure 9th ed. (2019) |
| Magnetic flux density (`units.magnetic_field`) | `mT` | millitesla | 1 x 10^-3 | SI Brochure 9th ed. (2019) |
| Magnetic flux density (`units.magnetic_field`) | `G` | gauss | 1 x 10^-4 | SP 811 Appendix B: 1 G = 1e-4 T exactly |
| Pressure (`units.pressure`) | `Pa` | pascal | 1 | SI Brochure 9th ed. (2019) |
| Pressure (`units.pressure`) | `kPa` | kilopascal | 1 x 10^3 | SI Brochure 9th ed. (2019) |
| Pressure (`units.pressure`) | `MPa` | megapascal | 1 x 10^6 | SI Brochure 9th ed. (2019) |
| Pressure (`units.pressure`) | `bar` | bar | 1 x 10^5 | SP 811 Appendix B: 1 bar = 1e5 Pa exactly |
| Pressure (`units.pressure`) | `atm` | standard atmosphere | 101325 | SP 811 Appendix B: 1 atm = 101 325 Pa exactly |

## For the desktop and other clients

Everything lives in `faris-model`: `settings::REGISTRY` holds the keys and defaults; `display::DisplayPrefs::from_resolved(&resolved_settings)` builds the choices; `prefs.show(group, si_value, decimals)` gives text with the unit always attached; `prefs.unit_for(group)` and `prefs.value_in(group, si_value)` give the unit and the number for a widget; `prefs.parse(group, text)` reads what the user typed and returns SI with the refusal message of a wrong kind; `prefs.number.fixed`, `.significant` and `.scientific` write numbers; `prefs.date.render_unix` writes dates. `units::convert` converts between two units and refuses mixed kinds.

## Unicode

Study names, bundle names, folder names and file names may hold any Unicode. FARIS stores and prints them as given and does not normalise: the same letter written in NFC and in NFD form is two different names, and both survive saving, opening and unpacking a study file unchanged.
