//! The limit tables, read from the data files embedded in the crate.
//!
//! `data/waste/nrc_10cfr61_55.json` holds Tables 1 and 2 of 10 CFR 61.55 beside the stored eCFR
//! snapshot they were read from; `data/waste/iaea_gsr3_table_i2.json` holds the IAEA clearance
//! levels. A test compares every encoded row with the snapshot or the quoted page.

use std::{collections::BTreeMap, sync::OnceLock};

use serde::Deserialize;

use super::{SchemeInfo, WasteForm};

const NRC_JSON: &str = include_str!("../../data/waste/nrc_10cfr61_55.json");
const IAEA_JSON: &str = include_str!("../../data/waste/iaea_gsr3_table_i2.json");
/// The stored eCFR text the NRC rows were read from.
pub const NRC_SNAPSHOT_XML: &str = include_str!("../../data/waste/ecfr_10cfr61.55.xml");

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    General,
    ActivatedMetal,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Table1Row {
    pub label: String,
    pub nuclide: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    pub scope: Scope,
    pub unit: String,
    pub limit: f64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Table2Row {
    pub label: String,
    pub nuclide: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    pub scope: Scope,
    pub unit: String,
    pub col1: Option<f64>,
    pub col2: Option<f64>,
    pub col3: Option<f64>,
}

impl Table2Row {
    pub fn column(&self, column: u8) -> Option<f64> {
        match column {
            1 => self.col1,
            2 => self.col2,
            _ => self.col3,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Source {
    pub title: String,
    pub url: String,
    pub retrieved: String,
    #[serde(default)]
    pub snapshot_sha256: Option<String>,
    pub licence: String,
    pub redistribution: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct NrcTables {
    pub scheme_id: String,
    pub scheme_name: String,
    pub scheme_version: String,
    pub scheme_date: String,
    pub source: Source,
    pub table1: Vec<Table1Row>,
    pub table2: Vec<Table2Row>,
}

impl NrcTables {
    pub fn scheme_info(&self) -> SchemeInfo {
        SchemeInfo {
            id: self.scheme_id.clone(),
            name: self.scheme_name.clone(),
            version: self.scheme_version.clone(),
            date: self.scheme_date.clone(),
            citation: format!(
                "{} ({}), retrieved {}",
                self.source.title, self.source.url, self.source.retrieved
            ),
            status: "IMPLEMENTED".to_string(),
            status_reason: None,
        }
    }

    /// Table 1 rows that apply to a form, one per nuclide or group: the activated-metal row where
    /// the form is activated metal and the nuclide has one, the plain row otherwise.
    pub fn table1_for(&self, form: WasteForm) -> Vec<&Table1Row> {
        let mut rows: Vec<&Table1Row> = Vec::new();
        for row in &self.table1 {
            let key = (&row.nuclide, &row.group);
            let better = |other: &&Table1Row| (&other.nuclide, &other.group) == key;
            match (row.scope, form) {
                (Scope::General, WasteForm::ActivatedMetal) => {
                    let has_metal_row = self
                        .table1
                        .iter()
                        .any(|o| o.scope == Scope::ActivatedMetal && better(&o));
                    if !has_metal_row {
                        rows.push(row);
                    }
                }
                (Scope::General, WasteForm::Other) => rows.push(row),
                (Scope::ActivatedMetal, WasteForm::ActivatedMetal) => rows.push(row),
                (Scope::ActivatedMetal, WasteForm::Other) => {}
            }
        }
        rows
    }

    /// Table 2 rows that apply to a form, chosen the same way.
    pub fn table2_for(&self, form: WasteForm) -> Vec<&Table2Row> {
        let mut rows: Vec<&Table2Row> = Vec::new();
        for row in &self.table2 {
            let key = (&row.nuclide, &row.group);
            let same = |other: &&Table2Row| (&other.nuclide, &other.group) == key;
            match (row.scope, form) {
                (Scope::General, WasteForm::ActivatedMetal) => {
                    let has_metal_row = self
                        .table2
                        .iter()
                        .any(|o| o.scope == Scope::ActivatedMetal && same(&o));
                    if !has_metal_row {
                        rows.push(row);
                    }
                }
                (Scope::General, WasteForm::Other) => rows.push(row),
                (Scope::ActivatedMetal, WasteForm::ActivatedMetal) => rows.push(row),
                (Scope::ActivatedMetal, WasteForm::Other) => {}
            }
        }
        rows
    }

    /// Nuclides that Table 1 lists only for activated metal: no limit exists for other forms.
    pub fn activated_metal_only_nuclides(&self) -> Vec<&str> {
        self.table1
            .iter()
            .filter(|r| r.scope == Scope::ActivatedMetal)
            .filter_map(|r| r.nuclide.as_deref())
            .filter(|n| {
                !self
                    .table1
                    .iter()
                    .any(|o| o.scope == Scope::General && o.nuclide.as_deref() == Some(n))
            })
            .collect()
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ClearanceLevel {
    pub nuclide: String,
    pub bq_per_g: f64,
    pub parent_with_progeny: bool,
    pub printed_page: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct IaeaSource {
    pub title: String,
    pub url: String,
    pub retrieved: String,
    pub table: String,
    pub licence: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct IaeaTables {
    pub scheme_id: String,
    pub scheme_name: String,
    pub scheme_version: String,
    pub scheme_date: String,
    pub source: IaeaSource,
    pub levels: Vec<ClearanceLevel>,
    #[serde(skip)]
    index: BTreeMap<String, usize>,
}

impl IaeaTables {
    pub fn scheme_info(&self) -> SchemeInfo {
        SchemeInfo {
            id: self.scheme_id.clone(),
            name: self.scheme_name.clone(),
            version: self.scheme_version.clone(),
            date: self.scheme_date.clone(),
            citation: format!(
                "{}, {} ({}), retrieved {}",
                self.source.title, self.source.table, self.source.url, self.source.retrieved
            ),
            status: "IMPLEMENTED".to_string(),
            status_reason: None,
        }
    }

    pub fn level(&self, nuclide: &str) -> Option<&ClearanceLevel> {
        self.index.get(nuclide).map(|&i| &self.levels[i])
    }
}

pub fn nrc() -> &'static NrcTables {
    static TABLES: OnceLock<NrcTables> = OnceLock::new();
    TABLES.get_or_init(|| {
        serde_json::from_str(NRC_JSON).expect("the embedded 10 CFR 61.55 table file is valid")
    })
}

pub fn iaea() -> &'static IaeaTables {
    static TABLES: OnceLock<IaeaTables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut tables: IaeaTables = serde_json::from_str(IAEA_JSON)
            .expect("the embedded IAEA clearance table file is valid");
        tables.index = tables
            .levels
            .iter()
            .enumerate()
            .map(|(i, l)| (l.nuclide.clone(), i))
            .collect();
        tables
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// One table row read from the stored eCFR text: cell texts with markup removed.
    fn snapshot_rows(table_title: &str) -> Vec<Vec<String>> {
        let xml = NRC_SNAPSHOT_XML;
        let start = xml
            .find(&format!("<P class=\"title\">{table_title}</P>"))
            .expect("table title in snapshot");
        let body = &xml[start..];
        let body = &body[body.find("<TBODY>").unwrap() + 7..];
        let body = &body[..body.find("</TBODY>").unwrap()];
        body.split("<TR>")
            .skip(1)
            .map(|row| {
                row.split("<TD")
                    .skip(1)
                    .map(|cell| {
                        let text = &cell[cell.find('>').unwrap() + 1..cell.find("</TD>").unwrap()];
                        strip_tags(text)
                    })
                    .collect()
            })
            .collect()
    }

    fn strip_tags(text: &str) -> String {
        let mut out = String::new();
        let mut in_tag = false;
        for c in text.chars() {
            match c {
                '<' => in_tag = true,
                '>' => in_tag = false,
                c if !in_tag => out.push(c),
                _ => {}
            }
        }
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// A limit cell: `Some(value)`, `None` for the "no limit" mark `(1)`.
    fn cell_value(cell: &str) -> Option<f64> {
        if cell == "(1)" {
            return None;
        }
        let digits = cell.strip_prefix("1 ").unwrap_or(cell);
        Some(digits.replace(',', "").parse().expect("number cell"))
    }

    // Verifies: ACT-030, ACT-031
    #[test]
    fn table1_equals_the_stored_ecfr_snapshot_row_for_row() {
        let snapshot = snapshot_rows("Table 1");
        let encoded = &nrc().table1;
        assert_eq!(snapshot.len(), 9);
        assert_eq!(snapshot.len(), encoded.len());
        for (cells, row) in snapshot.iter().zip(encoded) {
            assert_eq!(cells[0], row.label, "row label");
            assert_eq!(cell_value(&cells[1]), Some(row.limit), "{}", row.label);
            // The nanocurie per gram rows are exactly those whose cell carries footnote 1.
            assert_eq!(
                cells[1].starts_with("1 "),
                row.unit == "nCi/g",
                "{}",
                row.label
            );
        }
    }

    // Verifies: ACT-030, ACT-032
    #[test]
    fn table2_equals_the_stored_ecfr_snapshot_row_for_row() {
        let snapshot = snapshot_rows("Table 2");
        let encoded = &nrc().table2;
        assert_eq!(snapshot.len(), 7);
        assert_eq!(snapshot.len(), encoded.len());
        for (cells, row) in snapshot.iter().zip(encoded) {
            assert_eq!(cells[0], row.label, "row label");
            assert_eq!(cell_value(&cells[1]), row.col1, "{} col 1", row.label);
            assert_eq!(cell_value(&cells[2]), row.col2, "{} col 2", row.label);
            assert_eq!(cell_value(&cells[3]), row.col3, "{} col 3", row.label);
            assert_eq!(row.unit, "Ci/m3");
        }
    }

    // Verifies: ACT-031, ACT-032
    #[test]
    fn the_values_named_in_the_requirements_are_encoded() {
        let t = nrc();
        let t1 = |label: &str| t.table1.iter().find(|r| r.label == label).unwrap().limit;
        assert_eq!(t1("Nb-94 in activated metal"), 0.2);
        assert_eq!(t1("Ni-59 in activated metal"), 220.0);
        assert_eq!(t1("C-14"), 8.0);
        assert_eq!(t1("C-14 in activated metal"), 80.0);
        assert_eq!(t1("Tc-99"), 3.0);
        assert_eq!(t1("I-129"), 0.08);
        let t2 = |label: &str| t.table2.iter().find(|r| r.label == label).unwrap();
        assert_eq!(
            (t2("H-3").col1, t2("H-3").col2, t2("H-3").col3),
            (Some(40.0), None, None)
        );
        assert_eq!((t2("Co-60").col1, t2("Co-60").col2), (Some(700.0), None));
        let ni = t2("Ni-63");
        assert_eq!(
            (ni.col1, ni.col2, ni.col3),
            (Some(3.5), Some(70.0), Some(700.0))
        );
        let nim = t2("Ni-63 in activated metal");
        assert_eq!(
            (nim.col1, nim.col2, nim.col3),
            (Some(35.0), Some(700.0), Some(7000.0))
        );
        let sr = t2("Sr-90");
        assert_eq!(
            (sr.col1, sr.col2, sr.col3),
            (Some(0.04), Some(150.0), Some(7000.0))
        );
        let cs = t2("Cs-137");
        assert_eq!(
            (cs.col1, cs.col2, cs.col3),
            (Some(1.0), Some(44.0), Some(4600.0))
        );
    }

    // Verifies: ACT-030
    #[test]
    fn the_snapshot_matches_its_recorded_hash_and_carries_a_retrieval_date() {
        let t = nrc();
        let hash = format!("{:x}", Sha256::digest(NRC_SNAPSHOT_XML.as_bytes()));
        assert_eq!(Some(hash), t.source.snapshot_sha256);
        assert_eq!(t.source.retrieved.len(), 10);
        assert!(t.source.url.starts_with("https://www.ecfr.gov/"));
        assert!(NRC_SNAPSHOT_XML.contains("61.55 Waste classification"));
    }

    // Verifies: ACT-034
    #[test]
    fn activated_metal_rows_replace_plain_rows_only_for_that_form() {
        let t = nrc();
        let labels =
            |rows: Vec<&Table2Row>| rows.iter().map(|r| r.label.clone()).collect::<Vec<_>>();
        let metal = labels(t.table2_for(WasteForm::ActivatedMetal));
        let other = labels(t.table2_for(WasteForm::Other));
        assert!(metal.contains(&"Ni-63 in activated metal".to_string()));
        assert!(!metal.contains(&"Ni-63".to_string()));
        assert!(other.contains(&"Ni-63".to_string()));
        assert!(!other.contains(&"Ni-63 in activated metal".to_string()));
        let t1: Vec<_> = t
            .table1_for(WasteForm::Other)
            .iter()
            .map(|r| r.label.clone())
            .collect();
        assert!(
            !t1.iter()
                .any(|l| l.contains("Nb-94") || l.contains("Ni-59"))
        );
        assert_eq!(t.activated_metal_only_nuclides(), vec!["Ni-59", "Nb-94"]);
    }

    // Verifies: ACT-036
    #[test]
    fn iaea_levels_equal_the_values_read_from_the_standard() {
        // Read from Table I.2 (printed pages 124 to 127) of GSR Part 3, STI/PUB/1578.
        let expected: [(&str, f64, bool); 20] = [
            ("H-3", 100.0, false),
            ("C-14", 1.0, false),
            ("Mn-54", 0.1, false),
            ("Fe-55", 1000.0, false),
            ("Co-60", 0.1, false),
            ("Ni-59", 100.0, false),
            ("Ni-63", 100.0, false),
            ("Zn-65", 0.1, false),
            ("Nb-94", 0.1, false),
            ("Tc-99", 1.0, false),
            ("Ag-110m", 0.1, true),
            ("Sr-90", 1.0, true),
            ("I-129", 0.01, false),
            ("Cs-134", 0.1, false),
            ("Cs-137", 0.1, true),
            ("Eu-152", 0.1, false),
            ("Ta-182", 0.1, false),
            ("Pu-241", 10.0, false),
            ("Am-241", 0.1, false),
            ("Cm-242", 10.0, false),
        ];
        let t = iaea();
        for (nuclide, level, progeny) in expected {
            let row = t
                .level(nuclide)
                .unwrap_or_else(|| panic!("{nuclide} missing"));
            assert_eq!(row.bq_per_g, level, "{nuclide}");
            assert_eq!(row.parent_with_progeny, progeny, "{nuclide}");
            assert!((124..=127).contains(&row.printed_page));
        }
        assert_eq!(t.levels.len(), 257);
        assert!(t.source.licence.contains("IAEA"));
    }
}
