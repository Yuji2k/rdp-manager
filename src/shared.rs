//! Gemeinsame Verbindungslisten: eine connections.json (RDP Manager) oder confCons.xml (mRemoteNG),
//! z. B. auf einem Netzlaufwerk. Sie werden schreibgeschützt im Baum angezeigt, beim Start und
//! danach regelmäßig im Hintergrund neu eingelesen, sobald sich die Datei geändert hat.
//!
//! Die Verbindungen erhalten stabile IDs (aus Dateipfad, Ordnerpfad, Name und Host berechnet),
//! damit z. B. ein gespeichertes Passwort nach dem Neuladen weiter passt.

use crate::model::{Folder, Store};
use eframe::egui;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant, SystemTime};

/// Kennzeichen-Bit für IDs aus gemeinsamen Listen
pub const SHARED_BIT: u64 = 1 << 62;
/// Wie oft auf Änderungen geprüft wird
const CHECK_EVERY: Duration = Duration::from_secs(60);

struct Loaded {
    root: Folder,
    passwords: HashMap<u64, String>,
    mtime: Option<SystemTime>,
}

enum LoadResult {
    Unchanged,
    Loaded(Loaded),
    Failed(String),
}

pub struct SharedTree {
    pub name: String,
    pub path: String,
    pub root: Option<Folder>,
    pub error: Option<String>,
    /// Passwörter aus der mRemoteNG-Datei (nur im Speicher)
    pub passwords: HashMap<u64, String>,
    mtime: Option<SystemTime>,
    pending: Option<Receiver<LoadResult>>,
    last_check: Instant,
}

impl SharedTree {
    pub fn new(name: &str, path: &str, ctx: &egui::Context) -> Self {
        let mut t = Self {
            name: name.to_string(),
            path: path.to_string(),
            root: None,
            error: None,
            passwords: HashMap::new(),
            mtime: None,
            pending: None,
            last_check: Instant::now(),
        };
        t.reload(true, ctx);
        t
    }

    pub fn is_loading(&self) -> bool {
        self.pending.is_some()
    }

    /// Im Hintergrund (neu) laden. `force`: auch wenn sich die Datei nicht geändert hat.
    pub fn reload(&mut self, force: bool, ctx: &egui::Context) {
        if self.pending.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let (path, known) = (self.path.clone(), if force { None } else { self.mtime });
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(load(&path, known));
            ctx.request_repaint();
        });
        self.pending = Some(rx);
        self.last_check = Instant::now();
    }

    /// Ergebnisse übernehmen und regelmäßig auf Änderungen prüfen.
    pub fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(LoadResult::Loaded(l)) => {
                    self.root = Some(l.root);
                    self.passwords = l.passwords;
                    self.mtime = l.mtime;
                    self.error = None;
                    self.pending = None;
                }
                Ok(LoadResult::Unchanged) => self.pending = None,
                Ok(LoadResult::Failed(e)) => {
                    self.error = Some(e);
                    self.pending = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(_) => self.pending = None,
            }
        } else if self.last_check.elapsed() >= CHECK_EVERY {
            self.reload(false, ctx);
        }
    }
}

fn load(path: &str, known_mtime: Option<SystemTime>) -> LoadResult {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    if known_mtime.is_some() && mtime == known_mtime {
        return LoadResult::Unchanged;
    }
    let text = match std::fs::read_to_string(path) {
        // Byte-Order-Mark (Editor/PowerShell) entfernen
        Ok(t) => t.trim_start_matches('\u{feff}').to_string(),
        Err(e) => return LoadResult::Failed(format!("Datei nicht lesbar: {e}")),
    };
    let is_xml = path.to_ascii_lowercase().ends_with(".xml") || text.trim_start().starts_with('<');
    let (mut root, passwords) = if is_xml {
        let mut n = 0u64;
        let mut next = || {
            n += 1;
            n
        };
        match crate::mremote::import(&text, None, &mut next) {
            Ok(imp) => (imp.root, imp.passwords.into_iter().collect::<HashMap<_, _>>()),
            Err(crate::mremote::ImportError::NeedsPassword) => {
                return LoadResult::Failed(
                    "Die mRemoteNG-Datei ist mit einem Master-Passwort geschützt – bitte ohne Master-Passwort exportieren.".into(),
                );
            }
            Err(crate::mremote::ImportError::Other(e)) => return LoadResult::Failed(e),
        }
    } else {
        match serde_json::from_str::<Store>(&text) {
            Ok(s) => (s.root, HashMap::new()),
            Err(e) => return LoadResult::Failed(format!("Keine gültige Verbindungsdatei: {e}")),
        }
    };
    let mut id_map = HashMap::new();
    assign_ids(&mut root, path, "", &mut id_map);
    let passwords = passwords.into_iter().filter_map(|(old, pw)| id_map.get(&old).map(|new| (*new, pw))).collect();
    LoadResult::Loaded(Loaded { root, passwords, mtime })
}

/// FNV-1a, damit die IDs über Programmversionen hinweg gleich bleiben
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

fn stable_id(key: &str) -> u64 {
    SHARED_BIT | (fnv(key) & (SHARED_BIT - 1))
}

fn assign_ids(f: &mut Folder, source: &str, prefix: &str, map: &mut HashMap<u64, u64>) {
    let src = source.to_lowercase();
    for c in &mut f.connections {
        let new = stable_id(&format!("C|{src}|{prefix}|{}|{}|{}", c.name, c.host.trim().to_lowercase(), c.port));
        map.insert(c.id, new);
        c.id = new;
    }
    for sub in &mut f.folders {
        let path = format!("{prefix}/{}", sub.name);
        let new = stable_id(&format!("F|{src}|{path}"));
        map.insert(sub.id, new);
        sub.id = new;
        assign_ids(sub, source, &path, map);
    }
    if prefix.is_empty() {
        f.id = stable_id(&format!("R|{src}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Connection;

    #[test]
    fn load_json_with_stable_ids() {
        let dir = std::env::temp_dir().join(format!("rdpm_shared_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("team.json");
        let mut store = Store::default();
        store.root.folders.push(Folder {
            id: 5,
            name: "Server".into(),
            connections: vec![Connection { id: 7, name: "dc01".into(), host: "dc01".into(), ..Default::default() }],
            ..Default::default()
        });
        // mit BOM, wie sie Windows-Editoren gern schreiben
        std::fs::write(&path, format!("\u{feff}{}", serde_json::to_string(&store).unwrap())).unwrap();
        let p = path.display().to_string();
        let LoadResult::Loaded(a) = load(&p, None) else { panic!("laden") };
        let id = a.root.folders[0].connections[0].id;
        assert!(id & SHARED_BIT != 0);
        // erneut laden → gleiche ID; unverändert → nichts zu tun
        let LoadResult::Loaded(b) = load(&p, None) else { panic!("laden") };
        assert_eq!(b.root.folders[0].connections[0].id, id);
        assert!(matches!(load(&p, a.mtime), LoadResult::Unchanged));
        assert!(matches!(load(&dir.join("fehlt.json").display().to_string(), None), LoadResult::Failed(_)));
        let _ = std::fs::remove_dir_all(dir);
    }
}
