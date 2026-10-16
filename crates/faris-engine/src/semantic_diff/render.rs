//! The readable text form of a diff. It says the same as the JSON, in the same order.

use super::{Change, ChangeKind, DiffReport, Standing};
use faris_model::canonical::canonical_json;
use serde_json::Value;
use std::fmt::Write;

fn short(value: &Value) -> String {
    let text = match value {
        Value::Object(map) => format!("an object of {} keys", map.len()),
        Value::Array(items) => format!("a list of {} items", items.len()),
        other => canonical_json(other),
    };
    if text.chars().count() > 70 {
        let cut: String = text.chars().take(67).collect();
        format!("{cut}...")
    } else {
        text
    }
}

fn change_line(change: &Change) -> String {
    let unit = change
        .unit
        .as_deref()
        .map_or_else(String::new, |u| format!(" {u}"));
    let place = if change.path.is_empty() {
        change.document.clone()
    } else {
        format!("{} {}", change.document, change.path)
    };
    let body = match (change.kind, &change.old, &change.new) {
        (ChangeKind::Changed, Some(old), Some(new)) => {
            format!("{} -> {}{unit}", short(old), short(new))
        }
        (ChangeKind::Added, _, Some(new)) => format!("added: {}{unit}", short(new)),
        (ChangeKind::Removed, Some(old), _) => format!("removed: {}{unit}", short(old)),
        (ChangeKind::Moved, Some(old), Some(new)) => {
            format!("moved from position {} to {}", short(old), short(new))
        }
        _ => "changed".into(),
    };
    let scope = if change.scope.is_empty() {
        String::new()
    } else {
        format!(" [{}]", change.scope)
    };
    format!(
        "  {} {place}{scope}: {body} ({})",
        change.id,
        change.cost.words()
    )
}

fn section(out: &mut String, title: &str, changes: &[Change]) {
    let _ = writeln!(out, "\n{title} ({})", changes.len());
    if changes.is_empty() {
        let _ = writeln!(out, "  none");
    }
    for change in changes {
        let _ = writeln!(out, "{}", change_line(change));
        if let Some(note) = &change.note {
            let _ = writeln!(out, "      {note}");
        }
    }
}

fn hash(sha: &Option<String>) -> String {
    sha.as_deref()
        .map_or_else(|| "none".into(), |s| s.chars().take(12).collect())
}

pub fn render_text(report: &DiffReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "faris diff ({})", report.schema);
    let _ = writeln!(out, "  A: {}", report.a);
    let _ = writeln!(out, "  B: {}", report.b);
    let _ = writeln!(out, "  compared: {}", report.compared);
    if report.identical {
        let _ = writeln!(
            out,
            "\nNo differences. Key order, number spelling and white space are not differences."
        );
        return out;
    }
    let s = &report.summary;
    let cost = s.highest_cost.map_or_else(
        || "no input or evidence change".to_string(),
        |c| format!("the dearest change is: {}", c.words()),
    );
    let _ = writeln!(
        out,
        "\n{} inputs changed, {} evidence changed, {} results changed, {} presentation only; {cost}.",
        s.inputs_changed, s.evidence_changed, s.results_changed, s.presentation_changed
    );
    if s.not_listed > 0 {
        let _ = writeln!(
            out,
            "{} more changes are counted but not listed (the limit per document).",
            s.not_listed
        );
    }
    section(&mut out, "INPUTS CHANGED (causes)", &report.inputs);
    section(&mut out, "EVIDENCE CHANGED", &report.evidence);
    let _ = writeln!(
        out,
        "\nRESULTS CHANGED (effects) ({})",
        report.results.len()
    );
    if report.results.is_empty() {
        let _ = writeln!(out, "  none");
    }
    for result in &report.results {
        let cause = if result.caused_by.is_empty() {
            "no input change found".to_string()
        } else {
            format!("caused by {}", result.caused_by.join(", "))
        };
        let _ = writeln!(
            out,
            "  {} {}: {} ({} -> {}); {cause}",
            result.id,
            result.item,
            format!("{:?}", result.kind).to_lowercase(),
            hash(&result.old_sha256),
            hash(&result.new_sha256),
        );
        for value in &result.values {
            let _ = writeln!(out, "  {}", change_line(value));
        }
    }
    section(
        &mut out,
        "PRESENTATION ONLY (labels, colours, view)",
        &report.presentation,
    );
    if !report.components.is_empty() {
        let _ = writeln!(out, "\nCOMPONENTS ({})", report.components.len());
        for c in &report.components {
            let volume = match (c.volume_old_m3, c.volume_new_m3) {
                (Some(a), Some(b)) => format!(", volume {a:.6} -> {b:.6} m^3"),
                (None, Some(b)) => format!(", volume {b:.6} m^3"),
                (Some(a), None) => format!(", volume was {a:.6} m^3"),
                _ => String::new(),
            };
            let material = match (&c.material_old, &c.material_new) {
                (Some(a), Some(b)) if a != b => format!(", material {a} -> {b}"),
                _ => String::new(),
            };
            let _ = writeln!(
                out,
                "  {}/{}: {}{material}{volume}",
                c.variant,
                c.component,
                c.changes.join(", ")
            );
        }
    }
    let _ = writeln!(out, "\nWHAT THIS INVALIDATES AND WHAT STAYS REUSABLE");
    if report.impact.is_empty() {
        let _ = writeln!(out, "  no results are recorded to judge");
    }
    for item in &report.impact {
        match item.standing {
            Standing::Invalidated => {
                let cost = item.cost.map_or("", |c| c.words());
                let _ = writeln!(
                    out,
                    "  invalidated  {} (to get it again: {cost}; because {})",
                    item.item,
                    item.because.join(", ")
                );
            }
            Standing::Reusable => {
                let _ = writeln!(out, "  reusable     {}", item.item);
            }
        }
    }
    out
}
