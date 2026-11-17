//! Dateiübertragung für SSH-Verbindungen mit Windows-OpenSSH `scp.exe`.
//!
//! scp läuft – wie ssh – in einer eigenen Pseudo-Konsole: So erscheint sein Fortschrittsbalken
//! und eine Passwortabfrage kann automatisch (gespeichertes Passwort) oder über das
//! Übertragungsfenster beantwortet werden.

use crate::model::Connection;
use eframe::egui;
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

const SCP_EXE: &str = r"C:\Windows\System32\OpenSSH\scp.exe";

#[derive(Clone, Debug, PartialEq)]
pub enum TState {
    Running,
    /// scp fragt nach dem Passwort und es ist keins gespeichert
    NeedsPassword,
    Done,
    Failed(String),
    Cancelled,
}

pub struct Transfer {
    pub id: u64,
    pub label: String,
    pub state: TState,
    /// 0.0 – 1.0 (aktuelle Datei)
    pub progress: f32,
    /// aktuell übertragene Datei (aus der scp-Ausgabe)
    pub current: String,
    /// Eingabefeld, falls nach dem Passwort gefragt wird
    pub password_input: String,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    rx: Receiver<Vec<u8>>,
    password: Option<String>,
    password_sent: bool,
    /// letzte Ausgabe (ohne ANSI) für Prompt-Erkennung und Fehlermeldungen
    tail: String,
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

/// "user@host:" bzw. "host:" (scp-Zielangabe)
fn remote_prefix(conn: &Connection) -> String {
    let host = conn.host.trim();
    // IPv6-Adressen in Klammern
    let host = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
    let user = conn.username.trim();
    if user.is_empty() { format!("{host}:") } else { format!("{user}@{host}:") }
}

/// Argumente für scp.exe. Von den zusätzlichen ssh-Argumenten werden nur die übernommen,
/// die scp auch versteht (-J, -o, -F, -i).
pub fn scp_args(conn: &Connection, upload: bool, local: &[PathBuf], remote: &str) -> Vec<String> {
    let mut a = vec!["-r".to_string()];
    a.extend(common_args(conn));
    let remote = format!("{}{}", remote_prefix(conn), remote.trim());
    if upload {
        a.extend(local.iter().map(|p| p.display().to_string()));
        a.push(remote);
    } else {
        a.push(remote);
        a.push(local.first().map(|p| p.display().to_string()).unwrap_or_else(|| ".".into()));
    }
    a
}

/// Verbindungsargumente für scp.exe und sftp.exe (Port, Schlüssel, -J/-o/-F/-i aus den Zusatzargumenten)
pub fn common_args(conn: &Connection) -> Vec<String> {
    let mut a = vec!["-P".to_string(), conn.port.to_string(), "-o".into(), "ServerAliveInterval=30".into()];
    if !conn.ssh_key_file.trim().is_empty() {
        a.push("-i".into());
        a.push(conn.ssh_key_file.trim().to_string());
    }
    let extra = crate::ssh::split_args(&conn.ssh_extra_args);
    let mut it = extra.into_iter();
    while let Some(arg) = it.next() {
        if matches!(arg.as_str(), "-J" | "-o" | "-F" | "-i") {
            if let Some(v) = it.next() {
                a.push(arg);
                a.push(v);
            }
        }
    }
    a
}

/// "user@host" bzw. "host" für sftp
pub fn sftp_target(conn: &Connection) -> String {
    remote_prefix(conn).trim_end_matches(':').to_string()
}

impl Transfer {
    /// `local`: hochzuladende Dateien bzw. (beim Download) der Zielordner.
    /// `remote`: Zielordner auf dem Server bzw. herunterzuladender Pfad.
    pub fn start(
        id: u64,
        conn: &Connection,
        upload: bool,
        local: Vec<PathBuf>,
        remote: &str,
        password: Option<String>,
        ctx: &egui::Context,
    ) -> Result<Self, String> {
        if !std::path::Path::new(SCP_EXE).exists() {
            return Err("scp.exe nicht gefunden (OpenSSH-Client fehlt).".into());
        }
        let label = if upload {
            let names: Vec<String> = local
                .iter()
                .map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
                .collect();
            let shown = if names.len() > 2 { format!("{}, … ({} Dateien)", names[0], names.len()) } else { names.join(", ") };
            let dir = if remote.trim().is_empty() { "~" } else { remote.trim() };
            format!("⬆ {shown} ➡ {}:{dir}", conn.host.trim())
        } else {
            format!("⬇ {}:{} ➡ {}", conn.host.trim(), remote.trim(), local.first().map(|p| p.display().to_string()).unwrap_or_default())
        };

        let pair = native_pty_system()
            .openpty(PtySize { rows: 5, cols: 200, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| format!("Pseudo-Konsole konnte nicht erstellt werden: {e}"))?;
        let mut cmd = CommandBuilder::new(SCP_EXE);
        cmd.args(scp_args(conn, upload, &local, remote));
        let child = pair.slave.spawn_command(cmd).map_err(|e| format!("scp.exe konnte nicht gestartet werden: {e}"))?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
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
            label,
            state: TState::Running,
            progress: 0.0,
            current: String::new(),
            password_input: String::new(),
            child,
            writer,
            rx,
            password,
            password_sent: false,
            tail: String::new(),
            _master: pair.master,
        })
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state, TState::Running | TState::NeedsPassword)
    }

    fn write(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// Passwort aus dem Übertragungsfenster senden
    pub fn send_password(&mut self) {
        let pw = std::mem::take(&mut self.password_input);
        self.write(format!("{pw}\r").as_bytes());
        self.state = TState::Running;
        self.tail.clear();
    }

    pub fn cancel(&mut self) {
        if self.is_active() {
            let _ = self.child.kill();
            self.state = TState::Cancelled;
        }
    }

    pub fn update(&mut self) {
        while let Ok(bytes) = self.rx.try_recv() {
            // ConPTY fragt beim Start die Cursorposition ab und wartet auf eine Antwort
            if bytes.windows(4).any(|w| w == b"\x1b[6n") {
                self.write(b"\x1b[1;1R");
            }
            let text = crate::ssh::strip_ansi(&String::from_utf8_lossy(&bytes));
            self.tail.push_str(&text);
            if self.tail.len() > 2000 {
                let cut = self.tail.len() - 2000;
                let cut = (cut..self.tail.len()).find(|i| self.tail.is_char_boundary(*i)).unwrap_or(0);
                self.tail.drain(..cut);
            }
            self.parse_progress();
            let low = self.tail.to_lowercase();
            let low = low.trim_end();
            let last_line = low.rsplit('\n').next().unwrap_or("");
            if low.ends_with("password:") || (last_line.contains("passphrase") && low.ends_with(':')) {
                match self.password.clone().filter(|_| !self.password_sent) {
                    Some(pw) => {
                        self.write(format!("{pw}\r").as_bytes());
                        self.password_sent = true;
                        self.tail.clear();
                    }
                    None => self.state = TState::NeedsPassword,
                }
            } else if low.contains("(yes/no") {
                // unbekannter Hostschlüssel – hier nicht blind bestätigen
                let _ = self.child.kill();
                self.state = TState::Failed(
                    "Hostschlüssel des Servers ist unbekannt. Bitte zuerst einmal per SSH verbinden und bestätigen.".into(),
                );
            }
        }
        if self.is_active()
            && let Ok(Some(status)) = self.child.try_wait()
        {
            self.state = if status.success() {
                self.progress = 1.0;
                TState::Done
            } else {
                TState::Failed(self.last_error().unwrap_or_else(|| format!("scp wurde mit Code {} beendet.", status.exit_code())))
            };
        }
    }

    /// Fortschritt aus der scp-Anzeige lesen: "datei.iso   45%   12MB   3.0MB/s   00:02 ETA"
    fn parse_progress(&mut self) {
        let Some(line) = self.tail.rsplit(['\r', '\n']).find(|l| l.contains('%')) else { return };
        let words: Vec<&str> = line.split_whitespace().collect();
        if let Some(pos) = words.iter().position(|w| w.ends_with('%') && w[..w.len() - 1].parse::<u32>().is_ok()) {
            let pct: u32 = words[pos][..words[pos].len() - 1].parse().unwrap_or(0);
            self.progress = pct.min(100) as f32 / 100.0;
            if pos > 0 {
                self.current = words[..pos].join(" ");
            }
        }
    }

    /// Letzte aussagekräftige Zeile der Ausgabe (Fehlermeldung von scp/ssh)
    fn last_error(&self) -> Option<String> {
        self.tail
            .lines()
            .map(str::trim)
            .rev()
            .find(|l| !l.is_empty() && !l.contains('%'))
            .map(|l| l.chars().take(200).collect())
    }
}

impl Drop for Transfer {
    fn drop(&mut self) {
        self.cancel();
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
            ssh_extra_args: "-J jump -A -o StrictHostKeyChecking=yes".into(),
            ..Default::default()
        };
        assert_eq!(
            scp_args(&c, true, &[PathBuf::from(r"C:\a b.txt")], "/tmp"),
            vec!["-r", "-P", "2222", "-o", "ServerAliveInterval=30", "-J", "jump", "-o", "StrictHostKeyChecking=yes", r"C:\a b.txt", "root@srv:/tmp"]
        );
        let c = Connection { host: "fe80::1".into(), port: 22, ..Default::default() };
        assert_eq!(scp_args(&c, false, &[PathBuf::from(r"C:\dl")], "~/x").last().unwrap(), r"C:\dl");
        assert_eq!(scp_args(&c, false, &[PathBuf::from(r"C:\dl")], "~/x")[5], "[fe80::1]:~/x");
    }
}
