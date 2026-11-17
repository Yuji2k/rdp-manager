//! Dateibrowser für SSH-Verbindungen: `sftp.exe` (Windows-OpenSSH) läuft in einer
//! Pseudo-Konsole, die Verzeichnisliste wird aus der Ausgabe von `ls -la` gelesen.
//! Übertragungen laufen über die vorhandenen scp-Übertragungen (mit Fortschrittsfenster).

use crate::model::Connection;
use eframe::egui::{self, Color32};
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::mpsc::{Receiver, channel};

const SFTP_EXE: &str = r"C:\Windows\System32\OpenSSH\sftp.exe";
/// Breite der Seitenleiste (logische Punkte)
pub const WIDTH: f32 = 360.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    pub date: String,
    pub perms: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SState {
    Connecting,
    NeedsPassword,
    Ready,
    Failed(String),
}

/// Was der Browser von der Anwendung möchte
pub enum BrowserAction {
    /// Datei/Ordner (Pfad auf dem Server) herunterladen
    Download(String),
    /// Dateien auswählen und in diesen Ordner hochladen
    Upload(String),
    /// Im Terminal in diesen Ordner wechseln
    CdInTerminal(String),
    Close,
}

pub struct SftpBrowser {
    pub cwd: String,
    pub entries: Vec<Entry>,
    pub state: SState,
    pub path_edit: String,
    pub selected: Option<String>,
    pub password_input: String,
    /// Fehlermeldung des letzten Befehls
    pub message: Option<String>,
    pub show_hidden: bool,
    /// Bereich der Seitenleiste im letzten Frame (für Drag & Drop)
    pub rect: Option<egui::Rect>,
    confirm_delete: Option<Entry>,
    rename: Option<(String, String)>,
    new_dir: Option<String>,
    /// noch zu sendende Befehle
    queue: VecDeque<String>,
    /// laufender Befehl
    current: Option<String>,
    /// Ausgabe seit dem letzten Befehl (ohne Steuerzeichen)
    out: String,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    rx: Receiver<Vec<u8>>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    password: Option<String>,
    password_sent: bool,
}

/// Pfad für sftp-Befehle in Anführungszeichen setzen
fn quote(p: &str) -> String {
    format!("\"{}\"", p.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Verbindet Ordner und Name zu einem Pfad auf dem Server
pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") }
}

/// Übergeordneter Ordner ("/var/log" → "/var", "/var" → "/")
pub fn parent(dir: &str) -> String {
    let d = dir.trim_end_matches('/');
    match d.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => d[..i].to_string(),
    }
}

/// Ausgabe der Pseudo-Konsole in Text umwandeln: Cursor-vorwärts (ESC[nC) wird zu Leerzeichen,
/// Positionierungen zu Zeilenumbrüchen, alle anderen Steuersequenzen fallen weg.
pub fn vt_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    let mut param = String::new();
                    for c in chars.by_ref() {
                        if c.is_ascii_alphabetic() || c == '~' || c == '@' {
                            match c {
                                'C' => {
                                    let n = param.parse::<usize>().unwrap_or(1).min(400);
                                    out.extend(std::iter::repeat_n(' ', n));
                                }
                                'H' | 'f' if !out.ends_with('\n') && !out.is_empty() => out.push('\n'),
                                _ => {}
                            }
                            break;
                        }
                        param.push(c);
                    }
                }
                Some(']') => {
                    for c in chars.by_ref() {
                        if c == '\x07' {
                            break;
                        }
                    }
                }
                _ => {
                    chars.next();
                }
            },
            '\r' => {}
            c if c == '\n' || c == '\t' || !c.is_control() => out.push(c),
            _ => {}
        }
    }
    out
}

/// Eine Zeile aus `ls -la`, z. B. "drwxr-xr-x    2 jdoe  jdoe   4096 Nov 16 10:00 mein ordner"
pub fn parse_ls_line(line: &str) -> Option<Entry> {
    let line = line.trim_end();
    let perms = line.split_whitespace().next()?;
    if perms.len() < 10 || !"dl-cbps".contains(perms.chars().next()?) {
        return None;
    }
    // die ersten 8 Felder überspringen, der Rest ist der Name (darf Leerzeichen enthalten)
    let mut rest = line;
    let mut fields = Vec::new();
    for _ in 0..8 {
        rest = rest.trim_start();
        let end = rest.find(char::is_whitespace)?;
        fields.push(&rest[..end]);
        rest = &rest[end..];
    }
    let mut name = rest.trim_start().to_string();
    if name.is_empty() {
        return None;
    }
    let is_link = perms.starts_with('l');
    if is_link && let Some(p) = name.find(" -> ") {
        name.truncate(p);
    }
    if name == "." || name == ".." {
        return None;
    }
    Some(Entry {
        name,
        is_dir: perms.starts_with('d'),
        is_link,
        size: fields[4].parse().unwrap_or(0),
        date: format!("{} {} {}", fields[5], fields[6], fields[7]),
        perms: perms.to_string(),
    })
}

pub fn human_size(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n} B") } else { format!("{v:.1} {}", U[i]) }
}

impl SftpBrowser {
    pub fn start(conn: &Connection, password: Option<String>, ctx: &egui::Context) -> Result<Self, String> {
        if !std::path::Path::new(SFTP_EXE).exists() {
            return Err("sftp.exe nicht gefunden (OpenSSH-Client fehlt).".into());
        }
        let pair = native_pty_system()
            .openpty(PtySize { rows: 50, cols: 400, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| format!("Pseudo-Konsole konnte nicht erstellt werden: {e}"))?;
        let mut cmd = CommandBuilder::new(SFTP_EXE);
        cmd.args(crate::transfer::common_args(conn));
        cmd.arg(crate::transfer::sftp_target(conn));
        let child = pair.slave.spawn_command(cmd).map_err(|e| format!("sftp.exe konnte nicht gestartet werden: {e}"))?;
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
            cwd: String::new(),
            entries: Vec::new(),
            state: SState::Connecting,
            path_edit: String::new(),
            selected: None,
            password_input: String::new(),
            message: None,
            show_hidden: false,
            rect: None,
            confirm_delete: None,
            rename: None,
            new_dir: None,
            queue: VecDeque::new(),
            current: None,
            out: String::new(),
            child,
            writer,
            rx,
            _master: pair.master,
            password,
            password_sent: false,
        })
    }

    fn write(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn is_busy(&self) -> bool {
        self.current.is_some() || !self.queue.is_empty() || self.state == SState::Connecting
    }

    pub fn send_password(&mut self) {
        let pw = std::mem::take(&mut self.password_input);
        self.write(format!("{pw}\r").as_bytes());
        self.state = SState::Connecting;
        self.out.clear();
    }

    /// In einen Ordner wechseln und ihn anzeigen
    pub fn navigate(&mut self, dir: &str) {
        let dir = dir.trim();
        if dir.is_empty() {
            return;
        }
        self.selected = None;
        self.queue.push_back(format!("cd {}", quote(dir)));
        self.refresh();
    }

    pub fn refresh(&mut self) {
        self.queue.push_back("pwd".into());
        self.queue.push_back("ls -la".into());
    }

    fn send_next(&mut self) {
        if self.current.is_some() || self.state != SState::Ready {
            return;
        }
        if let Some(cmd) = self.queue.pop_front() {
            self.out.clear();
            self.write(format!("{cmd}\r").as_bytes());
            self.current = Some(cmd);
        }
    }

    pub fn close(&mut self) {
        let _ = self.child.kill();
    }

    pub fn update(&mut self) {
        while let Ok(bytes) = self.rx.try_recv() {
            // ConPTY fragt beim Start die Cursorposition ab und wartet auf eine Antwort
            if bytes.windows(4).any(|w| w == b"\x1b[6n") {
                self.write(b"\x1b[1;1R");
            }
            self.out.push_str(&vt_text(&String::from_utf8_lossy(&bytes)));
            let low = self.out.to_lowercase();
            let end = low.trim_end();
            if end.ends_with("sftp>") {
                let done = self.current.take();
                let text = std::mem::take(&mut self.out);
                if self.state != SState::Ready {
                    self.state = SState::Ready;
                    if self.queue.is_empty() {
                        self.refresh();
                    }
                }
                if let Some(cmd) = done {
                    self.finish(&cmd, &text);
                }
            } else if self.current.is_none() && self.state == SState::Connecting {
                let last = end.rsplit('\n').next().unwrap_or("");
                if end.ends_with("password:") || (last.contains("passphrase") && end.ends_with(':')) {
                    match self.password.clone().filter(|_| !self.password_sent) {
                        Some(pw) => {
                            self.write(format!("{pw}\r").as_bytes());
                            self.password_sent = true;
                            self.out.clear();
                        }
                        None => self.state = SState::NeedsPassword,
                    }
                } else if low.contains("(yes/no") {
                    let _ = self.child.kill();
                    self.state = SState::Failed(
                        "Hostschlüssel des Servers ist unbekannt. Bitte zuerst einmal per SSH verbinden und bestätigen.".into(),
                    );
                }
            }
        }
        if !matches!(self.state, SState::Failed(_))
            && let Ok(Some(status)) = self.child.try_wait()
        {
            let last = self.out.lines().map(str::trim).rev().find(|l| !l.is_empty()).unwrap_or("").to_string();
            self.state = SState::Failed(if last.is_empty() {
                format!("sftp wurde beendet (Code {}).", status.exit_code())
            } else {
                last.chars().take(200).collect()
            });
        }
        self.send_next();
    }

    /// Ausgabe eines abgeschlossenen Befehls auswerten
    fn finish(&mut self, cmd: &str, text: &str) {
        // erste Zeile = Echo des Befehls, letzte = Eingabeaufforderung
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with("sftp>"))
            .collect();
        if cmd == "pwd" {
            if let Some(dir) = lines.iter().find_map(|l| l.split_once("working directory:").map(|(_, d)| d.trim().to_string())) {
                self.cwd = dir.clone();
                self.path_edit = dir;
            }
        } else if cmd == "ls -la" {
            let mut entries: Vec<Entry> = lines.iter().filter_map(|l| parse_ls_line(l)).collect();
            entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
            self.entries = entries;
        } else {
            // Fehlermeldungen (Befehle ohne Ausgabe sind erfolgreich)
            let errors: Vec<&str> = lines.iter().copied().filter(|l| !cmd.starts_with(l.trim())).collect();
            if !errors.is_empty() {
                self.message = Some(errors.join(" "));
                if cmd.starts_with("cd ") {
                    // Ordner gibt es nicht → Liste des bisherigen Ordners behalten
                    self.queue.retain(|c| c != "ls -la");
                }
            }
        }
    }

    // ------------------------------------------------------------------ Darstellung

    /// Zeichnet die Seitenleiste in `rect`.
    pub fn ui(&mut self, ui: &mut egui::Ui, rect: egui::Rect, host: &str) -> Option<BrowserAction> {
        self.rect = Some(rect);
        let mut action = None;
        ui.painter().rect_filled(rect, 0.0, ui.visuals().panel_fill);
        ui.painter().vline(rect.left(), rect.y_range(), ui.visuals().widgets.noninteractive.bg_stroke);
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(8.0, 6.0))), |ui| {
            ui.horizontal(|ui| {
                ui.strong(format!("📁 Dateien – {host}"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✖").on_hover_text("Dateibrowser schließen").clicked() {
                        action = Some(BrowserAction::Close);
                    }
                    if self.is_busy() {
                        ui.spinner();
                    }
                });
            });
            match self.state.clone() {
                SState::Connecting if self.cwd.is_empty() => {
                    ui.weak("Verbinde per SFTP …");
                    return;
                }
                SState::NeedsPassword => {
                    ui.label("Passwort für SFTP:");
                    ui.horizontal(|ui| {
                        let r = ui.add(egui::TextEdit::singleline(&mut self.password_input).password(true).desired_width(180.0));
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.button("Senden").clicked() || enter {
                            self.send_password();
                        }
                    });
                    return;
                }
                SState::Failed(e) => {
                    ui.colored_label(Color32::from_rgb(220, 70, 70), format!("⛔ {e}"));
                    return;
                }
                _ => {}
            }

            // --- Pfadleiste
            ui.horizontal(|ui| {
                if ui.small_button("⬆").on_hover_text("Übergeordneter Ordner").clicked() {
                    let p = parent(&self.cwd);
                    self.navigate(&p);
                }
                if ui.small_button("🏠").on_hover_text("Home-Verzeichnis").clicked() {
                    self.navigate("~");
                }
                if ui.small_button("🔄").on_hover_text("Neu laden").clicked() {
                    self.refresh();
                }
                let r = ui.add(egui::TextEdit::singleline(&mut self.path_edit).desired_width(f32::INFINITY));
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    let p = self.path_edit.clone();
                    self.navigate(&p);
                }
            });
            ui.horizontal(|ui| {
                if ui.button("⬆ Hochladen…").on_hover_text("Dateien in diesen Ordner hochladen (oder hierher ziehen)").clicked() {
                    action = Some(BrowserAction::Upload(self.cwd.clone()));
                }
                if ui.button("📁+").on_hover_text("Neuer Ordner").clicked() {
                    self.new_dir = Some(String::new());
                }
                if ui.button("⌨ cd").on_hover_text("Im Terminal in diesen Ordner wechseln").clicked() {
                    action = Some(BrowserAction::CdInTerminal(self.cwd.clone()));
                }
                ui.checkbox(&mut self.show_hidden, "versteckte");
            });
            if let Some(m) = self.message.clone() {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(220, 140, 40), format!("⚠ {m}"));
                    if ui.small_button("OK").clicked() {
                        self.message = None;
                    }
                });
            }
            // --- Neuer Ordner / Umbenennen / Löschen bestätigen
            if let Some(name) = &mut self.new_dir {
                let mut done = None;
                ui.horizontal(|ui| {
                    let r = ui.add(egui::TextEdit::singleline(name).hint_text("Name des Ordners").desired_width(170.0));
                    r.request_focus();
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (ui.button("Anlegen").clicked() || enter) && !name.trim().is_empty() {
                        done = Some(Some(name.trim().to_string()));
                    }
                    if ui.button("✖").clicked() {
                        done = Some(None);
                    }
                });
                if let Some(d) = done {
                    self.new_dir = None;
                    if let Some(n) = d {
                        self.queue.push_back(format!("mkdir {}", quote(&n)));
                        self.refresh();
                    }
                }
            }
            if let Some((old, new)) = &mut self.rename {
                let mut done = None;
                ui.horizontal(|ui| {
                    let r = ui.add(egui::TextEdit::singleline(new).desired_width(170.0));
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (ui.button("Umbenennen").clicked() || enter) && !new.trim().is_empty() {
                        done = Some(Some((old.clone(), new.trim().to_string())));
                    }
                    if ui.button("✖").clicked() {
                        done = Some(None);
                    }
                });
                if let Some(d) = done {
                    self.rename = None;
                    if let Some((o, n)) = d.filter(|(o, n)| o != n) {
                        self.queue.push_back(format!("rename {} {}", quote(&o), quote(&n)));
                        self.refresh();
                    }
                }
            }
            if let Some(e) = self.confirm_delete.clone() {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(
                        Color32::from_rgb(220, 70, 70),
                        format!("„{}“ löschen?{}", e.name, if e.is_dir { " (nur leere Ordner)" } else { "" }),
                    );
                    if ui.button("Löschen").clicked() {
                        let cmd = if e.is_dir { "rmdir" } else { "rm" };
                        self.queue.push_back(format!("{cmd} {}", quote(&e.name)));
                        self.refresh();
                        self.confirm_delete = None;
                    }
                    if ui.button("Abbrechen").clicked() {
                        self.confirm_delete = None;
                    }
                });
            }
            ui.separator();

            // --- Liste
            let mut open_dir = None;
            let cwd = self.cwd.clone();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let shown: Vec<Entry> =
                    self.entries.iter().filter(|e| self.show_hidden || !e.name.starts_with('.')).cloned().collect();
                if shown.is_empty() && !self.is_busy() {
                    ui.weak("(leer)");
                }
                for e in shown {
                    let icon = if e.is_dir { "📁" } else if e.is_link { "🔗" } else { "📄" };
                    let sel = self.selected.as_deref() == Some(e.name.as_str());
                    let r = ui
                        .horizontal(|ui| {
                            let r = ui.selectable_label(sel, format!("{icon} {}", e.name));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if !e.is_dir {
                                    ui.weak(human_size(e.size));
                                }
                            });
                            r
                        })
                        .inner;
                    let path = join(&cwd, &e.name);
                    if r.clicked() {
                        self.selected = Some(e.name.clone());
                    }
                    if r.double_clicked() {
                        if e.is_dir || e.is_link {
                            open_dir = Some(path.clone());
                        } else {
                            action = Some(BrowserAction::Download(path.clone()));
                        }
                    }
                    let r = r.on_hover_text(format!(
                        "{}\n{}  {}{}",
                        path,
                        e.perms,
                        e.date,
                        if e.is_dir { String::new() } else { format!("  {}", human_size(e.size)) }
                    ));
                    r.context_menu(|ui| {
                        self.selected = Some(e.name.clone());
                        if (e.is_dir || e.is_link) && ui.button("📂 Öffnen").clicked() {
                            open_dir = Some(path.clone());
                        }
                        if ui.button("⬇ Herunterladen").clicked() {
                            action = Some(BrowserAction::Download(path.clone()));
                        }
                        if ui.button("📋 Pfad kopieren").clicked() {
                            ui.ctx().copy_text(path.clone());
                        }
                        if e.is_dir && ui.button("⌨ Im Terminal öffnen (cd)").clicked() {
                            action = Some(BrowserAction::CdInTerminal(path.clone()));
                        }
                        ui.separator();
                        if ui.button("✏ Umbenennen").clicked() {
                            self.rename = Some((e.name.clone(), e.name.clone()));
                        }
                        if ui.button("🗑 Löschen…").clicked() {
                            self.confirm_delete = Some(e.clone());
                        }
                    });
                }
            });
            if let Some(d) = open_dir {
                self.navigate(&d);
            }
        });
        // Dateien werden über die Seitenleiste gezogen
        if ui.ctx().input(|i| !i.raw.hovered_files.is_empty())
            && ui.ctx().pointer_latest_pos().is_some_and(|p| rect.contains(p))
        {
            ui.painter().rect_stroke(
                rect.shrink(4.0),
                6.0,
                egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("⬆ Ablegen: hochladen nach\n{}", self.cwd),
                egui::FontId::proportional(16.0),
                ui.visuals().strong_text_color(),
            );
        }
        action
    }
}

impl Drop for SftpBrowser {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ls_parsing() {
        let out = vt_text(
            "ls -la\r\n\
             drwxr-xr-x    5 jdoe       jdoe           4096 Nov 16 10:00 .\r\n\
             drwxr-xr-x    3 root     root         4096 Jan  2  2025 ..\r\n\
             -rw-r--r--\x1b[4C1 jdoe       jdoe      1048576 Nov 16 09:12 backup 2026.tar.gz\r\n\
             lrwxrwxrwx    1 jdoe       jdoe             11 Nov 16 09:12 logs -> /var/log/x\r\n\
             drwx------    2 jdoe       jdoe           4096 Nov 15 18:00 .ssh\r\n\
             sftp> ",
        );
        let e: Vec<Entry> = out.lines().filter_map(parse_ls_line).collect();
        assert_eq!(e.len(), 3);
        assert_eq!((e[0].name.as_str(), e[0].size, e[0].is_dir), ("backup 2026.tar.gz", 1_048_576, false));
        assert_eq!((e[1].name.as_str(), e[1].is_link), ("logs", true));
        assert_eq!((e[2].name.as_str(), e[2].is_dir, e[2].date.as_str()), (".ssh", true, "Nov 15 18:00"));
        assert_eq!(human_size(1_048_576), "1.0 MB");
    }

    #[test]
    fn paths() {
        assert_eq!(parent("/var/log/"), "/var");
        assert_eq!(parent("/var"), "/");
        assert_eq!(parent("/"), "/");
        assert_eq!(join("/", "etc"), "/etc");
        assert_eq!(join("/home/jdoe", "a b"), "/home/jdoe/a b");
        assert_eq!(quote(r#"a "b"\c"#), r#""a \"b\"\\c""#);
    }
}
