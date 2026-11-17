//! Verbindungsprotokoll: wer hat wann welche Verbindung wie lange genutzt.
//!
//! Gespeichert als JSON-Zeilen in %APPDATA%\RdpManager\verbindungsprotokoll.jsonl –
//! je Sitzung eine Zeile beim Öffnen und eine beim Verbinden/Fehler/Schließen.

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::PathBuf;

pub fn file() -> PathBuf {
    crate::model::Store::data_dir().join("verbindungsprotokoll.jsonl")
}

const MAX_BYTES: u64 = 5 * 1024 * 1024;

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
struct Line {
    id: String,
    /// open | connected | failed | closed
    ev: String,
    t: u64,
    #[serde(skip_serializing_if = "String::is_empty")]
    name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    host: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    protocol: String,
    /// Anmeldename auf dem Zielsystem
    #[serde(skip_serializing_if = "String::is_empty")]
    login: String,
    /// Windows-Benutzer, der die Verbindung geöffnet hat
    #[serde(skip_serializing_if = "String::is_empty")]
    user: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    info: String,
}

/// Eine Sitzung im Protokoll
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub start: u64,
    pub connected: Option<u64>,
    pub end: Option<u64>,
    pub name: String,
    pub host: String,
    pub protocol: String,
    pub login: String,
    pub user: String,
    /// Ergebnis bzw. Fehlermeldung
    pub result: String,
}

impl Entry {
    /// Dauer der Verbindung in Sekunden (ab Verbindungsaufbau)
    pub fn duration(&self) -> Option<u64> {
        Some(self.end?.saturating_sub(self.connected.unwrap_or(self.start)))
    }
}

fn append(line: &Line) {
    let path = file();
    let _ = std::fs::create_dir_all(crate::model::Store::data_dir());
    // zu groß → ältere Hälfte verwerfen
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES)
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        let lines: Vec<&str> = text.lines().collect();
        let keep = lines[lines.len() / 2..].join("\n") + "\n";
        let _ = std::fs::write(&path, keep);
    }
    if let (Ok(json), Ok(mut f)) =
        (serde_json::to_string(line), std::fs::OpenOptions::new().create(true).append(true).open(&path))
    {
        let _ = writeln!(f, "{json}");
    }
}

fn windows_user() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(d) if !d.is_empty() => format!("{d}\\{user}"),
        _ => user,
    }
}

/// Sitzung beginnt; liefert die Protokoll-ID
pub fn open(name: &str, host: &str, protocol: &str, login: &str, info: &str) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    append(&Line {
        id: id.clone(),
        ev: "open".into(),
        t: now(),
        name: name.into(),
        host: host.into(),
        protocol: protocol.into(),
        login: login.into(),
        user: windows_user(),
        info: info.into(),
    });
    id
}

pub fn event(id: &str, ev: &str, info: &str) {
    append(&Line { id: id.into(), ev: ev.into(), t: now(), info: info.into(), ..Default::default() });
}

/// Alle Einträge, neueste zuerst
pub fn read() -> Vec<Entry> {
    let text = std::fs::read_to_string(file()).unwrap_or_default();
    parse(&text)
}

fn parse(text: &str) -> Vec<Entry> {
    let mut order: Vec<String> = Vec::new();
    let mut map: std::collections::HashMap<String, Entry> = Default::default();
    for l in text.lines().filter_map(|l| serde_json::from_str::<Line>(l).ok()) {
        if l.ev == "open" {
            order.push(l.id.clone());
            map.insert(
                l.id.clone(),
                Entry {
                    start: l.t,
                    name: l.name,
                    host: l.host,
                    protocol: l.protocol,
                    login: l.login,
                    user: l.user,
                    result: if l.info.is_empty() { "nicht beendet".into() } else { l.info },
                    ..Default::default()
                },
            );
            continue;
        }
        let Some(e) = map.get_mut(&l.id) else { continue };
        match l.ev.as_str() {
            "connected" => {
                e.connected = Some(l.t);
                e.result = "verbunden".into();
            }
            "failed" => e.result = format!("Fehler: {}", l.info),
            "closed" => {
                e.end = Some(l.t);
                if e.connected.is_some() && !e.result.starts_with("Fehler") {
                    e.result = "beendet".into();
                } else if e.result == "nicht beendet" {
                    e.result = "abgebrochen".into();
                }
            }
            _ => {}
        }
    }
    order.iter().rev().filter_map(|id| map.remove(id)).collect()
}

// ---------------------------------------------------------------------------
// Zeitformat (lokal)
// ---------------------------------------------------------------------------

pub fn local_time(secs: u64) -> String {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ft_val = (secs + 11_644_473_600) * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ft_val as u32, dwHighDateTime: (ft_val >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err() || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err() {
            return secs.to_string();
        }
    }
    format!("{:02}.{:02}.{} {:02}:{:02}", local.wDay, local.wMonth, local.wYear, local.wHour, local.wMinute)
}

pub fn duration_text(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 { format!("{h} h {m:02} min") } else if m > 0 { format!("{m} min {s:02} s") } else { format!("{s} s") }
}

fn csv_escape(s: &str) -> String {
    if s.contains([';', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

pub fn to_csv(entries: &[Entry]) -> String {
    let mut out = String::from("Beginn;Ende;Dauer (s);Verbindung;Host;Protokoll;Anmeldung;Windows-Benutzer;Ergebnis\r\n");
    for e in entries {
        let cols = [
            local_time(e.start),
            e.end.map(local_time).unwrap_or_default(),
            e.duration().map(|d| d.to_string()).unwrap_or_default(),
            e.name.clone(),
            e.host.clone(),
            e.protocol.clone(),
            e.login.clone(),
            e.user.clone(),
            e.result.clone(),
        ];
        out.push_str(&cols.iter().map(|c| csv_escape(c)).collect::<Vec<_>>().join(";"));
        out.push_str("\r\n");
    }
    out
}

// ---------------------------------------------------------------------------
// Fenster
// ---------------------------------------------------------------------------

pub struct Viewer {
    entries: Vec<Entry>,
    filter: String,
    confirm_clear: bool,
}

impl Viewer {
    pub fn new() -> Self {
        Self { entries: read(), filter: String::new(), confirm_clear: false }
    }

    /// Zeichnet das Fenster; `enabled` = Protokoll führen. Rückgabe: (offen bleiben, Einstellung geändert)
    pub fn ui(&mut self, ctx: &egui::Context, enabled: &mut bool) -> (bool, bool) {
        let mut open = true;
        let mut changed = false;
        let mut msg: Option<String> = None;
        egui::Window::new("📜 Verbindungsprotokoll")
            .open(&mut open)
            .default_size([900.0, 480.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    changed |= ui.checkbox(enabled, "Protokoll führen").changed();
                    ui.separator();
                    ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("🔍 Filtern…").desired_width(200.0));
                    if ui.button("🔄").on_hover_text("Neu laden").clicked() {
                        self.entries = read();
                    }
                    if ui.button("📤 Als CSV speichern…").clicked()
                        && let Some(p) = rfd::FileDialog::new()
                            .set_title("Verbindungsprotokoll speichern")
                            .set_file_name("verbindungsprotokoll.csv")
                            .add_filter("CSV", &["csv"])
                            .save_file()
                    {
                        let list = self.filtered();
                        // UTF-8 mit BOM, damit Excel Umlaute richtig anzeigt
                        let data = format!("\u{feff}{}", to_csv(&list));
                        msg = Some(match std::fs::write(&p, data) {
                            Ok(()) => format!("{} Einträge gespeichert", list.len()),
                            Err(e) => format!("Fehler: {e}"),
                        });
                    }
                    if self.confirm_clear {
                        if ui.button(egui::RichText::new("Wirklich alles löschen").color(egui::Color32::from_rgb(220, 70, 70))).clicked() {
                            let _ = std::fs::remove_file(file());
                            self.entries.clear();
                            self.confirm_clear = false;
                        }
                        if ui.button("Abbrechen").clicked() {
                            self.confirm_clear = false;
                        }
                    } else if ui.button("🗑 Leeren").clicked() {
                        self.confirm_clear = true;
                    }
                });
                if let Some(m) = &msg {
                    ui.label(m);
                }
                ui.separator();
                let list = self.filtered();
                ui.weak(format!("{} Einträge", list.len()));
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    egui::Grid::new("connlog_grid").striped(true).num_columns(7).spacing([14.0, 4.0]).show(ui, |ui| {
                        for h in ["Beginn", "Dauer", "Verbindung", "Host", "Protokoll", "Anmeldung", "Ergebnis"] {
                            ui.strong(h);
                        }
                        ui.end_row();
                        for e in list.iter().take(3000) {
                            ui.label(local_time(e.start)).on_hover_text(format!("Windows-Benutzer: {}", e.user));
                            ui.label(e.duration().map(duration_text).unwrap_or_else(|| "–".into()));
                            ui.label(&e.name);
                            ui.label(&e.host);
                            ui.label(&e.protocol);
                            ui.label(&e.login);
                            let color = if e.result.starts_with("Fehler") {
                                egui::Color32::from_rgb(220, 70, 70)
                            } else {
                                ui.visuals().text_color()
                            };
                            ui.colored_label(color, &e.result);
                            ui.end_row();
                        }
                    });
                });
            });
        (open, changed)
    }

    fn filtered(&self) -> Vec<Entry> {
        let terms: Vec<String> = self.filter.to_lowercase().split_whitespace().map(String::from).collect();
        self.entries
            .iter()
            .filter(|e| {
                let hay = format!("{} {} {} {} {} {}", e.name, e.host, e.protocol, e.login, e.user, e.result).to_lowercase();
                terms.iter().all(|t| hay.contains(t))
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_lines() {
        let text = [
            r#"{"id":"a","ev":"open","t":100,"name":"Srv","host":"srv","protocol":"RDP","login":"corp\\jdoe","user":"CORP\\jdoe"}"#,
            r#"{"id":"b","ev":"open","t":110,"name":"Lin","host":"lin","protocol":"SSH"}"#,
            r#"{"id":"a","ev":"connected","t":105}"#,
            r#"{"id":"b","ev":"failed","t":115,"info":"Zeitüberschreitung"}"#,
            r#"{"id":"a","ev":"closed","t":3705}"#,
            r#"{"id":"b","ev":"closed","t":120}"#,
            "kaputt",
        ]
        .join("\n");
        let e = parse(&text);
        assert_eq!(e.len(), 2);
        assert_eq!((e[0].name.as_str(), e[0].result.as_str()), ("Lin", "Fehler: Zeitüberschreitung"));
        assert_eq!((e[1].duration(), e[1].result.as_str()), (Some(3600), "beendet"));
        assert_eq!(duration_text(3600), "1 h 00 min");
        assert!(to_csv(&e).contains("Srv;srv;RDP;corp\\jdoe;CORP\\jdoe;beendet"));
    }
}
