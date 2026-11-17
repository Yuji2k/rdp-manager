//! SSH-Tunnel für „Über SSH verbinden“: `ssh.exe -N -L 127.0.0.1:<freier Port>:<ziel>:<port>`
//! läuft unsichtbar in einer Pseudo-Konsole (für die Passwortabfrage). Sobald der lokale Port
//! erreichbar ist, wird die eigentliche Verbindung (RDP, VNC, Web) über 127.0.0.1 geöffnet.

use crate::model::Connection;
use eframe::egui;
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum TunState {
    Starting,
    NeedsPassword,
    Ready,
    Failed(String),
}

pub struct Tunnel {
    pub label: String,
    /// Sprunghost (Anzeige)
    pub via: String,
    pub local_port: u16,
    pub state: TunState,
    pub password_input: String,
    pub started: Instant,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    rx: Receiver<Vec<u8>>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    password: Option<String>,
    password_sent: bool,
    tail: String,
    last_probe: Instant,
}

/// Freien lokalen Port suchen
fn free_port() -> Result<u16, String> {
    let l = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("Kein freier lokaler Port: {e}"))?;
    l.local_addr().map(|a| a.port()).map_err(|e| e.to_string())
}

/// ssh-Argumente für den Tunnel: Verbindungsargumente des Sprunghosts + -N -L …
pub fn tunnel_args(jump: &Connection, local_port: u16, target_host: &str, target_port: u16) -> Vec<String> {
    // Portweiterleitungen des Sprunghosts selbst hier nicht mitnehmen (Port wäre doppelt belegt)
    let jump = Connection { forwards: Vec::new(), ..jump.clone() };
    let mut a = crate::ssh::ssh_args(&jump);
    let host = a.pop().unwrap_or_default();
    let t = target_host.trim();
    let t = if t.contains(':') { format!("[{t}]") } else { t.to_string() };
    a.extend([
        "-N".to_string(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        "-L".into(),
        format!("127.0.0.1:{local_port}:{t}:{target_port}"),
    ]);
    a.push(host);
    a
}

impl Tunnel {
    pub fn start(
        jump: &Connection,
        target_host: &str,
        target_port: u16,
        password: Option<String>,
        label: String,
        ctx: &egui::Context,
    ) -> Result<Self, String> {
        let exe = r"C:\Windows\System32\OpenSSH\ssh.exe";
        if !std::path::Path::new(exe).exists() {
            return Err("OpenSSH-Client nicht gefunden (für den SSH-Tunnel nötig).".into());
        }
        let local_port = free_port()?;
        let pair = native_pty_system()
            .openpty(PtySize { rows: 10, cols: 200, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| format!("Pseudo-Konsole konnte nicht erstellt werden: {e}"))?;
        let mut cmd = CommandBuilder::new(exe);
        cmd.args(tunnel_args(jump, local_port, target_host, target_port));
        let child = pair.slave.spawn_command(cmd).map_err(|e| format!("ssh.exe konnte nicht gestartet werden: {e}"))?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
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
            label,
            via: jump.display_name().to_string(),
            local_port,
            state: TunState::Starting,
            password_input: String::new(),
            started: Instant::now(),
            child,
            writer,
            rx,
            _master: pair.master,
            password,
            password_sent: false,
            tail: String::new(),
            last_probe: Instant::now(),
        })
    }

    fn write(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn send_password(&mut self) {
        let pw = std::mem::take(&mut self.password_input);
        self.write(format!("{pw}\r").as_bytes());
        self.state = TunState::Starting;
        self.tail.clear();
    }

    pub fn is_alive(&self) -> bool {
        !matches!(self.state, TunState::Failed(_))
    }

    pub fn close(&mut self) {
        let _ = self.child.kill();
    }

    pub fn update(&mut self) {
        while let Ok(bytes) = self.rx.try_recv() {
            if bytes.windows(4).any(|w| w == b"\x1b[6n") {
                self.write(b"\x1b[1;1R");
            }
            self.tail.push_str(&crate::ssh::strip_ansi(&String::from_utf8_lossy(&bytes)));
            if self.tail.len() > 2000 {
                let cut = self.tail.len() - 2000;
                let cut = (cut..self.tail.len()).find(|i| self.tail.is_char_boundary(*i)).unwrap_or(0);
                self.tail.drain(..cut);
            }
            let low = self.tail.to_lowercase();
            let end = low.trim_end();
            let last = end.rsplit('\n').next().unwrap_or("");
            if end.ends_with("password:") || (last.contains("passphrase") && end.ends_with(':')) {
                match self.password.clone().filter(|_| !self.password_sent) {
                    Some(pw) => {
                        self.write(format!("{pw}\r").as_bytes());
                        self.password_sent = true;
                        self.tail.clear();
                    }
                    None => self.state = TunState::NeedsPassword,
                }
            } else if low.contains("(yes/no") {
                let _ = self.child.kill();
                self.state = TunState::Failed(format!(
                    "Hostschlüssel von {} ist unbekannt. Bitte zuerst einmal per SSH dorthin verbinden und bestätigen.",
                    self.via
                ));
            }
        }
        if self.is_alive()
            && let Ok(Some(status)) = self.child.try_wait()
        {
            let last = self.tail.lines().map(str::trim).rev().find(|l| !l.is_empty()).unwrap_or("").to_string();
            self.state = TunState::Failed(if last.is_empty() {
                format!("SSH-Tunnel über {} beendet (Code {}).", self.via, status.exit_code())
            } else {
                format!("SSH-Tunnel über {}: {}", self.via, last.chars().take(200).collect::<String>())
            });
            return;
        }
        // Bereit, sobald der lokale Port angenommen wird (ssh lauscht erst nach der Anmeldung)
        if self.state == TunState::Starting && self.last_probe.elapsed() > Duration::from_millis(250) {
            self.last_probe = Instant::now();
            let addr = SocketAddr::from(([127, 0, 0, 1], self.local_port));
            if TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_ok() {
                self.state = TunState::Ready;
            }
        }
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Forward, ForwardKind};

    #[test]
    fn args() {
        let jump = Connection {
            host: "jump.example.com".into(),
            port: 2222,
            username: "jdoe".into(),
            forwards: vec![Forward { kind: ForwardKind::Dynamic, listen_port: 1080, ..Default::default() }],
            ..Default::default()
        };
        assert_eq!(
            tunnel_args(&jump, 50000, "10.0.0.5", 3389),
            vec![
                "-p", "2222", "-o", "ServerAliveInterval=30", "-l", "jdoe", "-N", "-o", "ExitOnForwardFailure=yes", "-L",
                "127.0.0.1:50000:10.0.0.5:3389", "jump.example.com"
            ]
        );
        assert!(free_port().unwrap() > 0);
    }
}
