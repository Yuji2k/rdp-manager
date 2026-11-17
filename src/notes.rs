//! Eingebaute Versionshinweise (aus CHANGELOG.md, beim Bauen in die exe übernommen).

use eframe::egui::{self, RichText};

const CHANGELOG: &str = include_str!("../CHANGELOG.md");

pub struct Release {
    pub version: String,
    /// Zeilen ohne die Überschrift
    pub lines: Vec<String>,
}

/// "0.1.10" → (0, 1, 10) für den Vergleich
pub fn parse_version(v: &str) -> (u32, u32, u32) {
    let mut it = v.trim().split('.').map(|p| p.trim().parse::<u32>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

pub fn releases() -> Vec<Release> {
    parse(CHANGELOG)
}

fn parse(text: &str) -> Vec<Release> {
    let mut out: Vec<Release> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            let version = rest.split_whitespace().next().unwrap_or("").to_string();
            out.push(Release { version, lines: Vec::new() });
        } else if let Some(r) = out.last_mut() {
            if !line.trim().is_empty() {
                r.lines.push(line.to_string());
            }
        }
    }
    out
}

/// Gibt es Versionen, die neuer sind als `last_seen`?
pub fn has_news(last_seen: &str) -> bool {
    let seen = parse_version(last_seen);
    releases().iter().any(|r| parse_version(&r.version) > seen)
}

/// Text mit **fett** und *kursiv* (einfaches Markdown) als egui-Zeile
fn rich_line(ui: &mut egui::Ui, text: &str, bullet: bool) {
    let mut job = egui::text::LayoutJob::default();
    let normal = egui::TextFormat {
        font_id: egui::TextStyle::Body.resolve(ui.style()),
        color: ui.visuals().text_color(),
        ..Default::default()
    };
    if bullet {
        job.append("•  ", 0.0, normal.clone());
    }
    // **fett** und *kursiv* auswerten
    let mut rest = text;
    while !rest.is_empty() {
        let (marker, strong) = match (rest.find("**"), rest.find('*')) {
            (Some(b), Some(i)) if b <= i => (b, true),
            (_, Some(i)) => (i, false),
            _ => {
                job.append(rest, 0.0, normal.clone());
                break;
            }
        };
        job.append(&rest[..marker], 0.0, normal.clone());
        let delim = if strong { "**" } else { "*" };
        let after = &rest[marker + delim.len()..];
        match after.find(delim) {
            Some(end) => {
                let mut f = normal.clone();
                if strong {
                    f.color = ui.visuals().strong_text_color();
                } else {
                    f.italics = true;
                }
                job.append(&after[..end], 0.0, f);
                rest = &after[end + delim.len()..];
            }
            None => {
                job.append(&rest[marker..], 0.0, normal.clone());
                break;
            }
        }
    }
    job.wrap.max_width = ui.available_width();
    ui.label(job);
}

/// Zeichnet alle Versionen; neuere als `last_seen` sind aufgeklappt und markiert.
pub fn show(ui: &mut egui::Ui, last_seen: &str) {
    let seen = parse_version(last_seen);
    for (i, r) in releases().iter().enumerate() {
        let is_new = !last_seen.is_empty() && parse_version(&r.version) > seen;
        let current = r.version == env!("CARGO_PKG_VERSION");
        let mut title = format!("Version {}", r.version);
        if current {
            title.push_str("  (installiert)");
        }
        let mut text = RichText::new(title).strong().size(15.0);
        if is_new {
            text = text.color(ui.visuals().selection.stroke.color);
        }
        egui::CollapsingHeader::new(text)
            .id_salt(("rdpm_release_notes", r.version.as_str()))
            .default_open(is_new || current || (last_seen.is_empty() && i == 0))
            .show(ui, |ui| {
                for line in &r.lines {
                    if let Some(h) = line.strip_prefix("### ") {
                        ui.add_space(4.0);
                        ui.label(RichText::new(h).strong());
                    } else if let Some(b) = line.trim_start().strip_prefix("- ") {
                        rich_line(ui, b, true);
                    } else {
                        rich_line(ui, line.trim(), false);
                    }
                }
            });
        if is_new {
            ui.add_space(2.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changelog_matches_version() {
        let r = releases();
        assert!(!r.is_empty());
        assert_eq!(r[0].version, env!("CARGO_PKG_VERSION"), "CHANGELOG.md muss mit der aktuellen Version beginnen");
        // absteigend sortiert
        for w in r.windows(2) {
            assert!(parse_version(&w[0].version) > parse_version(&w[1].version));
        }
        assert!(has_news("0.1.0"));
        assert!(!has_news(env!("CARGO_PKG_VERSION")));
        assert!(parse_version("0.1.10") > parse_version("0.1.9"));
        // Zeichen, die die eingebaute Schrift nicht kennt, würden als Kästchen erscheinen
        assert!(!CHANGELOG.contains('→'), "„→“ wird nicht dargestellt – „>“ verwenden");
    }
}
