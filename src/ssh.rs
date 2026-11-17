//! SSH-Sitzungen: Windows-OpenSSH (`ssh.exe`) läuft in einer Pseudo-Konsole (ConPTY),
//! die Ausgabe wird mit `vt100` emuliert und direkt mit egui im Tab gezeichnet.
//!
//! Vorteil: Schlüssel, ssh-agent, known_hosts und ~/.ssh/config funktionieren wie gewohnt.

use crate::model::Connection;
use crate::rdp::State;
use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

const SSH_EXE: &str = r"C:\Windows\System32\OpenSSH\ssh.exe";
const PAD: f32 = 4.0;

/// Tastenkombinationen für das SSH-Menü (Bezeichnung, Bytes)
pub const SSH_KEYS: &[(&str, &[u8])] = &[
    ("Strg+C (Abbrechen)", b"\x03"),
    ("Strg+D (EOF / Abmelden)", b"\x04"),
    ("Strg+Z (Anhalten)", b"\x1a"),
    ("Strg+L (Bildschirm leeren)", b"\x0c"),
    ("Strg+R (Verlauf durchsuchen)", b"\x12"),
    ("Strg+A", b"\x01"),
    ("Strg+K", b"\x0b"),
    ("Esc", b"\x1b"),
    ("Tab", b"\t"),
];

fn ssh_path() -> Result<&'static str, String> {
    if std::path::Path::new(SSH_EXE).exists() {
        Ok(SSH_EXE)
    } else {
        Err("OpenSSH-Client nicht gefunden. Installieren über Einstellungen > System > Optionale Features > „OpenSSH-Client“.".into())
    }
}

/// Einfache Aufteilung von Argumenten, "…" fasst Leerzeichen zusammen.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in s.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Argumente für ssh.exe aus den Verbindungseinstellungen
pub fn ssh_args(conn: &Connection) -> Vec<String> {
    let mut a = vec!["-p".to_string(), conn.port.to_string(), "-o".into(), "ServerAliveInterval=30".into()];
    if !conn.ssh_key_file.trim().is_empty() {
        a.push("-i".into());
        a.push(conn.ssh_key_file.trim().to_string());
    }
    if conn.ssh_agent_forward {
        a.push("-A".into());
    }
    for f in &conn.forwards {
        if let Some(fa) = f.args() {
            a.extend(fa);
        }
    }
    a.extend(split_args(&conn.ssh_extra_args));
    let user = if conn.username.trim().is_empty() { String::new() } else { conn.username.trim().to_string() };
    if !user.is_empty() {
        a.push("-l".into());
        a.push(user);
    }
    a.push(conn.host.trim().to_string());
    a
}

/// SSH in einem eigenen Konsolenfenster öffnen.
pub fn launch_external(conn: &Connection) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x10;
    std::process::Command::new(ssh_path()?)
        .args(ssh_args(conn))
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------- SSH-Agent

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Zustand des Windows-Dienstes „OpenSSH Authentication Agent“ (ssh-agent)
#[derive(Clone, Debug, PartialEq)]
pub enum AgentService {
    Running,
    Stopped,
    Disabled,
    Missing,
}

pub fn agent_service() -> AgentService {
    use std::os::windows::process::CommandExt;
    let run = |args: &[&str]| {
        std::process::Command::new("sc.exe")
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
    };
    let q = run(&["query", "ssh-agent"]);
    if !q.contains("STATE") {
        return AgentService::Missing;
    }
    if q.contains("RUNNING") {
        return AgentService::Running;
    }
    if run(&["qc", "ssh-agent"]).contains("DISABLED") { AgentService::Disabled } else { AgentService::Stopped }
}

/// Im Agent geladene Schlüssel (`ssh-add -l`)
pub fn agent_keys() -> Result<Vec<String>, String> {
    use std::os::windows::process::CommandExt;
    let exe = SSH_EXE.replace("ssh.exe", "ssh-add.exe");
    let out = std::process::Command::new(exe)
        .arg("-l")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("ssh-add konnte nicht gestartet werden: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    match out.status.code() {
        Some(0) => Ok(text.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect()),
        Some(1) => Ok(Vec::new()), // Agent läuft, aber ohne Schlüssel
        _ => Err(text.trim().to_string()),
    }
}

/// Öffnet ein Konsolenfenster mit `cmd` (Passphrase-Abfragen erfolgen dort).
pub fn run_in_console(program: &str, args: &[String]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x10;
    let dir = SSH_EXE.trim_end_matches("ssh.exe");
    let mut line = format!("\"{dir}{program}\"");
    for a in args {
        line.push_str(&format!(" \"{a}\""));
    }
    // Fenster bleibt bei Fehlern offen, damit die Meldung lesbar ist
    std::process::Command::new("cmd.exe")
        .raw_arg(format!("/c \"{line} || pause\""))
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Befehl, der einen öffentlichen Schlüssel auf dem Server in ~/.ssh/authorized_keys einträgt
/// (nur, wenn er dort noch nicht steht).
pub fn install_key_command(pubkey: &str) -> Result<String, String> {
    let key = pubkey.trim();
    let valid = ["ssh-", "ecdsa-", "sk-"].iter().any(|p| key.starts_with(p));
    if !valid || key.contains('\'') || key.contains('\n') {
        return Err("Das ist kein öffentlicher OpenSSH-Schlüssel (.pub-Datei mit einer Zeile wie „ssh-ed25519 AAAA… name“).".into());
    }
    Ok(format!(
        "mkdir -p ~/.ssh && chmod 700 ~/.ssh && touch ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys && \
         (grep -qxF '{key}' ~/.ssh/authorized_keys || echo '{key}' >> ~/.ssh/authorized_keys) && echo 'Schluessel eingetragen.'\r"
    ))
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct CellPos {
    row: u16,
    col: u16,
}

pub struct SshSession {
    pub id: u64,
    pub title: String,
    pub host: String,
    pub icon: &'static str,
    pub conn: Connection,
    parser: vt100::Parser,
    rx: Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    state: State,
    password: Option<String>,
    password_sent: bool,
    tail: String,
    size: (u16, u16),
    selection: Option<(CellPos, CellPos)>,
    started: Instant,
    /// zuletzt verwendeter Zielordner für Uploads
    pub remote_dir: String,
    /// Mitschnitt in eine Logdatei
    log: Option<(std::fs::File, PathBuf)>,
    /// Suchleiste (Strg+Umschalt+F)
    search: Option<Search>,
    /// Länge des Verlaufs (für die Zuordnung sichtbarer Zeilen zu Suchtreffern)
    history_len: usize,
    /// Der Server wartet gerade auf ein Passwort (z. B. sudo)
    pub password_prompt: bool,
    /// Benutzer möchte das Passwort senden (Strg+Umschalt+P oder Schaltfläche)
    pub wants_password: bool,
    /// Gleichzeitige Eingabe: Tastatureingaben auch an andere markierte Sitzungen
    pub mirror: bool,
    /// Eingaben, die an die anderen markierten Sitzungen weitergegeben werden sollen
    pub outbox: Vec<u8>,
}

#[derive(Default)]
struct Search {
    query: String,
    /// Treffer: (Zeile im Gesamtverlauf, Spalte, Länge in Zeichen)
    hits: Vec<(usize, usize, usize)>,
    current: Option<usize>,
    focus: bool,
}

impl SshSession {
    pub fn start(id: u64, conn: &Connection, password: Option<String>, ctx: &egui::Context) -> Result<Self, String> {
        let exe = ssh_path()?;
        let (rows, cols) = (30u16, 100u16);
        let pair = native_pty_system()
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| format!("Pseudo-Konsole konnte nicht erstellt werden: {e}"))?;
        let mut cmd = CommandBuilder::new(exe);
        cmd.args(ssh_args(conn));
        cmd.env("TERM", "xterm-256color");
        let child = pair.slave.spawn_command(cmd).map_err(|e| format!("ssh.exe konnte nicht gestartet werden: {e}"))?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 16384];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                }
            }
            ctx.request_repaint();
        });

        Ok(Self {
            id,
            title: conn.display_name().to_string(),
            host: conn.host.trim().to_string(),
            icon: conn.icon.glyph(),
            conn: conn.clone(),
            parser: vt100::Parser::new(NonZeroU16::new(rows).unwrap(), NonZeroU16::new(cols).unwrap(), 10_000),
            rx,
            writer,
            master: pair.master,
            child,
            state: State::Connected,
            password,
            password_sent: false,
            tail: String::new(),
            size: (rows, cols),
            selection: None,
            started: Instant::now(),
            remote_dir: String::new(),
            log: None,
            search: None,
            history_len: 0,
            password_prompt: false,
            wants_password: false,
            mirror: false,
            outbox: Vec::new(),
        })
    }

    /// Passwort, mit dem die Sitzung gestartet wurde (für scp)
    pub fn password(&self) -> Option<String> {
        self.password.clone()
    }

    // ------------------------------------------------------------------ Mitschnitt

    pub fn is_logging(&self) -> bool {
        self.log.is_some()
    }

    pub fn log_path(&self) -> Option<&std::path::Path> {
        self.log.as_ref().map(|(_, p)| p.as_path())
    }

    /// Startet den Mitschnitt in `%APPDATA%\RdpManager\logs\<host>_<datum>.log`.
    pub fn start_log(&mut self) -> Result<PathBuf, String> {
        if let Some((_, p)) = &self.log {
            return Ok(p.clone());
        }
        let dir = log_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let host: String = self.host.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
        let path = dir.join(format!("{host}_{}.log", local_timestamp()));
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("Logdatei konnte nicht angelegt werden: {e}"))?;
        // bisherigen Bildschirminhalt als Kontext voranstellen
        let _ = writeln!(f, "=== {} ({}) – Mitschnitt gestartet {} ===", self.title, self.host, local_timestamp());
        let _ = writeln!(f, "{}", self.parser.screen().contents().trim_end());
        self.log = Some((f, path.clone()));
        Ok(path)
    }

    pub fn stop_log(&mut self) {
        if let Some((mut f, _)) = self.log.take() {
            let _ = writeln!(f, "\n=== Mitschnitt beendet {} ===", local_timestamp());
        }
    }

    fn write_log(&mut self, bytes: &[u8]) {
        if let Some((f, _)) = &mut self.log {
            let text = strip_ansi(&String::from_utf8_lossy(bytes));
            let clean: String = text.chars().filter(|c| *c == '\n' || *c == '\t' || !c.is_control()).collect();
            let _ = f.write_all(clean.as_bytes());
        }
    }

    // ------------------------------------------------------------------ Suche

    pub fn open_search(&mut self) {
        let s = self.search.get_or_insert_with(Search::default);
        s.focus = true;
    }

    fn all_lines(&mut self) -> Vec<String> {
        all_lines(&mut self.parser, self.size.0, self.size.1)
    }

    fn run_search(&mut self) {
        let Some(query) = self.search.as_ref().map(|s| s.query.clone()) else { return };
        let hits = find_hits(&self.all_lines(), &query);
        if let Some(s) = &mut self.search {
            s.current = if hits.is_empty() { None } else { Some(hits.len() - 1) };
            s.hits = hits;
        }
        self.show_current_hit();
    }

    /// Springt zum nächsten (älteren, `back`) bzw. neueren Treffer.
    fn step_search(&mut self, back: bool) {
        if let Some(s) = &mut self.search {
            if s.hits.is_empty() {
                return;
            }
            let n = s.hits.len();
            s.current = Some(match s.current {
                Some(c) if back => (c + n - 1) % n,
                Some(c) => (c + 1) % n,
                None => n - 1,
            });
        }
        self.show_current_hit();
    }

    fn show_current_hit(&mut self) {
        let Some(line) = self.search.as_ref().and_then(|s| s.current.map(|c| s.hits[c].0)) else { return };
        let rows = self.size.0 as usize;
        let screen = self.parser.screen_mut();
        screen.set_scrollback(usize::MAX);
        let max = screen.scrollback();
        // Treffer etwa in die Mitte holen
        let off = (max as isize - line as isize + rows as isize / 2).clamp(0, max as isize) as usize;
        screen.set_scrollback(off);
    }

    /// Zeichnet die Suchleiste oben rechts im Terminal.
    fn search_bar(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let Some(mut s) = self.search.take() else { return };
        let mut close = false;
        let mut changed = false;
        let mut step = None;
        let bar = Rect::from_min_size(Pos2::new(rect.right() - 372.0, rect.top() + 4.0), Vec2::new(364.0, 34.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
            egui::Frame::popup(ui.style()).inner_margin(egui::Margin::symmetric(6, 3)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut s.query).hint_text("Im Verlauf suchen…").desired_width(150.0));
                    if std::mem::take(&mut s.focus) {
                        r.request_focus();
                    }
                    changed = r.changed();
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        step = Some(!ui.input(|i| i.modifiers.shift));
                        r.request_focus();
                    }
                    if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        close = true;
                    }
                    match (s.current, s.hits.len()) {
                        (_, 0) if !s.query.is_empty() => ui.weak("keine Treffer"),
                        (Some(c), n) => ui.label(format!("{}/{n}", c + 1)),
                        _ => ui.label(""),
                    };
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("✖").on_hover_text("Schließen (Esc)").clicked() {
                            close = true;
                        }
                        if ui.small_button("⏷").on_hover_text("Neuerer Treffer (Umschalt+Enter)").clicked() {
                            step = Some(false);
                        }
                        if ui.small_button("⏶").on_hover_text("Älterer Treffer (Enter)").clicked() {
                            step = Some(true);
                        }
                    });
                });
            });
        });
        if close {
            self.parser.screen_mut().set_scrollback(0);
            return; // Suche bleibt geschlossen
        }
        self.search = Some(s);
        if changed {
            self.run_search();
        } else if let Some(back) = step {
            // Ausgabe kann sich geändert haben → Treffer neu ermitteln, Position beibehalten
            let cur = self.search.as_ref().and_then(|s| s.current);
            self.run_search();
            if let (Some(c), Some(s)) = (cur, &mut self.search) {
                if c < s.hits.len() {
                    s.current = Some(c);
                }
            }
            self.step_search(back);
        }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn seconds_running(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// Ausgabe verarbeiten, Passwort-Prompt erkennen, Prozessende prüfen.
    pub fn update(&mut self) {
        let mut got = false;
        while let Ok(bytes) = self.rx.try_recv() {
            self.parser.process(&bytes);
            self.answer_queries(&bytes);
            self.write_log(&bytes);
            self.tail.push_str(&String::from_utf8_lossy(&bytes));
            if self.tail.len() > 400 {
                let cut = self.tail.len() - 400;
                let cut = (cut..self.tail.len()).find(|i| self.tail.is_char_boundary(*i)).unwrap_or(0);
                self.tail.drain(..cut);
            }
            got = true;
        }
        if got {
            let t = strip_ansi(&self.tail);
            // Anmeldung: gespeichertes Passwort einmalig automatisch eingeben
            if !self.password_sent && t.to_lowercase().trim_end().ends_with("password:") {
                if let Some(pw) = self.password.clone() {
                    self.send(format!("{pw}\r").as_bytes());
                    self.password_sent = true;
                    self.tail.clear();
                    return;
                }
            }
            // spätere Abfragen (sudo, su …) nur anzeigen – gesendet wird erst auf Wunsch
            self.password_prompt = is_password_prompt(&t);
        }
        if matches!(self.state, State::Connected | State::Connecting) {
            if let Ok(Some(status)) = self.child.try_wait() {
                self.state = if status.success() {
                    State::Closed
                } else {
                    State::Failed(format!("ssh wurde mit Code {} beendet.", status.exit_code()))
                };
            }
        }
    }

    /// Beantwortet Statusabfragen des Hosts. Wichtig: ConPTY fragt beim Start die Cursorposition
    /// ab (ESC[6n) und wartet, bis das Terminal antwortet.
    fn answer_queries(&mut self, bytes: &[u8]) {
        let find = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
        if find(b"\x1b[6n") {
            let (r, c) = self.parser.screen().cursor_position();
            self.write_raw(format!("\x1b[{};{}R", r + 1, c + 1).as_bytes());
        }
        if find(b"\x1b[5n") {
            self.write_raw(b"\x1b[0n");
        }
        if find(b"\x1b[c") || find(b"\x1b[0c") {
            self.write_raw(b"\x1b[?1;2c"); // VT100 mit erweiterten Optionen
        }
    }

    fn write_raw(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// Passwort (für sudo, su …) eintippen und mit Enter abschicken.
    pub fn send_password(&mut self, password: &str) {
        self.send(format!("{password}\r").as_bytes());
        self.tail.clear();
    }

    /// Passwort für diese Sitzung merken (z. B. nach Eingabe im Dialog)
    pub fn set_password(&mut self, password: String) {
        self.password = Some(password);
        self.password_sent = true;
    }

    pub fn send(&mut self, bytes: &[u8]) {
        // jede Eingabe beantwortet bzw. verwirft eine angezeigte Passwortabfrage
        self.password_prompt = false;
        if self.state == State::Connected {
            let _ = self.writer.write_all(bytes);
            let _ = self.writer.flush();
        }
        // Eingabe -> zurück ans Ende der Ausgabe springen
        if self.parser.screen().scrollback() > 0 {
            self.parser.screen_mut().set_scrollback(0);
        }
    }

    /// Eingabe des Benutzers: senden und bei gleichzeitiger Eingabe für die anderen Sitzungen merken.
    pub fn input(&mut self, bytes: &[u8]) {
        self.send(bytes);
        if self.mirror {
            self.outbox.extend_from_slice(bytes);
        }
    }

    pub fn paste(&mut self, text: &str) {
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        if self.parser.screen().bracketed_paste() {
            self.input(format!("\x1b[200~{text}\x1b[201~").as_bytes());
        } else {
            self.input(text.as_bytes());
        }
    }

    pub fn paste_clipboard(&mut self) {
        if let Ok(text) = arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
            self.paste(&text);
        }
    }

    pub fn screen_text(&self) -> String {
        self.parser.screen().contents()
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        if (rows, cols) == self.size || rows < 2 || cols < 10 {
            return;
        }
        self.size = (rows, cols);
        self.parser.screen_mut().set_size(NonZeroU16::new(rows).unwrap(), NonZeroU16::new(cols).unwrap());
        let _ = self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        self.selection = None;
    }

    pub fn close(&mut self) {
        if self.state == State::Connected {
            let _ = self.child.kill();
        }
        self.stop_log();
    }

    // ------------------------------------------------------------------ Darstellung

    /// Zeichnet das Terminal in `rect` und verarbeitet Eingaben. Gibt zurück, ob es den Fokus hat.
    pub fn ui(&mut self, ui: &mut egui::Ui, rect: Rect, font_size: f32, grab_focus: bool) -> bool {
        let font = FontId::monospace(font_size);
        let (cw, ch) = ui.ctx().fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
        let cols = ((rect.width() - 2.0 * PAD) / cw).floor().max(10.0) as u16;
        let rows = ((rect.height() - 2.0 * PAD) / ch).floor().max(2.0) as u16;
        self.resize(rows, cols);

        let id = egui::Id::unique(("ssh_terminal", self.id));
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        if grab_focus || resp.clicked() || resp.drag_started() || resp.secondary_clicked() {
            resp.request_focus();
        }
        let focused = resp.has_focus();
        if focused {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    id,
                    egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true },
                )
            });
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }

        let origin = rect.min + Vec2::splat(PAD);
        let cell_at = |p: Pos2| CellPos {
            row: (((p.y - origin.y) / ch).floor().max(0.0) as u16).min(rows - 1),
            col: (((p.x - origin.x) / cw).floor().max(0.0) as u16).min(cols - 1),
        };

        // --- Maus: Auswahl (kopiert beim Loslassen, wie PuTTY), Rechtsklick fügt ein
        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                let c = cell_at(p);
                self.selection = Some((c, c));
            }
        }
        if resp.dragged() {
            if let (Some(p), Some((a, _))) = (resp.interact_pointer_pos(), self.selection) {
                self.selection = Some((a, cell_at(p)));
            }
        }
        if resp.drag_stopped() {
            if let Some(text) = self.selected_text() {
                if !text.is_empty() {
                    ui.ctx().copy_text(text);
                }
            }
        }
        if resp.clicked() {
            self.selection = None;
        }
        if resp.secondary_clicked() {
            self.paste_clipboard();
        }

        // --- Mausrad: Verlauf (bzw. Pfeiltasten in Vollbild-Programmen wie less/vim)
        if resp.hovered() {
            // Mausrad in Zeilen umrechnen (positiv = nach oben in den Verlauf)
            let dy: f32 = ui.input(|i| {
                i.events
                    .iter()
                    .map(|e| match e {
                        egui::Event::MouseWheel { unit, delta, .. } => match unit {
                            egui::MouseWheelUnit::Line => delta.y * 3.0,
                            egui::MouseWheelUnit::Point => delta.y / ch,
                            egui::MouseWheelUnit::Page => delta.y * rows as f32,
                        },
                        _ => 0.0,
                    })
                    .sum()
            });
            if dy.abs() >= 0.5 {
                let lines = dy.abs().round().max(1.0) as usize;
                if self.parser.screen().alternate_screen() {
                    let seq: &[u8] = if dy > 0.0 { b"\x1b[A" } else { b"\x1b[B" };
                    for _ in 0..lines {
                        self.send(seq);
                    }
                } else {
                    let cur = self.parser.screen().scrollback();
                    let next = if dy > 0.0 { cur + lines } else { cur.saturating_sub(lines) };
                    self.parser.screen_mut().set_scrollback(next);
                }
            }
        }

        // --- Tastatur
        let mut open_search = false;
        if focused {
            let events = ui.input(|i| i.events.clone());
            let app_cursor = self.parser.screen().application_cursor();
            for ev in events {
                match ev {
                    // Strg+Umschalt+F: Suche im Verlauf (Strg+F bleibt für die Shell)
                    egui::Event::Key { key: egui::Key::F, pressed: true, modifiers, .. }
                        if modifiers.ctrl && modifiers.shift =>
                    {
                        open_search = true;
                    }
                    // Strg+Umschalt+P: Passwort senden (sudo …)
                    egui::Event::Key { key: egui::Key::P, pressed: true, modifiers, .. }
                        if modifiers.ctrl && modifiers.shift =>
                    {
                        self.wants_password = true;
                    }
                    egui::Event::Text(t) => self.input(t.as_bytes()),
                    egui::Event::Paste(t) => self.paste(&t),
                    egui::Event::Copy => match self.selected_text() {
                        Some(t) if !t.is_empty() => {
                            ui.ctx().copy_text(t);
                            self.selection = None;
                        }
                        _ => self.input(b"\x03"),
                    },
                    egui::Event::Cut => self.input(b"\x18"),
                    egui::Event::Key { key, pressed: true, modifiers, .. } => {
                        if let Some(seq) = key_sequence(key, modifiers, app_cursor) {
                            self.selection = None;
                            self.input(&seq);
                        }
                    }
                    _ => {}
                }
            }
        }

        if open_search {
            self.open_search();
        }
        if self.search.as_ref().is_some_and(|s| !s.hits.is_empty()) {
            let sc = self.parser.screen_mut();
            let cur = sc.scrollback();
            sc.set_scrollback(usize::MAX);
            self.history_len = sc.scrollback();
            sc.set_scrollback(cur);
        }
        self.paint(ui, rect, &font, cw, ch, focused);
        if self.mirror {
            // gleichzeitige Eingabe: deutlich sichtbarer Rahmen
            let orange = Color32::from_rgb(235, 135, 30);
            ui.painter().rect_stroke(rect.shrink(1.0), 0.0, Stroke::new(2.0, orange), egui::StrokeKind::Inside);
            let g = ui.painter().layout_no_wrap("📡 Gleichzeitige Eingabe".into(), FontId::proportional(12.0), Color32::WHITE);
            let bg = Rect::from_min_size(Pos2::new(rect.left() + 6.0, rect.bottom() - g.size().y - 10.0), g.size() + Vec2::new(10.0, 4.0));
            ui.painter().rect_filled(bg, 4.0, orange);
            ui.painter().galley(bg.min + Vec2::new(5.0, 2.0), g, Color32::WHITE);
        }
        self.search_bar(ui, rect);

        // Passwortabfrage erkannt → Schaltfläche unten rechts anbieten
        if self.password_prompt && self.state == State::Connected {
            let label = if self.password.is_some() {
                "🔑 Passwort senden  (Strg+Umschalt+P)"
            } else {
                "🔑 Passwort eingeben…  (Strg+Umschalt+P)"
            };
            let size = Vec2::new(300.0, 28.0);
            let r = Rect::from_min_size(rect.max - size - Vec2::new(14.0, 12.0), size);
            let b = ui.put(r, egui::Button::new(RichText::new(label).strong()).fill(ui.visuals().selection.bg_fill));
            if b.clicked() {
                self.wants_password = true;
            }
        }
        focused
    }

    fn selection_range(&self) -> Option<(CellPos, CellPos)> {
        let (a, b) = self.selection?;
        if a == b {
            return None;
        }
        Some(if (a.row, a.col) <= (b.row, b.col) { (a, b) } else { (b, a) })
    }

    fn selected_text(&self) -> Option<String> {
        let (s, e) = self.selection_range()?;
        let text = self.parser.screen().contents_between(s.row, s.col, e.row, e.col + 1);
        Some(text.lines().map(str::trim_end).collect::<Vec<_>>().join("\r\n"))
    }

    fn paint(&self, ui: &egui::Ui, rect: Rect, font: &FontId, cw: f32, ch: f32, focused: bool) {
        let painter = ui.painter_at(rect);
        let dark = ui.visuals().dark_mode;
        let default_bg = ui.visuals().extreme_bg_color;
        let default_fg = ui.visuals().text_color();
        painter.rect_filled(rect, 0.0, default_bg);

        let screen = self.parser.screen();
        let (rows, cols) = (self.size.0, self.size.1);
        let origin = rect.min + Vec2::splat(PAD);
        let sel = self.selection_range();
        let selected = |r: u16, c: u16| {
            sel.is_some_and(|(s, e)| (r, c) >= (s.row, s.col) && (r, c) <= (e.row, e.col))
        };

        for r in 0..rows {
            let y = origin.y + r as f32 * ch;
            let mut job = egui::text::LayoutJob::default();
            let mut c = 0u16;
            while c < cols {
                let Some(cell) = screen.cell(r, c) else { break };
                if cell.is_wide_continuation() {
                    c += 1;
                    continue;
                }
                let mut fg = term_color(cell.fgcolor(), default_fg, dark, true, cell.bold());
                let mut bg = term_color(cell.bgcolor(), default_bg, dark, false, false);
                if cell.inverse() {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if selected(r, c) {
                    bg = ui.visuals().selection.bg_fill;
                    fg = ui.visuals().selection.stroke.color;
                }
                if cell.dim() {
                    fg = fg.gamma_multiply(0.6);
                }
                let width = if cell.is_wide() { 2.0 } else { 1.0 };
                if bg != default_bg {
                    painter.rect_filled(
                        Rect::from_min_size(Pos2::new(origin.x + c as f32 * cw, y), Vec2::new(cw * width, ch)),
                        0.0,
                        bg,
                    );
                }
                let text = if cell.has_contents() { cell.contents() } else { " " };
                job.append(
                    text,
                    0.0,
                    egui::TextFormat {
                        font_id: font.clone(),
                        color: fg,
                        italics: cell.italic(),
                        underline: if cell.underline() { Stroke::new(1.0, fg) } else { Stroke::NONE },
                        ..Default::default()
                    },
                );
                c += if cell.is_wide() { 2 } else { 1 };
            }
            let galley = painter.layout_job(job);
            painter.galley(Pos2::new(origin.x, y), galley, default_fg);
        }

        // Suchtreffer hervorheben
        if let Some(s) = &self.search {
            let top = self.history_len.saturating_sub(screen.scrollback());
            for (i, &(line, col, len)) in s.hits.iter().enumerate() {
                if line < top || line >= top + rows as usize {
                    continue;
                }
                let r = Rect::from_min_size(
                    Pos2::new(origin.x + col as f32 * cw, origin.y + (line - top) as f32 * ch),
                    Vec2::new(len as f32 * cw, ch),
                );
                let color = if Some(i) == s.current {
                    Color32::from_rgba_unmultiplied(255, 140, 0, 120)
                } else {
                    Color32::from_rgba_unmultiplied(255, 220, 0, 60)
                };
                painter.rect_filled(r, 2.0, color);
            }
        }

        // Cursor
        if !screen.hide_cursor() && screen.scrollback() == 0 && self.state == State::Connected {
            let (cr, cc) = screen.cursor_position();
            let r = Rect::from_min_size(Pos2::new(origin.x + cc as f32 * cw, origin.y + cr as f32 * ch), Vec2::new(cw, ch));
            if focused {
                painter.rect_filled(r, 0.0, default_fg.gamma_multiply(0.55));
            } else {
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, default_fg.gamma_multiply(0.7)), egui::StrokeKind::Inside);
            }
        }

        // Hinweis beim Blättern im Verlauf
        if screen.scrollback() > 0 {
            let text = format!("↑ Verlauf: {} Zeilen zurück – tippen oder Mausrad nach unten", screen.scrollback());
            let pos = Pos2::new(rect.right() - 8.0, rect.top() + 6.0);
            let g = painter.layout_no_wrap(text, FontId::proportional(12.0), ui.visuals().strong_text_color());
            let bg = Rect::from_min_size(pos - Vec2::new(g.size().x + 8.0, 2.0), g.size() + Vec2::new(12.0, 4.0));
            painter.rect_filled(bg, 4.0, ui.visuals().window_fill.gamma_multiply(0.9));
            painter.galley(bg.min + Vec2::new(6.0, 2.0), g, Color32::PLACEHOLDER);
        }
    }
}

impl Drop for SshSession {
    fn drop(&mut self) {
        self.close();
    }
}

/// Erkennt Passwortabfragen wie "[sudo] password for jdoe:", "[sudo] Passwort für jdoe:", "Password:",
/// "Kennwort:" oder "Enter passphrase for key …:" am Ende der Ausgabe.
fn is_password_prompt(text: &str) -> bool {
    // letzte Zeile – nach einem Zeilenumbruch (Eingabe abgeschickt) ist sie leer
    let last = text.rsplit(['\n', '\r']).next().unwrap_or("");
    let last = last.trim_end().to_lowercase();
    last.ends_with(':')
        && last.len() < 120
        && ["password", "passwort", "kennwort", "passphrase", "mot de passe"].iter().any(|w| last.contains(w))
}

/// Alle Zeilen eines Terminals (Verlauf + Bildschirm), älteste zuerst.
fn all_lines(parser: &mut vt100::Parser, rows: u16, cols: u16) -> Vec<String> {
    let screen = parser.screen_mut();
    let saved = screen.scrollback();
    screen.set_scrollback(usize::MAX);
    let max = screen.scrollback();
    let mut lines: Vec<String> = Vec::with_capacity(max + rows as usize);
    // Fensterweise vom ältesten Verlauf bis zum aktuellen Bildschirm
    let mut off = max;
    loop {
        screen.set_scrollback(off);
        let start = max - off;
        for (i, l) in screen.rows(0, cols).enumerate() {
            if start + i == lines.len() {
                lines.push(l);
            }
        }
        if off == 0 {
            break;
        }
        off = off.saturating_sub(rows as usize);
    }
    screen.set_scrollback(saved);
    lines
}

/// Suchtreffer (Zeile, Spalte, Länge), ohne Groß-/Kleinschreibung
fn find_hits(lines: &[String], query: &str) -> Vec<(usize, usize, usize)> {
    let query = query.to_lowercase();
    let mut hits = Vec::new();
    if query.is_empty() {
        return hits;
    }
    let qlen = query.chars().count();
    for (n, line) in lines.iter().enumerate() {
        let low = line.to_lowercase();
        let mut from = 0;
        while let Some(pos) = low[from..].find(&query) {
            let byte = from + pos;
            hits.push((n, low[..byte].chars().count(), qlen));
            from = byte + query.len();
        }
    }
    hits
}

pub fn log_dir() -> PathBuf {
    crate::model::Store::data_dir().join("logs")
}

/// Lokale Zeit als "2026-11-16_09-30-05"
pub fn local_timestamp() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}-{:02}-{:02}_{:02}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// Entfernt ANSI-Escape-Sequenzen (für die Prompt-Erkennung).
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() || c == '~' {
                        break;
                    }
                }
            } else if chars.peek() == Some(&']') {
                for c in chars.by_ref() {
                    if c == '\x07' {
                        break;
                    }
                }
            } else {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Übersetzt eine Taste in die Byte-Folge eines xterm-kompatiblen Terminals.
fn key_sequence(key: egui::Key, m: egui::Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    use egui::Key::*;
    let csi = |s: &str| Some(format!("\x1b[{s}").into_bytes());
    // Modifikator-Parameter für Pfeiltasten usw. (xterm): 1 + Shift(1) + Alt(2) + Ctrl(4)
    let modp = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.ctrl as u8;
    let arrow = |c: char| {
        if modp > 1 {
            csi(&format!("1;{modp}{c}"))
        } else if app_cursor {
            Some(format!("\x1bO{c}").into_bytes())
        } else {
            csi(&c.to_string())
        }
    };
    let tilde = |n: u8| if modp > 1 { csi(&format!("{n};{modp}~")) } else { csi(&format!("{n}~")) };

    // Strg+Buchstabe -> Steuerzeichen (nicht bei AltGr = Strg+Alt)
    if m.ctrl && !m.alt {
        let name = key.name();
        if name.len() == 1 {
            let b = name.as_bytes()[0].to_ascii_uppercase();
            if b.is_ascii_uppercase() {
                return Some(vec![b - b'A' + 1]);
            }
        }
        match key {
            Space => return Some(vec![0]),
            OpenBracket => return Some(vec![0x1b]),
            Backslash => return Some(vec![0x1c]),
            CloseBracket => return Some(vec![0x1d]),
            _ => {}
        }
    }
    // Alt+Buchstabe -> ESC + Buchstabe (Meta)
    if m.alt && !m.ctrl {
        let name = key.name();
        if name.len() == 1 && name.as_bytes()[0].is_ascii_alphanumeric() {
            let ch = if m.shift { name.to_ascii_uppercase() } else { name.to_ascii_lowercase() };
            return Some(format!("\x1b{ch}").into_bytes());
        }
    }
    match key {
        Enter => Some(b"\r".to_vec()),
        Backspace => Some(if m.ctrl { vec![0x08] } else { vec![0x7f] }),
        Tab => Some(if m.shift { b"\x1b[Z".to_vec() } else { b"\t".to_vec() }),
        Escape => Some(b"\x1b".to_vec()),
        ArrowUp => arrow('A'),
        ArrowDown => arrow('B'),
        ArrowRight => arrow('C'),
        ArrowLeft => arrow('D'),
        Home => arrow('H'),
        End => arrow('F'),
        Insert => tilde(2),
        Delete => tilde(3),
        PageUp => tilde(5),
        PageDown => tilde(6),
        F1 => Some(b"\x1bOP".to_vec()),
        F2 => Some(b"\x1bOQ".to_vec()),
        F3 => Some(b"\x1bOR".to_vec()),
        F4 => Some(b"\x1bOS".to_vec()),
        F5 => tilde(15),
        F6 => tilde(17),
        F7 => tilde(18),
        F8 => tilde(19),
        F9 => tilde(20),
        F10 => tilde(21),
        F11 => tilde(23),
        F12 => tilde(24),
        _ => None,
    }
}

/// Terminalfarben (xterm-Palette, an Hell/Dunkel angepasst)
fn term_color(c: vt100::Color, default: Color32, dark: bool, fg: bool, bold: bool) -> Color32 {
    const DARK: [(u8, u8, u8); 16] = [
        (0, 0, 0), (205, 49, 49), (13, 188, 121), (229, 229, 16), (36, 114, 200), (188, 63, 188), (17, 168, 205), (229, 229, 229),
        (102, 102, 102), (241, 76, 76), (35, 209, 139), (245, 245, 67), (59, 142, 234), (214, 112, 214), (41, 184, 219), (255, 255, 255),
    ];
    const LIGHT: [(u8, u8, u8); 16] = [
        (0, 0, 0), (205, 49, 49), (0, 140, 0), (150, 130, 0), (4, 81, 165), (188, 5, 188), (5, 130, 165), (85, 85, 85),
        (102, 102, 102), (205, 49, 49), (20, 160, 20), (160, 150, 0), (4, 81, 165), (188, 5, 188), (5, 140, 175), (60, 60, 60),
    ];
    match c {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
        vt100::Color::Idx(mut i) => {
            if fg && bold && i < 8 {
                i += 8;
            }
            if i < 16 {
                let (r, g, b) = if dark { DARK[i as usize] } else { LIGHT[i as usize] };
                Color32::from_rgb(r, g, b)
            } else if i < 232 {
                let i = i - 16;
                let lv = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                Color32::from_rgb(lv(i / 36), lv((i / 6) % 6), lv(i % 6))
            } else {
                let v = 8 + (i - 232) * 10;
                Color32::from_rgb(v, v, v)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args() {
        let c = Connection {
            host: "srv".into(),
            port: 2222,
            username: "root".into(),
            ssh_key_file: r"C:\keys\id ed".into(),
            ssh_extra_args: r#"-J jump -o "ProxyCommand=x y""#.into(),
            ..Default::default()
        };
        assert_eq!(
            ssh_args(&c),
            vec!["-p", "2222", "-o", "ServerAliveInterval=30", "-i", r"C:\keys\id ed", "-J", "jump", "-o", "ProxyCommand=x y", "-l", "root", "srv"]
        );
    }

    #[test]
    fn args_with_forwards() {
        use crate::model::{Forward, ForwardKind};
        let c = Connection {
            host: "srv".into(),
            port: 22,
            ssh_agent_forward: true,
            forwards: vec![
                Forward { kind: ForwardKind::Local, listen_port: 13389, target_host: "intern".into(), target_port: 3389 },
                Forward { kind: ForwardKind::Local, listen_port: 0, ..Default::default() }, // unvollständig → weglassen
            ],
            ..Default::default()
        };
        assert_eq!(
            ssh_args(&c),
            vec!["-p", "22", "-o", "ServerAliveInterval=30", "-A", "-L", "127.0.0.1:13389:intern:3389", "srv"]
        );
    }

    #[test]
    fn install_key() {
        let cmd = install_key_command("ssh-ed25519 AAAAC3Nza jdoe@pc\n").unwrap();
        assert!(cmd.contains("'ssh-ed25519 AAAAC3Nza jdoe@pc'") && cmd.ends_with('\r'));
        assert!(install_key_command("-----BEGIN OPENSSH PRIVATE KEY-----").is_err());
        assert!(install_key_command("ssh-rsa AAA it's").is_err());
    }

    #[test]
    fn search_in_history() {
        // 5 Zeilen Bildschirm, 40 Zeilen Ausgabe → 35 im Verlauf
        let mut p = vt100::Parser::new(NonZeroU16::new(5).unwrap(), NonZeroU16::new(40).unwrap(), 1000);
        for i in 0..40 {
            p.process(format!("Zeile {i}{}\r\n", if i == 7 { " ERROR hier" } else { "" }).as_bytes());
        }
        p.screen_mut().set_scrollback(3);
        let lines = all_lines(&mut p, 5, 40);
        assert_eq!(p.screen().scrollback(), 3, "Scrollposition bleibt erhalten");
        assert_eq!(lines[0], "Zeile 0");
        assert_eq!(lines[39], "Zeile 39");
        assert_eq!(find_hits(&lines, "error"), vec![(7, 8, 5)]);
        assert_eq!(find_hits(&lines, "zeile 3").len(), 11); // 3, 30–39
    }

    #[test]
    fn sudo_prompts() {
        assert!(is_password_prompt("$ sudo apt update\r\n[sudo] password for jdoe: "));
        assert!(is_password_prompt("[sudo] Passwort für jdoe:"));
        assert!(is_password_prompt("Password:"));
        assert!(is_password_prompt("Enter passphrase for key '/home/jdoe/.ssh/id_ed25519': "));
        assert!(!is_password_prompt("jdoe@srv:~$ "));
        assert!(!is_password_prompt("Your password expires in 5 days.\r\ntl@srv:~$ "));
        // nach dem Abschicken (Zeilenumbruch) keine Abfrage mehr
        assert!(!is_password_prompt("[sudo] password for jdoe: \r\n"));
    }

    #[test]
    fn prompt_detection() {
        assert!(strip_ansi("\x1b[0mroot@srv's password: ").to_lowercase().trim_end().ends_with("password:"));
    }

    #[test]
    fn keys() {
        let ctrl = egui::Modifiers { ctrl: true, ..Default::default() };
        assert_eq!(key_sequence(egui::Key::D, ctrl, false), Some(vec![4]));
        assert_eq!(key_sequence(egui::Key::ArrowUp, Default::default(), true), Some(b"\x1bOA".to_vec()));
        assert_eq!(key_sequence(egui::Key::ArrowUp, Default::default(), false), Some(b"\x1b[A".to_vec()));
        // AltGr (Strg+Alt) darf kein Steuerzeichen erzeugen
        let altgr = egui::Modifiers { ctrl: true, alt: true, ..Default::default() };
        assert_eq!(key_sequence(egui::Key::Q, altgr, false), None);
    }
}
