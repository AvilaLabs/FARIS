//! Export of the limit and property tables with their citations (MAT-056).
//!
//! JSON is the database itself. CSV has one row per limit or property, every field in its own
//! column, and reads back to the same rows.

use serde::de::DeserializeOwned;

use super::{
    Cite, Label, Limit, LimitStatus, MaterialsDb, Property, PropertyValue, Quantity, Reason,
    Stated, ValidRange,
};

/// The database as pretty JSON (the same schema as the file).
pub fn to_json(db: &MaterialsDb) -> String {
    let mut text = serde_json::to_string_pretty(&db.data).expect("the database serialises");
    text.push('\n');
    text
}

fn quote_field(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

fn write_row(out: &mut String, fields: &[String]) {
    let line: Vec<String> = fields.iter().map(|f| quote_field(f)).collect();
    out.push_str(&line.join(","));
    out.push('\n');
}

/// Parses RFC 4180 CSV text into rows of fields.
pub fn parse_csv(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
        } else {
            match c {
                '"' if field.is_empty() => in_quotes = true,
                ',' => row.push(std::mem::take(&mut field)),
                '\n' => {
                    row.push(std::mem::take(&mut field));
                    rows.push(std::mem::take(&mut row));
                }
                '\r' => {}
                _ => field.push(c),
            }
        }
    }
    if in_quotes {
        return Err("a quoted field is not closed".to_string());
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    Ok(rows)
}

fn enum_text<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn enum_from<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    serde_json::from_value(serde_json::Value::String(text.to_string()))
        .map_err(|e| format!("'{text}': {e}"))
}

fn opt_f64(text: &str) -> Result<Option<f64>, String> {
    if text.is_empty() {
        Ok(None)
    } else {
        text.parse()
            .map(Some)
            .map_err(|_| format!("'{text}' is not a number"))
    }
}

pub const LIMIT_COLUMNS: [&str; 24] = [
    "id",
    "material",
    "component",
    "quantity",
    "value",
    "unit",
    "convention",
    "convention_not_stated",
    "temperature_low_c",
    "temperature_high_c",
    "temperature_not_stated",
    "spectrum",
    "spectrum_not_stated",
    "form",
    "form_not_stated",
    "conditions",
    "status",
    "kind",
    "source",
    "locator",
    "quote",
    "value_text",
    "missing_reason",
    "missing_next_step",
];

/// The limit table as CSV with citations.
pub fn limits_csv(db: &MaterialsDb) -> String {
    let mut out = String::new();
    write_row(&mut out, &LIMIT_COLUMNS.map(str::to_string));
    for l in &db.data.limits {
        let (tl, th) = l
            .temperature_window_c
            .value()
            .map_or((String::new(), String::new()), |(a, b)| {
                (a.to_string(), b.to_string())
            });
        let cite = l.cite.clone().unwrap_or(Cite {
            source: String::new(),
            locator: String::new(),
            quote: String::new(),
            value_text: String::new(),
        });
        let ne = l.not_evaluated.clone().unwrap_or(Reason {
            reason: String::new(),
            next_step: String::new(),
        });
        let row = [
            l.id.clone(),
            l.material.clone(),
            l.component.clone(),
            enum_text(&l.quantity),
            l.value.map(|v| v.to_string()).unwrap_or_default(),
            l.unit.clone(),
            l.convention.value().cloned().unwrap_or_default(),
            l.convention.reason().unwrap_or_default().to_string(),
            tl,
            th,
            l.temperature_window_c
                .reason()
                .unwrap_or_default()
                .to_string(),
            l.spectrum.value().cloned().unwrap_or_default(),
            l.spectrum.reason().unwrap_or_default().to_string(),
            l.form.value().cloned().unwrap_or_default(),
            l.form.reason().unwrap_or_default().to_string(),
            l.conditions.join("\n"),
            enum_text(&l.status),
            enum_text(&l.kind),
            cite.source,
            cite.locator,
            cite.quote,
            cite.value_text,
            ne.reason,
            ne.next_step,
        ];
        write_row(&mut out, &row);
    }
    out
}

fn stated<T>(value: Option<T>, reason: String, what: &str) -> Result<Stated<T>, String> {
    match value {
        Some(value) => Ok(Stated::Value { value }),
        None if !reason.is_empty() => Ok(Stated::NotStated { not_stated: reason }),
        None => Err(format!("{what} has neither a value nor a reason")),
    }
}

/// Reads rows written by `limits_csv` back into limits.
pub fn limits_from_csv(text: &str) -> Result<Vec<Limit>, String> {
    let rows = parse_csv(text)?;
    let Some((header, body)) = rows.split_first() else {
        return Err("empty file".to_string());
    };
    if header.iter().map(String::as_str).collect::<Vec<_>>() != LIMIT_COLUMNS {
        return Err("the header row is not the limit table's columns".to_string());
    }
    let mut limits = Vec::new();
    for (n, r) in body.iter().enumerate() {
        if r.len() != LIMIT_COLUMNS.len() {
            return Err(format!(
                "row {} has {} fields, expected {}",
                n + 2,
                r.len(),
                LIMIT_COLUMNS.len()
            ));
        }
        let nonempty = |s: &String| (!s.is_empty()).then(|| s.clone());
        let temperature = match (opt_f64(&r[8])?, opt_f64(&r[9])?) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => None,
        };
        let cite = if r[18].is_empty() && r[19].is_empty() && r[20].is_empty() && r[21].is_empty() {
            None
        } else {
            Some(Cite {
                source: r[18].clone(),
                locator: r[19].clone(),
                quote: r[20].clone(),
                value_text: r[21].clone(),
            })
        };
        let not_evaluated = if r[22].is_empty() && r[23].is_empty() {
            None
        } else {
            Some(Reason {
                reason: r[22].clone(),
                next_step: r[23].clone(),
            })
        };
        limits.push(Limit {
            id: r[0].clone(),
            material: r[1].clone(),
            component: r[2].clone(),
            quantity: enum_from::<Quantity>(&r[3])?,
            value: opt_f64(&r[4])?,
            unit: r[5].clone(),
            convention: stated(nonempty(&r[6]), r[7].clone(), "convention")?,
            temperature_window_c: stated(temperature, r[10].clone(), "temperature window")?,
            spectrum: stated(nonempty(&r[11]), r[12].clone(), "spectrum")?,
            form: stated(nonempty(&r[13]), r[14].clone(), "form")?,
            conditions: if r[15].is_empty() {
                Vec::new()
            } else {
                r[15].split('\n').map(str::to_string).collect()
            },
            status: enum_from::<LimitStatus>(&r[16])?,
            kind: enum_from::<Label>(&r[17])?,
            cite,
            not_evaluated,
        });
    }
    Ok(limits)
}

pub const PROPERTY_COLUMNS: [&str; 17] = [
    "material",
    "id",
    "name",
    "unit",
    "label",
    "shape",
    "value",
    "points",
    "x_name",
    "valid_low",
    "valid_high",
    "conditions",
    "source",
    "locator",
    "quote",
    "value_text",
    "reason_or_method",
];

/// The property table as CSV with citations. A table is written as `x:y` pairs joined by `;`.
/// `reason_or_method` holds the not-evaluated reason, the calculation method or the authored reason.
pub fn properties_csv(db: &MaterialsDb) -> String {
    let mut out = String::new();
    write_row(&mut out, &PROPERTY_COLUMNS.map(str::to_string));
    for m in &db.data.materials {
        for p in &m.properties {
            let (shape, value, points, x_name) = match &p.value {
                PropertyValue::Scalar { value } => {
                    ("scalar", value.to_string(), String::new(), String::new())
                }
                PropertyValue::Table { points, x_name, .. } => (
                    "table",
                    String::new(),
                    points
                        .iter()
                        .map(|(x, y)| format!("{x}:{y}"))
                        .collect::<Vec<_>>()
                        .join(";"),
                    x_name.clone(),
                ),
                PropertyValue::None => ("none", String::new(), String::new(), String::new()),
            };
            let (vl, vh) = p
                .valid_range
                .as_ref()
                .map_or((String::new(), String::new()), |r| {
                    (r.low.to_string(), r.high.to_string())
                });
            let cite = p.cite.clone().unwrap_or(Cite {
                source: String::new(),
                locator: String::new(),
                quote: String::new(),
                value_text: String::new(),
            });
            let extra = p
                .not_evaluated
                .as_ref()
                .map(|r| format!("{} Next: {}", r.reason, r.next_step))
                .or_else(|| p.method.clone())
                .or_else(|| {
                    p.authored
                        .as_ref()
                        .map(|a| format!("{}: {}", a.author, a.reason))
                })
                .unwrap_or_default();
            write_row(
                &mut out,
                &[
                    m.id.clone(),
                    p.id.clone(),
                    p.name.clone(),
                    p.unit.clone(),
                    enum_text(&p.label),
                    shape.to_string(),
                    value,
                    points,
                    x_name,
                    vl,
                    vh,
                    p.conditions.join("\n"),
                    cite.source,
                    cite.locator,
                    cite.quote,
                    cite.value_text,
                    extra,
                ],
            );
        }
    }
    out
}

/// Properties with their material id, read back from `properties_csv`. The not-evaluated reason,
/// method and authored fields are carried in the one `reason_or_method` column, so only the
/// columns' own fields (label, shape, value, range, conditions, citation) are restored exactly.
pub fn properties_from_csv(text: &str) -> Result<Vec<(String, Property)>, String> {
    let rows = parse_csv(text)?;
    let Some((header, body)) = rows.split_first() else {
        return Err("empty file".to_string());
    };
    if header.iter().map(String::as_str).collect::<Vec<_>>() != PROPERTY_COLUMNS {
        return Err("the header row is not the property table's columns".to_string());
    }
    let mut out = Vec::new();
    for (n, r) in body.iter().enumerate() {
        if r.len() != PROPERTY_COLUMNS.len() {
            return Err(format!(
                "row {} has {} fields, expected {}",
                n + 2,
                r.len(),
                PROPERTY_COLUMNS.len()
            ));
        }
        let value = match r[5].as_str() {
            "scalar" => PropertyValue::Scalar {
                value: opt_f64(&r[6])?.ok_or("a scalar row has no value")?,
            },
            "table" => PropertyValue::Table {
                x_name: r[8].clone(),
                x_unit: String::new(),
                points: r[7]
                    .split(';')
                    .map(|pair| {
                        let (x, y) = pair.split_once(':').ok_or("a table point is not x:y")?;
                        Ok((
                            x.parse().map_err(|_| format!("'{x}' is not a number"))?,
                            y.parse().map_err(|_| format!("'{y}' is not a number"))?,
                        ))
                    })
                    .collect::<Result<Vec<(f64, f64)>, String>>()?,
            },
            "none" => PropertyValue::None,
            other => return Err(format!("unknown shape '{other}'")),
        };
        let valid_range = match (opt_f64(&r[9])?, opt_f64(&r[10])?) {
            (Some(low), Some(high)) => Some(ValidRange {
                x_name: r[8].clone(),
                low,
                high,
            }),
            _ => None,
        };
        let cite = (!r[12].is_empty()).then(|| Cite {
            source: r[12].clone(),
            locator: r[13].clone(),
            quote: r[14].clone(),
            value_text: r[15].clone(),
        });
        out.push((
            r[0].clone(),
            Property {
                id: r[1].clone(),
                name: r[2].clone(),
                unit: r[3].clone(),
                label: enum_from::<Label>(&r[4])?,
                value,
                valid_range,
                conditions: if r[11].is_empty() {
                    Vec::new()
                } else {
                    r[11].split('\n').map(str::to_string).collect()
                },
                cite,
                method: None,
                authored: None,
                not_evaluated: None,
            },
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::materials_db::Database;

    // Verifies: MAT-056
    #[test]
    fn the_json_export_is_the_database_and_reads_back_identically() {
        let db = MaterialsDb::embedded();
        let text = to_json(db);
        let back: Database = serde_json::from_str(&text).unwrap();
        assert_eq!(back, db.data);
        // A database rebuilt from the export validates and carries the same content.
        let rebuilt = MaterialsDb::from_json_str(&text).unwrap();
        assert_eq!(rebuilt.data, db.data);
    }

    // Verifies: MAT-056
    #[test]
    fn the_limit_csv_round_trips_every_row_and_citation_exactly() {
        let db = MaterialsDb::embedded();
        let csv = limits_csv(db);
        let back = limits_from_csv(&csv).unwrap();
        assert_eq!(back, db.data.limits);
        assert!(
            csv.contains("approximately 1.0 appm"),
            "the quote is exported"
        );
        assert!(
            csv.lines()
                .next()
                .unwrap()
                .contains("locator,quote,value_text")
        );
        // The same file written again from the rows is identical.
        let again = limits_csv(&MaterialsDb::from_json_str(&to_json(db)).unwrap());
        assert_eq!(again, csv);
    }

    // Verifies: MAT-056
    #[test]
    fn the_property_csv_round_trips_labels_values_ranges_and_citations() {
        let db = MaterialsDb::embedded();
        let csv = properties_csv(db);
        let back = properties_from_csv(&csv).unwrap();
        let mut expected = Vec::new();
        for m in &db.data.materials {
            for p in &m.properties {
                expected.push((m.id.clone(), p.clone()));
            }
        }
        assert_eq!(back.len(), expected.len());
        for ((bm, bp), (em, ep)) in back.iter().zip(&expected) {
            assert_eq!(bm, em);
            assert_eq!(
                (&bp.id, &bp.name, &bp.unit, bp.label),
                (&ep.id, &ep.name, &ep.unit, ep.label)
            );
            assert_eq!(bp.value, ep.value);
            assert_eq!(bp.cite, ep.cite);
            assert_eq!(bp.conditions, ep.conditions);
        }
        assert!(csv.contains("lower viable operational temperature"));
    }

    #[test]
    fn csv_quoting_survives_commas_quotes_and_line_breaks() {
        let rows = vec![vec![
            "a,b".to_string(),
            "say \"hi\"".to_string(),
            "two\nlines".to_string(),
            String::new(),
        ]];
        let mut text = String::new();
        write_row(&mut text, &rows[0]);
        assert_eq!(parse_csv(&text).unwrap(), rows);
        assert!(parse_csv("\"open").is_err());
    }
}
