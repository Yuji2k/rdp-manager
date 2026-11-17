//! Erreichbarkeit der Verbindungen (TCP-Verbindungsaufbau zum RDP-/SSH-Port).
//!
//! Geprüft wird nur, was im Baum gerade sichtbar ist (also erst, wenn ein Ordner geöffnet wird),
//! und zwar nach und nach: wenige Hintergrund-Threads arbeiten eine Warteschlange ab.
//! Aufträge für Zeilen, die inzwischen nicht mehr sichtbar sind (Ordner zugeklappt,
//! weggescrollt), werden verworfen. Ergebnisse gelten eine Minute, danach wird neu geprüft.

use eframe::egui;
use std::collections::{HashMap, VecDeque};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Gleichzeitige Prüfungen
const WORKERS: usize = 3;
/// Wartezeit pro Verbindungsversuch
const TIMEOUT: Duration = Duration::from_millis(1500);
/// So lange gilt ein Ergebnis
const MAX_AGE: Duration = Duration::from_secs(60);
/// Aufträge, deren Zeile so lange nicht mehr gezeichnet wurde, werden verworfen
const WANTED_FOR: Duration = Duration::from_millis(1500);

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Up(u32),
    Down(String),
}

#[derive(Default)]
struct Entry {
    status: Option<Status>,
    checked: Option<Instant>,
    queued: bool,
    wanted: Option<Instant>,
}

/// Ergebnisse und Warteschlange liegen bewusst unter EINER Sperre:
/// Zwei getrennte Mutexe wurden von Oberfläche und Prüf-Threads in unterschiedlicher
/// Reihenfolge gesperrt und konnten sich gegenseitig blockieren (Programm „reagiert nicht“).
#[derive(Default)]
struct State {
    map: HashMap<(String, u16), Entry>,
    queue: VecDeque<(String, u16)>,
}

struct Shared {
    state: Mutex<State>,
    signal: Condvar,
    ctx: egui::Context,
}

pub struct Reach {
    shared: Arc<Shared>,
}

impl Reach {
    pub fn new(ctx: &egui::Context) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            signal: Condvar::new(),
            ctx: ctx.clone(),
        });
        for i in 0..WORKERS {
            let s = shared.clone();
            let _ = std::thread::Builder::new().name(format!("reach-{i}")).spawn(move || worker(&s));
        }
        Self { shared }
    }

    /// Liefert den bekannten Status und stellt die Prüfung in die Warteschlange,
    /// falls es noch kein (aktuelles) Ergebnis gibt. Nur für sichtbare Zeilen aufrufen.
    /// Rückgabe: (Status, wird gerade geprüft)
    pub fn check(&self, host: &str, port: u16) -> (Option<Status>, bool) {
        let host = host.trim();
        if host.is_empty() {
            return (None, false);
        }
        let key = (host.to_ascii_lowercase(), port);
        let now = Instant::now();
        let mut st = self.shared.state.lock().unwrap();
        let e = st.map.entry(key.clone()).or_default();
        e.wanted = Some(now);
        let stale = e.checked.is_none_or(|t| now.duration_since(t) > MAX_AGE);
        let enqueue = stale && !e.queued;
        if enqueue {
            e.queued = true;
        }
        let result = (e.status.clone(), e.queued);
        if enqueue {
            st.queue.push_back(key);
            self.shared.signal.notify_one();
        }
        result
    }

    /// Alle Ergebnisse verwerfen (z. B. „Jetzt neu prüfen“).
    pub fn clear(&self) {
        let mut st = self.shared.state.lock().unwrap();
        for e in st.map.values_mut() {
            e.checked = None;
        }
    }
}

fn worker(s: &Shared) {
    loop {
        // nächsten Auftrag holen, der noch gebraucht wird (die Sperre wird beim Prüfen nicht gehalten)
        let key = {
            let mut st = s.state.lock().unwrap();
            loop {
                match st.queue.pop_front() {
                    Some(k) => {
                        let e = st.map.entry(k.clone()).or_default();
                        if e.wanted.is_some_and(|t| t.elapsed() < WANTED_FOR) {
                            break k;
                        }
                        e.queued = false; // nicht mehr sichtbar – beim nächsten Anzeigen neu einreihen
                    }
                    None => st = s.signal.wait(st).unwrap(),
                }
            }
        };
        let status = probe(&key.0, key.1);
        if let Some(e) = s.state.lock().unwrap().map.get_mut(&key) {
            e.status = Some(status);
            e.checked = Some(Instant::now());
            e.queued = false;
        }
        s.ctx.request_repaint();
    }
}

fn probe(host: &str, port: u16) -> Status {
    let addrs: Vec<_> = match (host, port).to_socket_addrs() {
        Ok(a) => a.take(2).collect(),
        Err(_) => return Status::Down("Name nicht auflösbar".into()),
    };
    if addrs.is_empty() {
        return Status::Down("Name nicht auflösbar".into());
    }
    let start = Instant::now();
    let mut last = String::new();
    for a in addrs {
        match TcpStream::connect_timeout(&a, TIMEOUT) {
            Ok(_) => return Status::Up(start.elapsed().as_millis() as u32),
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => last = "keine Antwort".into(),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => last = format!("Port {port} abgelehnt"),
            Err(e) => last = e.to_string(),
        }
    }
    Status::Down(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_local() {
        // Ein lokaler Listener muss als erreichbar gelten, ein geschlossener Port nicht.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(matches!(probe("127.0.0.1", port), Status::Up(_)));
        drop(l);
        assert!(matches!(probe("127.0.0.1", port), Status::Down(_)));
        assert!(matches!(probe("diesen-host-gibt-es-nicht.invalid", 3389), Status::Down(_)));
    }

    /// Viele gleichzeitige Abfragen, während die Prüf-Threads Ergebnisse eintragen,
    /// dürfen nicht hängen bleiben (früher: Deadlock durch zwei Sperren).
    #[test]
    fn no_deadlock_under_load() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let reach = Arc::new(Reach::new(&egui::Context::default()));
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        for t in 0..4 {
            let r = reach.clone();
            let tx = done_tx.clone();
            std::thread::spawn(move || {
                let end = Instant::now() + Duration::from_millis(1500);
                let mut i = 0u32;
                while Instant::now() < end {
                    // wechselnde Ziele, damit ständig eingereiht und eingetragen wird
                    r.check("127.0.0.1", port);
                    r.check(&format!("127.0.0.{}", 2 + (i + t) % 50), port);
                    if i % 200 == 0 {
                        r.clear();
                    }
                    i += 1;
                }
                tx.send(()).unwrap();
            });
        }
        for _ in 0..4 {
            done_rx.recv_timeout(Duration::from_secs(10)).expect("Erreichbarkeitsprüfung blockiert (Deadlock)");
        }
        // das lokale Ziel muss inzwischen als erreichbar gelten
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let (Some(Status::Up(_)), _) = reach.check("127.0.0.1", port) {
                break;
            }
            assert!(Instant::now() < deadline, "kein Ergebnis");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
