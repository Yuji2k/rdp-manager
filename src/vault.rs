//! Anbindung externer Passwort-Tresore über deren Kommandozeilenprogramme:
//! KeePassXC (keepassxc-cli), KeePass 2 (KPScript) und Bitwarden/Vaultwarden (bw).
//!
//! Das Masterkennwort der Datenbank (bzw. der Sitzungsschlüssel von Bitwarden) wird nur im
//! Arbeitsspeicher gehalten, bis der Tresor gesperrt oder das Programm beendet wird.

use crate::model::{VaultKind, VaultSettings};
use eframe::egui;
use std::io::Write as _;
use std::os::windows::process::CommandExt as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

/// Wofür ein Eintrag geholt wird
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Purpose {
    /// Verbindung mit dieser ID öffnen
    Connect(u64),
    /// „Testen“ im Einstellungsdialog
    Test,
}

pub enum Event {
    /// Zugangsdaten geholt
    Fetched(Purpose, Credentials),
    /// Fehler (bei falschem Masterkennwort wird der Tresor wieder gesperrt)
    Failed(Purpose, String),
}

struct Job {
    purpose: Purpose,
    entry: String,
    label: String,
    rx: mpsc::Receiver<Result<(Credentials, Option<String>), Fail>>,
}

enum Fail {
    /// Masterkennwort falsch oder Sitzung abgelaufen → erneut fragen
    Auth(String),
    Other(String),
}

#[derive(Default)]
pub struct Vault {
    /// KeePass: Masterkennwort, Bitwarden: Sitzungsschlüssel
    secret: Option<String>,
    jobs: Vec<Job>,
    /// Wartet auf das Masterkennwort: (Zweck, Eintrag, Anzeigename)
    waiting: Vec<(Purpose, String, String)>,
    input: String,
    error: Option<String>,
}

impl Vault {
    pub fn is_unlocked(&self) -> bool {
        self.secret.is_some()
    }

    /// Masterkennwort vergessen
    pub fn lock(&mut self) {
        self.secret = None;
        self.input.clear();
    }

    pub fn busy(&self) -> bool {
        !self.jobs.is_empty() || !self.waiting.is_empty()
    }

    /// Eintrag holen; fragt vorher nach dem Masterkennwort, falls nötig.
    pub fn request(&mut self, settings: &VaultSettings, purpose: Purpose, entry: &str, label: &str, ctx: &egui::Context) {
        // Bitwarden: während des ersten Entsperrens warten, bis der Sitzungsschlüssel da ist
        let unlocking = self.secret.as_deref().is_some_and(|s| s.starts_with("pw:")) && !self.jobs.is_empty();
        if self.secret.is_none() || unlocking {
            self.waiting.push((purpose, entry.to_string(), label.to_string()));
            return;
        }
        self.spawn(settings, purpose, entry, label, ctx);
    }

    fn spawn_waiting(&mut self, settings: &VaultSettings, ctx: &egui::Context) {
        let mut rest = std::mem::take(&mut self.waiting);
        if settings.kind == VaultKind::Bitwarden && self.secret.as_deref().is_some_and(|s| s.starts_with("pw:")) {
            // nur einer entsperrt, die anderen warten auf den Sitzungsschlüssel
            if !rest.is_empty() {
                let (p, entry, label) = rest.remove(0);
                self.spawn(settings, p, &entry, &label, ctx);
            }
            self.waiting = rest;
        } else {
            for (p, entry, label) in rest {
                self.spawn(settings, p, &entry, &label, ctx);
            }
        }
    }

    fn spawn(&mut self, settings: &VaultSettings, purpose: Purpose, entry: &str, label: &str, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let (settings, entry_owned, secret, ctx2) = (settings.clone(), entry.to_string(), self.secret.clone(), ctx.clone());
        let first_unlock = settings.kind == VaultKind::Bitwarden && secret.as_deref().is_some_and(|s| s.starts_with("pw:"));
        std::thread::spawn(move || {
            let r = fetch(&settings, &entry_owned, secret.as_deref().unwrap_or(""), first_unlock);
            let _ = tx.send(r);
            ctx2.request_repaint();
        });
        self.jobs.push(Job { purpose, entry: entry.to_string(), label: label.to_string(), rx });
    }

    /// Abfragen auswerten, Dialoge zeichnen. Liefert fertige Ergebnisse.
    pub fn update(&mut self, ctx: &egui::Context, settings: &VaultSettings) -> Vec<Event> {
        let mut events = Vec::new();
        let mut new_session = None;
        let mut auth_failed = Vec::new();
        self.jobs.retain(|j| match j.rx.try_recv() {
            Ok(Ok((c, s))) => {
                if s.is_some() {
                    new_session = s;
                }
                events.push(Event::Fetched(j.purpose, c));
                false
            }
            Ok(Err(Fail::Auth(e))) => {
                // Masterkennwort falsch / Sitzung abgelaufen → erneut fragen
                auth_failed.push((j.purpose, j.entry.clone(), j.label.clone(), e));
                false
            }
            Ok(Err(Fail::Other(e))) => {
                events.push(Event::Failed(j.purpose, e));
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => false,
        });
        if let Some(s) = new_session {
            self.secret = Some(s);
        }
        if !auth_failed.is_empty() {
            self.secret = None;
            for (p, entry, label, e) in auth_failed {
                self.error = Some(e);
                self.waiting.push((p, entry, label));
            }
        }

        // --- Fortschritt
        if !self.jobs.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
            egui::Window::new("🔐 Passwort-Tresor")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_TOP, [0.0, 60.0])
                .show(ctx, |ui| {
                    for j in &self.jobs {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(format!("Hole Zugangsdaten für {} …", j.label));
                        });
                    }
                });
        }

        // --- Masterkennwort abfragen
        if !self.waiting.is_empty() && self.secret.is_none() {
            let mut ok = false;
            let mut cancel = false;
            let title = match settings.kind {
                VaultKind::Bitwarden => "🔐 Bitwarden entsperren",
                _ => "🔐 KeePass-Datenbank entsperren",
            };
            let names: Vec<&str> = self.waiting.iter().map(|w| w.2.as_str()).collect();
            let resp = egui::Modal::new(egui::Id::unique("vault_unlock")).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.heading(title);
                ui.label(format!("Für: {}", names.join(", ")));
                if settings.kind != VaultKind::Bitwarden {
                    ui.weak(&settings.database);
                }
                ui.add_space(4.0);
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.input)
                        .password(true)
                        .hint_text("Masterkennwort")
                        .desired_width(f32::INFINITY),
                );
                r.request_focus();
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.weak("Wird nur im Arbeitsspeicher gehalten, bis der Tresor gesperrt oder das Programm beendet wird.");
                if let Some(e) = &self.error {
                    ui.colored_label(egui::Color32::from_rgb(220, 70, 70), e);
                }
                ui.horizontal(|ui| {
                    if (ui.button("Entsperren").clicked() || enter) && !self.input.is_empty() {
                        ok = true;
                    }
                    if ui.button("Abbrechen").clicked() {
                        cancel = true;
                    }
                });
            });
            if ok {
                let pw = std::mem::take(&mut self.input);
                // Bitwarden: erst beim ersten Abruf entsperren (liefert den Sitzungsschlüssel)
                self.secret = Some(if settings.kind == VaultKind::Bitwarden { format!("pw:{pw}") } else { pw });
                self.error = None;
                self.spawn_waiting(settings, ctx);
            } else if cancel || resp.should_close() {
                self.waiting.clear();
                self.input.clear();
                self.error = None;
            }
        } else if !self.waiting.is_empty() && self.jobs.is_empty() {
            self.spawn_waiting(settings, ctx);
        }
        events
    }
}

// ---------------------------------------------------------------------------
// Programme aufrufen
// ---------------------------------------------------------------------------

/// Pfad zum Kommandozeilenprogramm: Einstellung, Standard-Installationsort oder PATH
pub fn program(settings: &VaultSettings) -> PathBuf {
    let configured = settings.program.trim();
    if !configured.is_empty() {
        return PathBuf::from(configured);
    }
    let pf = |var: &str, rest: &str| std::env::var_os(var).map(|p| PathBuf::from(p).join(rest));
    let candidates: Vec<Option<PathBuf>> = match settings.kind {
        VaultKind::KeePassXc => vec![pf("ProgramFiles", r"KeePassXC\keepassxc-cli.exe")],
        VaultKind::KpScript => vec![
            pf("ProgramFiles", r"KeePass Password Safe 2\KPScript.exe"),
            pf("ProgramFiles(x86)", r"KeePass Password Safe 2\KPScript.exe"),
        ],
        VaultKind::Bitwarden => vec![pf("APPDATA", r"npm\bw.cmd"), pf("LOCALAPPDATA", r"Programs\Bitwarden CLI\bw.exe")],
        VaultKind::None => vec![],
    };
    candidates
        .into_iter()
        .flatten()
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(settings.kind.default_program()))
}

/// Programm starten, `stdin` hineinschreiben, (Exit-Code ok?, stdout, stderr) liefern
fn run(prog: &PathBuf, args: &[String], stdin: Option<&str>, env: &[(&str, &str)]) -> Result<(bool, String, String), String> {
    let mut cmd = Command::new(prog);
    cmd.args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().map_err(|e| format!("{} konnte nicht gestartet werden: {e}", prog.display()))?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let _ = pipe.write_all(text.as_bytes());
        let _ = pipe.write_all(b"\n");
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n"),
    ))
}

fn first_line(s: &str) -> String {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string()
}

/// Ergebnis: Zugangsdaten und ggf. neuer Bitwarden-Sitzungsschlüssel
fn fetch(settings: &VaultSettings, entry: &str, secret: &str, first_unlock: bool) -> Result<(Credentials, Option<String>), Fail> {
    let prog = program(settings);
    let entry = entry.trim();
    if entry.is_empty() {
        return Err(Fail::Other("Kein Tresor-Eintrag angegeben.".into()));
    }
    match settings.kind {
        VaultKind::None => Err(Fail::Other("Kein Passwort-Tresor eingerichtet (Datei > Passwort-Tresor).".into())),
        VaultKind::KeePassXc => {
            if settings.database.trim().is_empty() {
                return Err(Fail::Other("Keine KeePass-Datenbank eingetragen.".into()));
            }
            let mut args: Vec<String> = vec!["show".into(), "-q".into(), "-s".into(), "-a".into(), "UserName".into(), "-a".into(), "Password".into()];
            if !settings.key_file.trim().is_empty() {
                args.extend(["-k".into(), settings.key_file.trim().into()]);
            }
            args.extend([settings.database.trim().into(), entry.into()]);
            let (ok, out, err) = run(&prog, &args, Some(secret), &[]).map_err(Fail::Other)?;
            if !ok {
                let msg = first_line(&err);
                let lower = msg.to_lowercase();
                return Err(if lower.contains("invalid credentials") || lower.contains("wrong key") || lower.contains("hmac") {
                    Fail::Auth("Falsches Masterkennwort für die KeePass-Datenbank.".into())
                } else {
                    Fail::Other(format!("KeePassXC: {}", if msg.is_empty() { first_line(&out) } else { msg }))
                });
            }
            Ok((parse_keepassxc(&out), None))
        }
        VaultKind::KpScript => {
            if settings.database.trim().is_empty() {
                return Err(Fail::Other("Keine KeePass-Datenbank eingetragen.".into()));
            }
            let field = |name: &str| -> Result<String, Fail> {
                let mut args: Vec<String> = vec![
                    "-c:GetEntryString".into(),
                    settings.database.trim().into(),
                    "-pw-stdin".into(),
                    format!("-Field:{name}"),
                    format!("-ref-Title:{entry}"),
                    "-FailIfNoEntry".into(),
                ];
                if !settings.key_file.trim().is_empty() {
                    args.push(format!("-keyfile:{}", settings.key_file.trim()));
                }
                let (_, out, err) = run(&prog, &args, Some(secret), &[]).map_err(Fail::Other)?;
                parse_kpscript(&out).map_err(|e| {
                    let e = if e.is_empty() { first_line(&err) } else { e };
                    let lower = e.to_lowercase();
                    if lower.contains("key") && (lower.contains("invalid") || lower.contains("wrong") || lower.contains("composite")) {
                        Fail::Auth("Falsches Masterkennwort für die KeePass-Datenbank.".into())
                    } else {
                        Fail::Other(format!("KPScript: {e}"))
                    }
                })
            };
            let username = field("UserName")?;
            let password = field("Password")?;
            Ok((Credentials { username, password }, None))
        }
        VaultKind::Bitwarden => {
            let mut new_session = None;
            let session = if let Some(pw) = secret.strip_prefix("pw:").filter(|_| first_unlock) {
                let (ok, out, err) =
                    run(&prog, &["unlock".into(), "--passwordenv".into(), "RDPM_BW_PW".into(), "--raw".into()], None, &[("RDPM_BW_PW", pw)])
                        .map_err(Fail::Other)?;
                let key = out.trim().to_string();
                if !ok || key.is_empty() {
                    let msg = first_line(&err);
                    return Err(if msg.to_lowercase().contains("not logged in") {
                        Fail::Other("Bitwarden: nicht angemeldet – einmalig „bw login“ in einer Konsole ausführen (Datei > Passwort-Tresor).".into())
                    } else if msg.to_lowercase().contains("invalid master password") {
                        Fail::Auth("Falsches Bitwarden-Masterkennwort.".into())
                    } else {
                        Fail::Auth(format!("Bitwarden: {msg}"))
                    });
                }
                new_session = Some(key.clone());
                key
            } else {
                secret.to_string()
            };
            let (ok, out, err) = run(
                &prog,
                &["get".into(), "item".into(), entry.into(), "--nointeraction".into()],
                None,
                &[("BW_SESSION", session.as_str())],
            )
            .map_err(Fail::Other)?;
            if !ok {
                let msg = first_line(&err);
                let lower = msg.to_lowercase();
                return Err(if lower.contains("locked") || lower.contains("not logged in") || lower.contains("session") {
                    Fail::Auth("Bitwarden ist gesperrt – bitte erneut entsperren.".into())
                } else if lower.contains("more than one result") {
                    Fail::Other(format!("Bitwarden: mehrere Einträge passen zu „{entry}“ – bitte die Element-ID verwenden."))
                } else if lower.contains("not found") {
                    Fail::Other(format!("Bitwarden: kein Eintrag „{entry}“ gefunden."))
                } else {
                    Fail::Other(format!("Bitwarden: {msg}"))
                });
            }
            let c = parse_bitwarden(&out).map_err(Fail::Other)?;
            Ok((c, new_session))
        }
    }
}

/// keepassxc-cli show -a UserName -a Password: je Attribut eine Zeile
fn parse_keepassxc(out: &str) -> Credentials {
    let mut lines = out.lines();
    let username = lines.next().unwrap_or("").to_string();
    let password = lines.next().unwrap_or("").to_string();
    Credentials { username, password }
}

/// KPScript: Wert(e), danach "OK: …" bzw. "E: Fehlermeldung"
fn parse_kpscript(out: &str) -> Result<String, String> {
    let lines: Vec<&str> = out.lines().collect();
    if let Some(e) = lines.iter().find_map(|l| l.strip_prefix("E: ")) {
        return Err(e.trim().to_string());
    }
    let ok = lines.iter().position(|l| l.starts_with("OK:")).ok_or_else(|| first_line(out))?;
    Ok(lines[..ok].join("\n"))
}

fn parse_bitwarden(json: &str) -> Result<Credentials, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("Bitwarden: unerwartete Antwort ({e})"))?;
    let login = v.get("login").ok_or("Bitwarden: Der Eintrag ist kein Anmelde-Eintrag.")?;
    let s = |k: &str| login.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    Ok(Credentials { username: s("username"), password: s("password") })
}

/// Einstellungsdialog: Ergebnis von „Testen“ / Konsole für `bw login`
pub fn settings_ui(ui: &mut egui::Ui, s: &mut VaultSettings, test_entry: &mut String) -> SettingsAction {
    let mut action = SettingsAction::None;
    egui::Grid::new("vault_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label("Tresor");
        egui::ComboBox::from_id_salt("vault_kind").selected_text(s.kind.label()).width(300.0).show_ui(ui, |ui| {
            for k in VaultKind::ALL {
                ui.selectable_value(&mut s.kind, k, k.label());
            }
        });
        ui.end_row();
        if s.kind == VaultKind::None {
            return;
        }
        if matches!(s.kind, VaultKind::KeePassXc | VaultKind::KpScript) {
            ui.label("Datenbank (.kdbx)");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut s.database).desired_width(260.0));
                if ui.button("…").clicked()
                    && let Some(p) = rfd::FileDialog::new().add_filter("KeePass", &["kdbx"]).pick_file()
                {
                    s.database = p.display().to_string();
                }
            });
            ui.end_row();
            ui.label("Schlüsseldatei");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut s.key_file).hint_text("optional").desired_width(260.0));
                if ui.button("…").clicked()
                    && let Some(p) = rfd::FileDialog::new().pick_file()
                {
                    s.key_file = p.display().to_string();
                }
            });
            ui.end_row();
        }
        ui.label("Programm");
        let found = program(s);
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut s.program).hint_text(found.display().to_string()).desired_width(260.0));
            if ui.button("…").clicked()
                && let Some(p) = rfd::FileDialog::new().add_filter("Programm", &["exe", "cmd"]).pick_file()
            {
                s.program = p.display().to_string();
            }
        });
        ui.end_row();
        ui.label("Test");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(test_entry).hint_text("Name eines Eintrags").desired_width(200.0));
            if ui.add_enabled(!test_entry.trim().is_empty(), egui::Button::new("Testen")).clicked() {
                action = SettingsAction::Test;
            }
        });
        ui.end_row();
    });
    ui.add_space(4.0);
    match s.kind {
        VaultKind::None => {
            ui.weak("Passwörter liegen dann nur in der Windows-Anmeldeinformationsverwaltung.");
        }
        VaultKind::KeePassXc => {
            ui.weak("Benötigt KeePassXC (enthält keepassxc-cli). Eintrag in der Verbindung: Titel oder Pfad, z. B. „Server/srv01“.");
        }
        VaultKind::KpScript => {
            ui.weak("Benötigt KeePass 2 und das Zusatzprogramm KPScript.exe im KeePass-Ordner. Eintrag in der Verbindung: Titel des Eintrags.");
        }
        VaultKind::Bitwarden => {
            ui.weak("Benötigt die Bitwarden-CLI (bw). Eigener Server: einmalig „bw config server https://…“. Eintrag in der Verbindung: Name oder Element-ID.");
            if ui.button("Konsole für „bw login“ öffnen").clicked() {
                action = SettingsAction::Login;
            }
        }
    }
    action
}

#[derive(PartialEq)]
pub enum SettingsAction {
    None,
    Test,
    Login,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsers() {
        assert_eq!(
            parse_keepassxc("admin\nge:heim\n"),
            Credentials { username: "admin".into(), password: "ge:heim".into() }
        );
        assert_eq!(parse_kpscript("s3cret\nOK: Operation completed successfully.\n").unwrap(), "s3cret");
        assert_eq!(parse_kpscript("E: The master key is invalid!\n").unwrap_err(), "The master key is invalid!");
        let bw = r#"{"id":"1","name":"srv","login":{"username":"CORP\\jdoe","password":"pw"}}"#;
        assert_eq!(parse_bitwarden(bw).unwrap(), Credentials { username: "CORP\\jdoe".into(), password: "pw".into() });
        assert!(parse_bitwarden(r#"{"id":"1","type":2}"#).is_err());
    }
}
