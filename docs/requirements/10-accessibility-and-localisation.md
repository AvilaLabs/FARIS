# Accessibility and localisation

Who can use FARIS and in which language and number format. Part one (A11Y) is the
accessibility contract for a native egui desktop app: WCAG 2.2 AA applied by analogy
(CSS pixel read as logical pixel, and stated as such in every conformance report),
with Section 508 and EN 301 549 mapped, not certified. Part two (L10N) covers language,
number and unit formatting and file-format stability. Reference hardware is in
[the index](README.md#reference-hardware-and-models). Related: usability in
[09-usability.md](09-usability.md), colour maps and charts in 11-visualization.

Repository check (2026-10-05): `Cargo.lock` holds the `accesskit` core crate (0.24.1)
but no platform adapter crate (no `accesskit_winit` or `accesskit_unix`), and no source
file in `crates/faris-app` supplies accessibility information by hand. egui 0.36 builds
the accessibility tree; whether FARIS exposes it to the operating system has not been
tested. A11Y-040 requires that test.

## Keyboard operation and focus

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| A11Y-001 | FARIS shall allow every function to be operated from the keyboard alone. | 100 % of commands in the command registry (UX-030) reachable and operable by keyboard; core tasks T1–T5 (UX-010) completed keyboard-only in a scripted run. | Automated focus-walk plus manual keyboard-only audit. | WCAG 2.1.1 Keyboard [V]([W3C](https://www.w3.org/TR/WCAG22/)) | F2 | Partial: step keys 1–5, Ctrl/Cmd+S/O, tour keys; viewport and charts not operable |
| A11Y-002 | FARIS shall never trap keyboard focus. | 0 traps in the focus-walk over every panel, dialog and the 3D viewport; Esc or Tab always leaves. | Focus-walk test over all screens. | WCAG 2.1.2 No Keyboard Trap [V]([W3C](https://www.w3.org/TR/WCAG22/)) | F2 | Unmeasured |
| A11Y-003 | Focus order shall follow reading and task order. | Tab order matches visual order on 100 % of panels; verified by snapshot of the order list. | Focus-order snapshot test. | WCAG 2.4.3 Focus Order [V]([W3C](https://www.w3.org/TR/WCAG22/)) | F2 | Unmeasured |
| A11Y-004 | Keyboard focus shall always be visible. | A visible focus indicator on 100 % of focusable controls in all themes. | Focus-walk screenshots checked by pixel diff against unfocused state. | WCAG 2.4.7 Focus Visible (AA) [V]([W3C](https://www.w3.org/TR/WCAG22/)) | F2 | Unmeasured: egui default focus outline |
| A11Y-005 | Focused controls shall not be hidden by other UI. | 0 focused controls fully covered by a floating panel, tooltip or tour card in the focus-walk; the view scrolls the focused control into view. | Focus-walk with rectangle-overlap check. | WCAG 2.4.11 Focus Not Obscured (Minimum) (AA) [V]([W3C](https://www.w3.org/TR/WCAG22/)) | F2 | Unmeasured |
| A11Y-006 | The focus indicator should be strong (stretch goal, not AA). | Ring ≥ 2 px thick with ≥ 3:1 change in contrast between focused and unfocused state. | Pixel measurement in the focus-walk. | WCAG 2.4.13 Focus Appearance is Level AAA in WCAG 2.2, so this is a stretch goal [V]([W3C](https://www.w3.org/WAI/WCAG22/Understanding/focus-appearance.html)) | F4 | Unmeasured |
| A11Y-007 | Every drag shall have a single-pointer or keyboard alternative. | 100 % of drag operations (sliders, orbit, pan, panel resize, drag-and-drop open) have a click, key-step or numeric alternative. | Audit against the registry. | WCAG 2.5.7 Dragging Movements (AA) [V]([W3C](https://www.w3.org/TR/WCAG22/)) | F2 | Partial: sliders accept drags; alternatives not audited |
| A11Y-008 | The 3D viewport shall be operable from the keyboard. | Orbit, pan, zoom, reset, select next or previous component, and activate section plane by keys; each step documented in the cheat sheet. | UI automation script. | FARIS choice | F2 | No |
| A11Y-009 | No function shall depend on a time limit or hover alone. | 0 time-limited controls; every hover-only content has a keyboard or tap route; the tour never advances by itself. | Audit. | WCAG 2.2.1 Timing Adjustable and 1.4.13 [V]([W3C](https://www.w3.org/TR/WCAG22/)) | F2 | Partial: tour is user-paced; hover explanations on badges have no keyboard route verified |

## Contrast, colour and motion

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| A11Y-010 | Text shall have enough contrast. | ≥ 4.5:1 for 100 % of text styles (≥ 3:1 for text ≥ 18 pt) in the default theme; ≥ 7:1 in the high-contrast theme; muted and disabled text included. | Palette check in CI for every theme and text style. | WCAG 1.4.3 Contrast (Minimum) (AA) [V]([W3C](https://www.w3.org/TR/WCAG22/)) | F1 | Unmeasured |
| A11Y-011 | Controls and graphical content shall have enough contrast. | ≥ 3:1 against adjacent colours for 100 % of control borders, icons, focus ring, chart lines, markers and status pills. | Automated check on theme palette plus screenshot sampling. | WCAG 1.4.11 Non-text Contrast (AA) [V]([W3C](https://www.w3.org/TR/WCAG22/)) | F1 | Unmeasured |
| A11Y-012 | A high-contrast theme shall exist. | One theme passing A11Y-010 at 7:1 and A11Y-011 at 4.5:1; chosen automatically when the OS asks for it. | Contrast audit; OS-setting test. | VS Code and Blender ship high-contrast themes [U] | F2 | No |
| A11Y-013 | Colour shall never be the only carrier of meaning. | 100 % of status and flags (calculated, authored, literature, conditional, not-evaluated, failed, 2σ flag, stale, error cell) also carry text, a shape or a pattern; line charts add markers or dash styles. | Greyscale and colour-vision-simulation screenshot diff; registry lint. | WCAG 1.4.1 Use of Color (A) [V]([W3C](https://www.w3.org/TR/WCAG22/)) | F1 | Partial: kind pills carry text labels; chart series and flags not audited |
| A11Y-014 | Palettes shall be safe for common colour-vision deficiencies. | Categorical palette of ≤ 8 colours keeps every pair distinguishable under deuteranopia, protanopia and tritanopia simulation (CIEDE2000 ΔE ≥ 10 for each pair, provisional: no source for the threshold; confirm before F2 gate); sequential maps per VIS-020. | Simulation script over the palette. | CVD affects a notable share of users; cividis abstract cites more than 600 million people [V]([Nunez 2018](https://doi.org/10.1371/journal.pone.0199239)); ΔE threshold FARIS choice | F2 | No |
| A11Y-015 | FARIS shall honour the reduced-motion setting. | When the OS asks for reduced motion, tour transitions, camera easing, animation playback and spinners become cuts or static; auto-playing motion starts paused. | Setting toggle test. | WCAG 2.3.3 Animation from Interactions is AAA [V]([W3C](https://www.w3.org/TR/WCAG22/)); good practice | F2 | No |
| A11Y-016 | FARIS shall never flash. | 0 content flashing more than 3 times in any second; animation playback rate capped. | Frame-sequence analysis on tour, playback and busy states. | WCAG 2.3.1 Three Flashes or Below Threshold (A) [V]([W3C](https://www.w3.org/TR/WCAG22/)) | F2 | Unmeasured |
| A11Y-017 | Moving content shall be pausable. | Any motion lasting > 5 s (playback, progress animation) has pause, stop or hide. | Audit. | WCAG 2.2.2 Pause, Stop, Hide [V]([W3C](https://www.w3.org/TR/WCAG22/)) | F4 | No |

## Scaling, size and layout

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| A11Y-020 | The whole interface shall scale. | 100–200 % in the menu at F1, 75–300 % by F4; at 200 % no clipped, overlapping or unreachable text on any panel at the 980×640 minimum window; 3D viewport and charts scale too. | Screenshot matrix at 100, 125, 150, 200 and 300 %. | WCAG 1.4.4 Resize Text (AA) [V]([W3C](https://www.w3.org/TR/WCAG22/)) | F1 (200 %), F4 (300 %) | Partial: interface size menu 100/125/150/175/200 % and Ctrl/Cmd +/−/0; no overlap check |
| A11Y-021 | Body text shall start at a readable size. | Minimum 13 logical px for body and 11 px for dense table text at 100 %; no text style below 11 px. | Style lint on the theme. | FARIS choice; some chart and tour text is 11–12 px today [internal] | F1 | Partial: smallest font set found is 11 px |
| A11Y-022 | Pointer targets shall be large enough. | ≥ 24×24 logical px for all controls, ≥ 32 px for primary controls, or the spacing exception documented per control. | Layout lint on widget rectangles in egui_kittest. | WCAG 2.5.8 Target Size (Minimum) (AA) [V]([W3C](https://www.w3.org/WAI/WCAG22/Understanding/target-size-minimum.html)) | F2 | Unmeasured |
| A11Y-023 | Content shall reflow in narrow windows. | At the minimum window and 200 % scale, panels stack or scroll; no two-dimensional scrolling of text blocks. | Screenshot matrix. | WCAG 1.4.10 Reflow, by analogy [U] | F2 | Unmeasured |
| A11Y-024 | Text spacing changes shall not break the layout. | Increasing line height to 1.5× and letter spacing to 0.12 em loses no content or function. | Screenshot test with modified style. | WCAG 1.4.12 Text Spacing, by analogy [U] | F4 | No |
| A11Y-025 | Tooltips shall be dismissable, hoverable and persistent. | Esc dismisses; pointer can move onto the tooltip; it stays until dismissed or focus leaves. | UI test. | WCAG 1.4.13 [V]([W3C](https://www.w3.org/TR/WCAG22/)) | F2 | Unmeasured |

## Screen reader and text equivalents

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| A11Y-040 | FARIS shall expose its interface to the platform accessibility service. | The AccessKit tree is reachable by Orca (Linux AT-SPI) at F2, NVDA and Narrator (Windows UI Automation) and VoiceOver (macOS) by F7; a recorded test lists role and name for the main window as seen by the screen reader. | Scripted walkthrough per screen reader; adapter present in the lock file and build. | egui builds the AccessKit tree and eframe enables it where an adapter exists [V]([egui accessibility](https://github.com/emilk/egui#what-about-accessibility-such-as-screen-readers)); adapter parity incomplete [V]([AccessKit](https://github.com/AccessKit/accesskit)) | F2 (Linux), F7 (others) | No: only the `accesskit` core crate is in Cargo.lock, no adapter |
| A11Y-041 | Every standard control shall have role, name, value and state. | 100 % of non-canvas controls have all four in the AccessKit tree; egui_kittest query fails the build on any unnamed control. | egui_kittest accessibility-tree lint in CI. | WCAG 4.1.2 Name, Role, Value [V]([W3C](https://www.w3.org/TR/WCAG22/)); egui_kittest queries the tree [V]([egui accessibility](https://github.com/emilk/egui#what-about-accessibility-such-as-screen-readers)) | F2 | No |
| A11Y-042 | Custom-painted widgets shall be described by hand. | 100 % of custom widgets (viewport, charts, badges, spectrum plots, timelines) register a role, a name and a value or summary through egui widget information. | Tree lint; custom-widget inventory against source. | egui: custom widgets must be given widget information by hand [V]([egui accessibility](https://github.com/emilk/egui#what-about-accessibility-such-as-screen-readers)) | F2 | No |
| A11Y-043 | The core workflow shall be completable with a screen reader. | T1–T4 completed by ≥ 3 screen-reader users per platform with success ≥ 80 % (provisional: no evidence yet that egui adapters support the needed widgets; confirm after A11Y-040 test, before F2 gate). | Moderated session per A11Y-050. | Real-world NVDA, Orca and VoiceOver behaviour with egui not verified [U] | F2 (Orca), F7 | No |
| A11Y-044 | Every chart shall have a text equivalent. | 100 % of charts offer "view as table", copy as CSV and a one-sentence generated summary naming quantity, unit, range and kind; the table opens with one key press. | UI test over all charts. | WCAG 1.1.1 Non-text Content (A) [V]([W3C](https://www.w3.org/TR/WCAG22/)) | F2 | Partial: charts also export as CSV; no in-app table view or summary |
| A11Y-045 | The 3D view shall have a text equivalent. | A "view as text" lists visible components with identity, material, selected-field value and uncertainty, sortable, with the same selection as the 3D view; a spoken-style summary of the current section and field. | UI test; tree lint. | WCAG 1.1.1 [V]([W3C](https://www.w3.org/TR/WCAG22/)); canvas content is invisible to AccessKit unless mirrored [R3] | F2 | No |
| A11Y-046 | Status changes shall be announced. | Run started, progress milestones, completion, cancel, errors and stale results sent to the accessibility live region within 1 s. | Screen-reader walkthrough; tree event log. | WCAG 4.1.3 Status Messages [U] | F2 | No |
| A11Y-047 | Exported PDF reports shall be accessible. | PDF/UA-tagged with reading order, document language and figure alt text; veraPDF passes. | veraPDF in CI. | PDF/UA ISO 14289 [U] | F4 | No: PDF is two pages of real text, untagged |
| A11Y-048 | Input assistance shall not make users repeat themselves. | No re-entry of already-given data; error messages name the field and the fix; sign-in has no cognitive-function test. | Audit. | WCAG 3.3.1, 3.3.3, 3.3.7, 3.3.8 [U] | F4 | Unmeasured: optional sign-in present |

## Conformance statements and audits

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| A11Y-050 | FARIS shall be audited with assistive-technology users. | ≥ 5 users of screen readers, magnification, switch or voice control before each major release; findings tracked per UX-077; 0 open "blocks core task" findings at release. | Audit report with findings and closure. | FARIS choice | F4 | No |
| A11Y-051 | FARIS shall publish an Accessibility Conformance Report. | One report per release in the VPAT 2.5 structure against WCAG 2.2 A and AA, Section 508 and EN 301 549 mapping; every "Does not support" carries a date and a fix target; states plainly that WCAG is applied to a desktop app by analogy and that it is a mapping, not a certification. | Report diff against release; checklist in CI. | VPAT 2.5 [U]([ITI](https://www.itic.org/policy/accessibility/vpat)) | F4 | No |
| A11Y-052 | Section 508 shall be mapped honestly. | Table mapping each applicable criterion (WCAG 2.0 A and AA for software, excluding the four web-only criteria and Complete Processes) to a FARIS requirement ID and a test. | Mapping table with 0 unmapped rows. | Section 508 incorporates WCAG 2.0 A/AA; non-web software minus Bypass Blocks, Multiple Ways, Consistent Navigation, Consistent Identification [V]([Access Board](https://www.access-board.gov/ict/)) | F4 | No |
| A11Y-053 | EN 301 549 shall be mapped honestly. | Table mapping clause 11 (software) and the platform-service and assistive-technology compatibility clauses of v3.2.1 to FARIS requirement IDs; clause numbers verified against the primary text before publication. | Mapping table; clause check against the ETSI document. | v3.2.1 is current and builds on WCAG 2.1 [V]([EC](https://digital-strategy.ec.europa.eu/en/policies/latest-changes-accessibility-standard)); clause numbers [U] | F4 | No |
| A11Y-054 | Automated accessibility checks shall run in CI and not be mistaken for conformance. | Contrast, target size, focus-walk, tree lint and screenshot checks green on every merge; the ACR states which criteria are covered by automation and which by manual audit. | CI job; ACR coverage table. | Automated checks find about a third of problems [U] | F2 | No |
| A11Y-055 | Accessibility regressions shall fail the release. | 0 new failures in A11Y-001 to A11Y-046 automated checks between releases. | Release checklist. | FARIS choice | F2 | No |

## Language and strings

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| L10N-001 | FARIS shall hold every user-facing string in resource files. | 100 % of UI, error, tooltip, tour, export and CLI help strings externalised; no string literal passed to a widget or exporter outside the resource layer. | Source lint in CI. | Fluent or ICU message resources [U] | F4 | No: strings are inline in the app crate |
| L10N-002 | Messages shall be built with named arguments, not concatenation. | 0 concatenated sentences; plural and gender handled by the message format; 100 % of messages with variables have a test with a long and a short value. | Lint plus message tests. | Fluent / ICU message format [U] | F4 | No |
| L10N-003 | A pseudo-locale build shall prove the layout copes. | Pseudo-locale (accents, +40 % length, brackets) shows 0 hard-coded strings and 0 clipped or overlapping text over all panels at 100 % and 200 % scale. | Screenshot test in CI. | Standard pseudo-localisation practice [U] | F4 | No |
| L10N-004 | FARIS shall ship English first and add languages by demand. | English (British spelling) at F1; ≥ 2 further languages by F6 chosen from the user community (Japanese is the first candidate); a language ships only when 100 % of strings are translated and reviewed by a domain speaker. | Translation-coverage report. | Nuclear community outreach priority [internal]; provisional language choice | F1 (en-GB), F6 | Partial: English only |
| L10N-005 | Translation shall have a workflow that protects meaning. | Source strings carry context notes; domain glossary is shared with translators; every translation reviewed by a second speaker; missing strings fall back to English visibly, never blank; a status label (calculated, authored, ...) is never translated into a word with a different meaning. | Workflow audit; fallback test. | FARIS choice | F6 | No |
| L10N-006 | Stable identifiers shall never be translated. | Parameter keys, file-format fields, CLI flags, CSV headers and status enum names stay English in all locales; display names are separate. | Schema lint. | FARIS choice | F4 | Partial: file and CSV field names are fixed English; no display-name layer |

## Numbers, dates, units and exports

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| L10N-010 | The UI shall format numbers, dates and times by locale. | Decimal mark, grouping, date order and 24-hour or 12-hour clock follow the locale for en-GB, en-US, de-DE, fr-FR and ja-JP; tests round-trip each. | Locale matrix tests. | Unicode CLDR [U]([CLDR](https://cldr.unicode.org/)) | F4 | No |
| L10N-011 | Exports and files shall use one locale-invariant format. | CSV, JSON, .faris and CLI output use "." as the decimal mark, no grouping, ISO 8601 dates in UTC, UTF-8; the format is stated in every export's header or schema; reading a file never depends on the user's locale. | Locale matrix test: export under de-DE, parse under en-US, values identical. | Trap: decimal-comma locales corrupting CSV [R3] | F1 | Partial: values written at full precision with units in headers; locale independence untested |
| L10N-012 | A decimal separator shall never be ambiguous on screen. | Numbers with thousands grouping in a decimal-comma locale use a thin space, not a dot; ranges and ± never split across a separator; tables show the unit in the column header. | Screenshot review by locale; format unit tests. | FARIS choice | F4 | No |
| L10N-013 | Numbers shall keep their precision rules in every locale. | Significant figures follow the uncertainty (error to 1–2 figures, value to the same decimal place) in every locale; 0 differences in digits between locales. | Property test. | FARIS choice | F4 | No |

## Units

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| L10N-020 | Every quantity shall have a dimension and a unit. | 100 % of inputs and outputs typed with a dimension; stored values in SI (or the stated nuclear unit); display units chosen per quantity group with SI as default; conversions exact to 1e-12 relative. | Property tests; registry lint. | NIST SP 811 and ISO 80000 [U]; unit errors are the larger risk than language [R3] | F2 | Partial: units in labels and CSV headers; no dimension type or display choice |
| L10N-021 | Mixed dimensions shall be rejected with a message. | 100 % of attempts to enter a wrong-dimension value are refused with cause and the accepted units. | Fuzz inputs. | FARIS choice | F2 | No |
| L10N-022 | Common nuclear and engineering units shall be offered. | eV, keV, MeV, J; cm and m; barn; n/cm²/s; Gy and Sv; years and seconds; chosen per study or per user, saved in preferences (UX-060); the chosen unit is always printed next to the number. | UI test. | Domain practice [internal] | F2 | No |
| L10N-023 | A unit change shall never alter stored data. | Switching display units changes 0 bytes of the saved study and 0 result hashes. | Hash test. | FARIS choice | F2 | No |

## Scripts, fonts and input

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| L10N-030 | FARIS shall display CJK text. | Japanese, Simplified Chinese and Korean render with a bundled or system font fallback; 0 missing-glyph boxes in a corpus test of 1,000 names and strings. | Glyph-coverage test. | FARIS choice; egui default fonts cover Latin only [U] | F6 | No: Latin default fonts only |
| L10N-031 | Text input shall work with input methods. | Typing and composition with IME on Linux (IBus, Fcitx) and later Windows and macOS in every text field; 0 lost or duplicated characters in a scripted composition test. | IME test. | egui IME support varies by platform [U] | F6 | No |
| L10N-032 | Right-to-left readiness shall be known, not assumed. | A capability test states what Arabic and Hebrew display and editing do today; until it passes, RTL is documented as unsupported; layout code uses start/end, not left/right, so mirroring can be added. | Capability test; layout lint. | Stock egui text stack has no bidi shaping [U] | F7 | No |
| L10N-033 | Exports shall embed their fonts. | PDF and SVG exports embed or outline fonts so text is identical on any machine, including non-Latin scripts shipped. | Render on a clean machine; text extraction test. | Exports use real text in bundled fonts [internal] | F4 | Partial: PDF uses bundled Ubuntu fonts; non-Latin not covered |
| L10N-034 | Names and file paths shall allow any Unicode. | Study names, paths and author fields accept NFC and NFD Unicode; round-trip through .faris and CSV with 0 changes. | Round-trip test. | FARIS choice | F2 | Unmeasured |

## Traps

- A green automated scan is not conformance. It sees buttons and misses the 3D view and the charts, which are FARIS's main value.
- Accessibility libraries expose built-in widgets for free. Custom-painted content is invisible until each piece is described by hand.
- "Supports a screen reader" is meaningless without the platform and the reader named, and a recorded test on FARIS itself.
- WCAG was written for the web. State the analogy (logical pixels for CSS pixels) in the report, and never write "WCAG conformant" where the mapping is partial. A VPAT is a self-declared report, not a certificate.
- Focus appearance (2.4.13) is AAA. Meeting it is good; claiming it as AA is wrong.
- Translation percentage hides meaning errors. A translated "authored" that reads like "measured" is worse than English.
- Locale bugs appear in files, not screens. A decimal comma in a CSV is a silent wrong number, so test export and import across locales.
- Pseudo-locale proves layout only. It does not prove a translation is correct or that the font can draw the script.

