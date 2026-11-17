//! Automatische Sicherungen der connections.json.
//!
//! Gesichert wird die *bisherige* Datei, bevor sie überschrieben wird – beim ersten Speichern
//! pro Programmstart, danach höchstens stündlich und immer vor riskanten Aktionen.
//! Identische Stände werden nicht doppelt abgelegt. Aufbewahrt werden die letzten N Sicherungen
//! und zusätzlich je eine pro Tag der letzten 14 Tage.

use crate::model::Store;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const PREFIX: &str = "connections_";
const DAYS_KEPT: usize = 14;

#[derive(Clone, Debug)]
pub struct BackupInfo {
    pub path: PathBuf,
    /// "2026-11-16 09:30:05"
    pub when: String,
    /// Anlass, z. B. "vor Import" (leer = automatisch)
    pub reason: String,
    pub folders: usize,
    pub connections: usize,
    pub readable: bool,
}

pub fn backup_dir() -> PathBuf {
    Store::data_dir().join("backups")
}

/// Sicherungsdateien in `dir`, neueste zuerst (Dateinamen sind nach Zeit sortierbar).
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                        n.starts_with(PREFIX)
                            && n.ends_with(".json")
                            && n.as_bytes().get(PREFIX.len()).is_some_and(u8::is_ascii_digit)
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    v
}

/// "2026-11-16" aus dem Dateinamen
fn day_of(p: &Path) -> String {
    p.file_name().and_then(|n| n.to_str()).and_then(|n| n.get(PREFIX.len()..PREFIX.len() + 10)).unwrap_or("").to_string()
}

/// Sichert `source` nach `dir`, sofern sie existiert und sich vom neuesten Stand unterscheidet.
/// `stamp`: lokale Zeit "2026-11-16_09-30-05". Gibt den Pfad der neuen Sicherung zurück.
pub fn create(source: &Path, dir: &Path, stamp: &str, reason: Option<&str>) -> Result<Option<PathBuf>, String> {
    let data = match std::fs::read(source) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Sicherung: Datei nicht lesbar: {e}")),
    };
    if data.is_empty() {
        return Ok(None);
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("Sicherungsordner nicht anlegbar: {e}"))?;
    if let Some(newest) = files(dir).first() {
        if std::fs::read(newest).is_ok_and(|old| old == data) {
            return Ok(None); // unverändert
        }
    }
    let suffix = reason.map(|r| format!("_{}", r.replace(' ', "-"))).unwrap_or_default();
    let mut path = dir.join(format!("{PREFIX}{stamp}{suffix}.json"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{PREFIX}{stamp}{suffix}_{n}.json"));
        n += 1;
    }
    std::fs::write(&path, &data).map_err(|e| format!("Sicherung fehlgeschlagen: {e}"))?;
    Ok(Some(path))
}

/// Löscht alte Sicherungen: behalten werden die neuesten `keep` und je Tag die neueste der letzten 14 Tage.
pub fn prune(dir: &Path, keep: usize) {
    let all = files(dir);
    let mut days_seen = HashSet::new();
    for (i, p) in all.iter().enumerate() {
        let day = day_of(p);
        let first_of_day = days_seen.insert(day.clone());
        let daily = first_of_day && days_seen.len() <= DAYS_KEPT;
        if i >= keep.max(1) && !daily {
            let _ = std::fs::remove_file(p);
        }
    }
}

fn count(f: &crate::model::Folder) -> (usize, usize) {
    f.folders.iter().fold((f.folders.len(), f.connections.len()), |(a, b), s| {
        let (x, y) = count(s);
        (a + x, b + y)
    })
}

/// Liest eine Sicherung (BOM wird toleriert)
pub fn read(path: &Path) -> Result<Store, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str::<Store>(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())
}

pub fn list(dir: &Path) -> Vec<BackupInfo> {
    files(dir)
        .into_iter()
        .map(|path| {
            let name = path.file_stem().and_then(|n| n.to_str()).unwrap_or("").to_string();
            let rest = &name[PREFIX.len().min(name.len())..];
            let ts = rest.get(..19).unwrap_or(rest);
            let when = ts.replacen('_', " ", 1).replace('-', ":").replacen(':', "-", 2);
            let reason = rest.get(20..).unwrap_or("").replace('-', " ");
            let (readable, folders, connections) = match read(&path) {
                Ok(s) => {
                    let (f, c) = count(&s.root);
                    (true, f, c)
                }
                Err(_) => (false, 0, 0),
            };
            BackupInfo { path, when, reason, folders, connections, readable }
        })
        .collect()
}

/// Neueste lesbare Sicherung (zum Wiederherstellen einer beschädigten Datei)
pub fn newest_readable(dir: &Path) -> Option<(PathBuf, Store)> {
    files(dir).into_iter().find_map(|p| read(&p).ok().map(|s| (p, s)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rdpm_backup_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn create_skips_unchanged_and_lists() {
        let d = tmp("create");
        let src = d.join("connections.json");
        let bdir = d.join("backups");
        // keine Datei → nichts zu sichern
        assert!(create(&src, &bdir, "2026-11-16_09-00-00", None).unwrap().is_none());
        let mut s = Store::default();
        s.root.connections.push(crate::model::Connection { id: 1, name: "a".into(), ..Default::default() });
        std::fs::write(&src, serde_json::to_string(&s).unwrap()).unwrap();
        assert!(create(&src, &bdir, "2026-11-16_09-00-00", None).unwrap().is_some());
        // unverändert → keine zweite Sicherung
        assert!(create(&src, &bdir, "2026-11-16_10-00-00", None).unwrap().is_none());
        std::fs::write(&src, b"{\"root\":{}}").unwrap();
        let p = create(&src, &bdir, "2026-11-16_11-00-00", Some("vor Import")).unwrap().unwrap();
        assert!(p.file_name().unwrap().to_str().unwrap().ends_with("_vor-Import.json"));
        let l = list(&bdir);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].when, "2026-11-16 11:00:00");
        assert_eq!(l[0].reason, "vor Import");
        assert_eq!(l[1].connections, 1);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn prune_keeps_recent_and_daily() {
        let d = tmp("prune");
        // 3 Tage × 10 Sicherungen
        for day in 1..=3 {
            for h in 0..10 {
                std::fs::write(d.join(format!("{PREFIX}2026-11-0{day}_{h:02}-00-00.json")), "{}").unwrap();
            }
        }
        std::fs::write(d.join("connections_defekt_2026-11-01.json"), "x").unwrap();
        prune(&d, 5);
        let left: Vec<String> = files(&d).iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        // 5 neueste (Tag 3, 09–05 Uhr) + neueste von Tag 2 und Tag 1
        assert_eq!(left.len(), 7, "{left:?}");
        assert!(left.contains(&"connections_2026-11-02_09-00-00.json".to_string()));
        assert!(left.contains(&"connections_2026-11-01_09-00-00.json".to_string()));
        assert!(d.join("connections_defekt_2026-11-01.json").exists(), "andere Dateien bleiben");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn newest_readable_skips_broken() {
        let d = tmp("readable");
        std::fs::write(d.join(format!("{PREFIX}2026-11-01_09-00-00.json")), "{\"next_id\":42}").unwrap();
        std::fs::write(d.join(format!("{PREFIX}2026-11-02_09-00-00.json")), "kaputt").unwrap();
        let (p, s) = newest_readable(&d).unwrap();
        assert_eq!(s.next_id, 42);
        assert!(p.to_string_lossy().contains("2026-11-01"));
        let _ = std::fs::remove_dir_all(d);
    }
}
