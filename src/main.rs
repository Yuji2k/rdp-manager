#![windows_subsystem = "windows"]

mod backup;
mod connlog;
mod importers;
mod lock;
mod model;
mod mremote;
mod notes;
mod rdp;
mod reach;
mod sftp;
mod shared;
mod ssh;
mod tools;
mod transfer;
mod tunnel;
mod update;
mod vault;
mod vnc;
mod web;
mod wol;

use eframe::egui;
use egui::{CollapsingHeader, Color32, RichText};
use model::{
    AuthLevel, Connection, Folder, Forward, ForwardKind, GatewayUsage, IconKind, Item, Protocol, ROOT_ID, Resolution,
    SoundMode, Store, TabColor, ThemeMode, VaultKind,
};
use std::time::Instant;
use std::collections::{BTreeSet, HashMap};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use rdp::{Session as RdpSession, State};
use ssh::SshSession;
use std::cell::Cell;
use std::collections::VecDeque;
use std::time::Duration;
use windows::Win32::Foundation::HWND;

// ---------------------------------------------------------------------------
// Tab = RDP- oder SSH-Sitzung
// ---------------------------------------------------------------------------

enum Kind {
    Rdp(RdpSession),
    Ssh(SshSession),
    Web(web::WebSession),
    Vnc(vnc::VncSession),
}

/// Höhe der Adressleiste über Web-Tabs (logische Punkte)
const WEB_TOOLBAR: f32 = 32.0;

struct Session {
    id: u64,
    title: String,
    host: String,
    icon: &'static str,
    conn: Connection,
    /// SSH: Tastaturfokus beim nächsten Zeichnen ins Terminal holen
    grab_focus: Cell<bool>,
    kind: Kind,
}

impl Session {
    fn rdp(s: RdpSession) -> Self {
        Self {
            id: s.id,
            title: s.title.clone(),
            host: s.host.clone(),
            icon: s.icon,
            conn: s.conn.clone(),
            grab_focus: Cell::new(false),
            kind: Kind::Rdp(s),
        }
    }
    fn ssh(s: SshSession) -> Self {
        Self {
            id: s.id,
            title: s.title.clone(),
            host: s.host.clone(),
            icon: s.icon,
            conn: s.conn.clone(),
            grab_focus: Cell::new(true),
            kind: Kind::Ssh(s),
        }
    }
    fn web(s: web::WebSession) -> Self {
        Self {
            id: s.id,
            title: s.title.clone(),
            host: s.host.clone(),
            icon: s.icon,
            conn: s.conn.clone(),
            grab_focus: Cell::new(false),
            kind: Kind::Web(s),
        }
    }
    fn vnc(s: vnc::VncSession) -> Self {
        Self {
            id: s.id,
            title: s.title.clone(),
            host: s.host.clone(),
            icon: s.icon,
            conn: s.conn.clone(),
            grab_focus: Cell::new(true),
            kind: Kind::Vnc(s),
        }
    }
    fn proto(&self) -> Protocol {
        match &self.kind {
            Kind::Rdp(_) => Protocol::Rdp,
            Kind::Ssh(_) => Protocol::Ssh,
            Kind::Web(_) => Protocol::Https,
            Kind::Vnc(_) => Protocol::Vnc,
        }
    }
    fn proto_label(&self) -> &'static str {
        match &self.kind {
            Kind::Web(_) => self.conn.protocol.label(),
            _ => self.proto().label(),
        }
    }
    fn is_logging(&self) -> bool {
        matches!(&self.kind, Kind::Ssh(s) if s.is_logging())
    }
    fn state(&self) -> &State {
        match &self.kind {
            Kind::Rdp(s) => s.state(),
            Kind::Ssh(s) => s.state(),
            Kind::Web(s) => s.state(),
            Kind::Vnc(s) => s.state(),
        }
    }
    fn seconds_running(&self) -> u64 {
        match &self.kind {
            Kind::Rdp(s) => s.seconds_running(),
            Kind::Ssh(s) => s.seconds_running(),
            Kind::Web(s) => s.seconds_running(),
            Kind::Vnc(s) => s.seconds_running(),
        }
    }
    fn update(&mut self) {
        match &mut self.kind {
            Kind::Rdp(s) => s.update(),
            Kind::Ssh(s) => s.update(),
            Kind::Web(s) => {
                s.update();
                // Seitentitel als Tab-Titel, wenn die Verbindung keinen eigenen Namen hat
                if self.conn.name.trim().is_empty() {
                    let t = s.page_title();
                    if !t.is_empty() {
                        self.title = t.chars().take(40).collect();
                    }
                }
            }
            Kind::Vnc(s) => s.update(),
        }
    }
    fn close(&mut self) {
        match &mut self.kind {
            Kind::Rdp(s) => s.close(),
            Kind::Ssh(s) => s.close(),
            Kind::Web(s) => s.close(),
            Kind::Vnc(s) => s.close(),
        }
    }
    fn focus(&self) {
        match &self.kind {
            Kind::Rdp(s) => s.focus(),
            Kind::Ssh(_) | Kind::Vnc(_) => self.grab_focus.set(true),
            Kind::Web(s) => s.focus(),
        }
    }
    fn place(&mut self, x: i32, y: i32, w: i32, h: i32, scale: u32) {
        match &mut self.kind {
            Kind::Rdp(s) => s.place(x, y, w, h, scale),
            Kind::Web(s) => {
                // oben Platz für die Adressleiste lassen
                let bar = (WEB_TOOLBAR * scale as f32 / 100.0).round() as i32;
                s.place(x, y + bar, w, (h - bar).max(1))
            }
            _ => {}
        }
    }
    fn set_cutouts(&mut self, c: &[(i32, i32, i32, i32)]) {
        match &mut self.kind {
            Kind::Rdp(s) => s.set_cutouts(c),
            Kind::Web(s) => s.set_cutouts(c),
            _ => {}
        }
    }
    fn hide(&mut self) {
        match &mut self.kind {
            Kind::Rdp(s) => s.hide(),
            Kind::Web(s) => s.hide(),
            _ => {}
        }
    }
    /// Eingebettetes Fenster (RDP/Web) in ein anderes Fenster umhängen
    fn set_parent(&mut self, parent: HWND) {
        match &mut self.kind {
            Kind::Rdp(s) => s.set_parent(parent),
            Kind::Web(s) => s.set_parent(parent),
            _ => {}
        }
    }
    fn color(&self) -> Option<Color32> {
        self.conn.color.rgb().map(|(r, g, b)| Color32::from_rgb(r, g, b))
    }
    fn is_mirror(&self) -> bool {
        matches!(&self.kind, Kind::Ssh(s) if s.mirror)
    }
    fn rdp_mut(&mut self) -> Result<&mut RdpSession, String> {
        match &mut self.kind {
            Kind::Rdp(s) => Ok(s),
            _ => Err("Nur für RDP-Sitzungen verfügbar.".into()),
        }
    }
    /// Eingebettetes Fenster (für Miniaturansichten)
    fn hwnd(&self) -> Option<HWND> {
        match &self.kind {
            Kind::Rdp(s) => Some(s.hwnd()),
            Kind::Web(s) => Some(s.hwnd()),
            _ => None,
        }
    }
}

/// Anmeldename für das Verbindungsprotokoll
fn login_of(c: &Connection) -> String {
    match c.protocol {
        Protocol::Rdp => c.full_username(),
        Protocol::Http | Protocol::Https => String::new(),
        _ => c.username.trim().to_string(),
    }
}

/// "DOMÄNE\benutzer" bzw. "benutzer@domäne" aus dem Tresor in Benutzer/Domäne aufteilen
fn apply_vault_user(conn: &mut Connection, user: &str) {
    let user = user.trim();
    if user.is_empty() {
        return;
    }
    match user.split_once('\\') {
        Some((d, u)) if conn.protocol == Protocol::Rdp => {
            conn.domain = d.to_string();
            conn.username = u.to_string();
        }
        _ => {
            if conn.protocol == Protocol::Rdp && user.contains('@') {
                conn.domain.clear();
            }
            conn.username = user.to_string();
        }
    }
}

fn main() -> eframe::Result {
    // Abstürze protokollieren (ohne Konsole sonst unsichtbar)
    std::panic::set_hook(Box::new(|info| {
        let bt = std::backtrace::Backtrace::force_capture();
        rdp::log(&format!("PANIC: {info}\n{bt}"));
    }));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RDP Manager")
            .with_inner_size([1400.0, 850.0])
            .with_min_inner_size([700.0, 400.0])
            .with_icon(egui::IconData {
                rgba: include_bytes!("../assets/icon_64.rgba").to_vec(),
                width: 64,
                height: 64,
            }),
        ..Default::default()
    };
    eframe::run_native("RDP Manager", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

// ---------------------------------------------------------------------------

enum Action {
    Connect(u64),
    ConnectExternal(u64),
    NewConnection(u64),
    EditConnection(u64),
    DuplicateConnection(u64),
    DeleteConnection(u64),
    NewFolder(u64),
    RenameFolder(u64),
    DeleteFolder(u64),
    Move(Item, u64),
    /// Alle Verbindungen eines Ordners öffnen (true = inkl. Unterordner)
    ConnectFolder(u64, bool),
    ManagePasswords,
    ManageSnippets,
    /// Externes Werkzeug für eine Verbindung starten (Verbindung, Werkzeug-Index)
    RunTool(u64, usize),
    ManageTools,
    ManageBackups,
    ReleaseNotes,
    ManageShared,
    ReloadShared,
    /// Eigene Verbindungen als gemeinsame Liste (ohne Passwörter) speichern
    ExportShared,
    /// Notizen einer Verbindung anzeigen/bearbeiten
    EditNotes(u64),
    /// SSH-Agent und Schlüssel
    SshAgent,
    /// Schnellsuche öffnen
    OpenPalette,
    /// RDP über alle Monitore (mstsc)
    ConnectAllMonitors(u64),
    /// Verbindungsprotokoll anzeigen
    ShowConnLog,
    /// Programm sperren
    LockNow,
    /// Sperre & Hauptkennwort einrichten
    Security,
    /// Passwort-Tresor einrichten
    VaultSettings,
    /// Import aus anderen Programmen
    Import(ImportSource),
    /// Übersicht aller Sitzungen ein/aus
    ToggleOverview,
    /// Favorit an/aus
    ToggleFavorite(u64),
    /// „Zuletzt verwendet“ leeren
    ClearRecent,
    /// Wake-on-LAN (true = danach verbinden, sobald erreichbar)
    Wake(u64, bool),
    /// Update-Prüfung / -Einstellungen
    Updates,
}

/// Wake-on-LAN, auf dessen Ergebnis gewartet wird
struct WakeJob {
    conn_id: u64,
    label: String,
    started: Instant,
    rx: std::sync::mpsc::Receiver<Result<(), String>>,
}

#[derive(Clone, Copy)]
enum ImportSource {
    MRemoteNg,
    RdcMan,
    Putty,
    SshConfig,
    Csv,
}

/// Befehle für die aktive Sitzung (Menü "Sitzung", Tab-Kontextmenü, ⌨-Knopf)
#[derive(Clone, Copy)]
enum SessionCmd {
    Keys(&'static str, &'static [u16]),
    SshKeys(&'static str, &'static [u8]),
    Paste,
    CopyScreen,
    Fullscreen,
    SmartSize,
    Reconnect,
    Close,
    /// Sitzung in der rechten Hälfte anzeigen
    SplitRight,
    /// Geteilte Ansicht beenden
    Unsplit,
    /// scp: Dateien hochladen / herunterladen
    Upload,
    Download,
    /// Schnipsel senden (Index, an alle SSH-Sitzungen)
    Snippet(usize, bool),
    ManageSnippets,
    /// Suche im Terminal-Verlauf
    Search,
    /// Mitschnitt an/aus
    ToggleLog,
    OpenLogs,
    VncCtrlAltDel,
    /// SSH: Passwort eintippen (sudo, su …)
    SendPassword,
    WebReload,
    WebExternal,
    /// Externes Werkzeug für die Verbindung des Tabs
    Tool(usize),
    /// In eigenem Fenster anzeigen / zurück ins Hauptfenster
    Detach,
    Attach,
    /// Gleichzeitige Eingabe für diese Sitzung an/aus, für alle SSH-Sitzungen, beenden
    ToggleMirror,
    MirrorAll,
    MirrorOff,
    /// SFTP-Dateibrowser ein-/ausblenden
    Sftp,
    /// Öffentlichen Schlüssel auf dem Server eintragen
    InstallKey,
    /// Notizen der Verbindung
    Notes,
    /// RDP: Auflösung jetzt an die Fenstergröße anpassen / automatisch anpassen an-aus
    FitNow,
    ToggleAutoResize,
    /// RDP: dieselbe Verbindung in mstsc über alle Monitore öffnen
    AllMonitors,
}

/// Zusätzlicher Zustand der Sitzung für das Menü
#[derive(Clone, Copy, Default)]
struct MenuFlags {
    logging: bool,
    mirror: bool,
    sftp: bool,
    detached: bool,
    saved: bool,
    auto_resize: bool,
}

/// Gemeinsames Menü mit Tastenkombinationen und Sitzungsfunktionen.
fn session_menu(
    ui: &mut egui::Ui,
    connected: bool,
    has_session: bool,
    proto: Option<Protocol>,
    snippets: &[model::Snippet],
    flags: MenuFlags,
) -> Option<SessionCmd> {
    let mut cmd = None;
    let logging = flags.logging;
    let is_ssh = proto == Some(Protocol::Ssh);
    if proto == Some(Protocol::Vnc) {
        ui.add_enabled_ui(connected, |ui| {
            if ui.button("Strg+Alt+Entf senden").clicked() {
                cmd = Some(SessionCmd::VncCtrlAltDel);
            }
            if ui.button("📋 Zwischenablage eintippen").on_hover_text("Tippt den Text der lokalen Zwischenablage in die Sitzung").clicked() {
                cmd = Some(SessionCmd::Paste);
            }
        });
    }
    if proto.is_some_and(Protocol::is_web) {
        ui.add_enabled_ui(connected, |ui| {
            if ui.button("🔄 Neu laden (F5)").clicked() {
                cmd = Some(SessionCmd::WebReload);
            }
        });
        if ui.button("↗ Im Standardbrowser öffnen").clicked() {
            cmd = Some(SessionCmd::WebExternal);
        }
    }
    if is_ssh {
        ui.add_enabled_ui(connected, |ui| {
            ui.menu_button("⌨ Taste senden", |ui| {
                for (label, bytes) in ssh::SSH_KEYS {
                    if ui.button(*label).clicked() {
                        cmd = Some(SessionCmd::SshKeys(label, bytes));
                    }
                }
            });
            ui.menu_button("📝 Schnipsel", |ui| {
                if snippets.is_empty() {
                    ui.weak("Noch keine Schnipsel angelegt");
                }
                for (i, s) in snippets.iter().enumerate() {
                    let name = if s.name.trim().is_empty() { s.command.lines().next().unwrap_or("") } else { s.name.as_str() };
                    if ui.button(name).on_hover_text(&s.command).clicked() {
                        cmd = Some(SessionCmd::Snippet(i, false));
                    }
                }
                if !snippets.is_empty() {
                    ui.separator();
                    ui.menu_button("An alle SSH-Sitzungen senden", |ui| {
                        for (i, s) in snippets.iter().enumerate() {
                            let name = if s.name.trim().is_empty() { s.command.lines().next().unwrap_or("") } else { s.name.as_str() };
                            if ui.button(name).on_hover_text(&s.command).clicked() {
                                cmd = Some(SessionCmd::Snippet(i, true));
                            }
                        }
                    });
                }
                ui.separator();
                if ui.button("Schnipsel verwalten…").clicked() {
                    cmd = Some(SessionCmd::ManageSnippets);
                }
            });
            if ui.button("📋 Einfügen (auch Rechtsklick / Strg+V)").clicked() {
                cmd = Some(SessionCmd::Paste);
            }
            if ui
                .button("🔑 Passwort senden (Strg+Umschalt+P)")
                .on_hover_text("Tippt das Passwort der Verbindung ein und drückt Enter – z. B. für sudo")
                .clicked()
            {
                cmd = Some(SessionCmd::SendPassword);
            }
            let mut mirror = flags.mirror;
            if ui
                .checkbox(&mut mirror, "📡 Gleichzeitige Eingabe")
                .on_hover_text("Was du in diesem Terminal tippst, geht auch an alle anderen markierten SSH-Sitzungen")
                .clicked()
            {
                cmd = Some(SessionCmd::ToggleMirror);
            }
            if ui.button("📡 Alle SSH-Sitzungen markieren").clicked() {
                cmd = Some(SessionCmd::MirrorAll);
            }
            if flags.mirror && ui.button("📡 Gleichzeitige Eingabe beenden (alle)").clicked() {
                cmd = Some(SessionCmd::MirrorOff);
            }
            ui.separator();
            let mut sftp = flags.sftp;
            if ui.checkbox(&mut sftp, "📁 Dateibrowser (SFTP)").clicked() {
                cmd = Some(SessionCmd::Sftp);
            }
            if ui.button("⬆ Dateien hochladen… (oder hineinziehen)").clicked() {
                cmd = Some(SessionCmd::Upload);
            }
            if ui.button("⬇ Herunterladen…").clicked() {
                cmd = Some(SessionCmd::Download);
            }
            if ui
                .button("🔑 Öffentlichen Schlüssel eintragen…")
                .on_hover_text("Trägt eine .pub-Datei in ~/.ssh/authorized_keys auf dem Server ein – danach Anmeldung ohne Passwort")
                .clicked()
            {
                cmd = Some(SessionCmd::InstallKey);
            }
        });
        ui.separator();
        if ui.button("🔍 Im Verlauf suchen (Strg+Umschalt+F)").clicked() {
            cmd = Some(SessionCmd::Search);
        }
        if ui.button("📄 Bildschirminhalt kopieren").clicked() {
            cmd = Some(SessionCmd::CopyScreen);
        }
        let mut on = logging;
        if ui.checkbox(&mut on, "Sitzung mitschneiden (Logdatei)").clicked() {
            cmd = Some(SessionCmd::ToggleLog);
        }
        if ui.button("📂 Logordner öffnen").clicked() {
            cmd = Some(SessionCmd::OpenLogs);
        }
    }
    if proto == Some(Protocol::Rdp) || proto.is_none() {
        rdp_menu(ui, connected && proto.is_some(), flags, &mut cmd);
    }
    ui.add_enabled_ui(has_session, |ui| {
        ui.separator();
        if flags.saved && ui.button("📝 Notizen…").clicked() {
            cmd = Some(SessionCmd::Notes);
        }
        if flags.detached {
            if ui.button("⇤ Zurück ins Hauptfenster").clicked() {
                cmd = Some(SessionCmd::Attach);
            }
        } else if ui.button("⧉ In eigenem Fenster öffnen").on_hover_text("Tab abdocken, z. B. auf einen zweiten Monitor").clicked() {
            cmd = Some(SessionCmd::Detach);
        }
        if ui.button("🔄 Neu verbinden").clicked() {
            cmd = Some(SessionCmd::Reconnect);
        }
        if ui.button("✖ Trennen / Tab schließen").clicked() {
            cmd = Some(SessionCmd::Close);
        }
    });
    cmd
}

/// RDP-spezifische Einträge (Tastenkombinationen, Vollbild, Smart Sizing)
fn rdp_menu(ui: &mut egui::Ui, enabled: bool, flags: MenuFlags, cmd: &mut Option<SessionCmd>) {
    ui.add_enabled_ui(enabled, |ui| {
        ui.menu_button("⌨ Tastenkombination senden", |ui| {
            for (i, (label, keys)) in rdp::HOTKEYS.iter().enumerate() {
                // Gruppen trennen: Strg/Alt | Druck | Windows-Taste
                if i == 7 || i == 9 {
                    ui.separator();
                }
                if ui.button(*label).clicked() {
                    *cmd = Some(SessionCmd::Keys(label, keys));
                }
            }
        });
        if ui.button("Strg+Alt+Entf senden").clicked() {
            let (label, keys) = rdp::HOTKEYS[0];
            *cmd = Some(SessionCmd::Keys(label, keys));
        }
        ui.separator();
        if ui.button("⛶ Vollbild").on_hover_text("Zurück mit Strg+Alt+Pause oder über die Verbindungsleiste").clicked() {
            *cmd = Some(SessionCmd::Fullscreen);
        }
        if ui.button("🔍 Smart Sizing umschalten").clicked() {
            *cmd = Some(SessionCmd::SmartSize);
        }
        if ui
            .button("↔ Auflösung jetzt anpassen")
            .on_hover_text("Stellt die Auflösung der Remotesitzung auf die aktuelle Fenstergröße ein – ohne neu zu verbinden")
            .clicked()
        {
            *cmd = Some(SessionCmd::FitNow);
        }
        let mut auto = flags.auto_resize;
        if ui
            .checkbox(&mut auto, "Auflösung automatisch anpassen")
            .on_hover_text("Ändert die Auflösung live, wenn das Fenster größer oder kleiner wird (auch beim Verschieben auf einen anderen Monitor)")
            .clicked()
        {
            *cmd = Some(SessionCmd::ToggleAutoResize);
        }
        if ui
            .button("🖥 Auf alle Monitore verschieben (mstsc)")
            .on_hover_text("Schließt den Tab und öffnet die Verbindung in mstsc im Vollbild über alle Monitore – die Remotesitzung wird dabei übernommen")
            .clicked()
        {
            *cmd = Some(SessionCmd::AllMonitors);
        }
    });
}

/// Schaltflächen im SSH-Agent-Dialog
enum AgentCmd {
    StartService,
    Add,
    Generate,
    OpenDir,
    Refresh,
}

/// Zeile der Passwortübersicht
struct PwRow {
    id: u64,
    is_folder: bool,
    /// Pfad im Baum inkl. Name
    path: String,
    user: String,
    stored: bool,
    /// Verbindung übernimmt die Anmeldedaten von diesem Ordner
    inherits_from: Option<String>,
}

enum Dialog {
    None,
    Connection {
        draft: Connection,
        folder: u64,
        password: String,
        has_password: bool,
        delete_password: bool,
        is_new: bool,
    },
    Password {
        conn: Connection,
        /// Unter dieser ID wird das Passwort gespeichert (Verbindung oder Ordner)
        cred_id: u64,
        username: String,
        password: String,
        remember: bool,
    },
    Folder {
        id: Option<u64>,
        parent: u64,
        name: String,
        /// Gemeinsame Anmeldedaten
        username: String,
        domain: String,
        password: String,
        has_password: bool,
        delete_password: bool,
    },
    ConfirmDelete {
        folder: Option<u64>,
        connection: Option<u64>,
        label: String,
    },
    Export {
        use_master: bool,
        master: String,
        master_repeat: String,
    },
    /// Übersicht der gespeicherten Passwörter
    Passwords {
        rows: Vec<PwRow>,
        /// Tresor-Einträge ohne zugehörige Verbindung/Ordner
        orphans: Vec<u64>,
        only_stored: bool,
        filter: String,
        /// (ID, neues Passwort) in Bearbeitung
        edit: Option<(u64, String)>,
        confirm_all: bool,
    },
    /// Befehls-Schnipsel bearbeiten
    Snippets {
        list: Vec<model::Snippet>,
    },
    /// Passwort für eine SSH-Sitzung eingeben (sudo …)
    SshPassword {
        session: u64,
        password: String,
        remember: bool,
    },
    /// SSH-Agent (Dienst, geladene Schlüssel)
    SshAgent {
        service: ssh::AgentService,
        keys: Result<Vec<String>, String>,
    },
    /// Versionshinweise; Versionen neuer als `since` werden hervorgehoben
    ReleaseNotes {
        since: String,
    },
    /// Sicherungen anzeigen / wiederherstellen
    Backups {
        list: Vec<backup::BackupInfo>,
        /// Index, dessen Wiederherstellung bestätigt werden soll
        confirm: Option<usize>,
    },
    /// Externe Werkzeuge bearbeiten
    Tools {
        list: Vec<model::ExtTool>,
    },
    /// Gemeinsame Verbindungslisten einrichten
    Shared {
        list: Vec<model::SharedSource>,
    },
    /// Dateien per scp hochladen
    Upload {
        session: u64,
        files: Vec<std::path::PathBuf>,
        remote_dir: String,
    },
    /// Datei/Ordner per scp herunterladen
    Download {
        session: u64,
        remote_path: String,
        local_dir: String,
    },
    Import {
        path: std::path::PathBuf,
        xml: String,
        needs_password: bool,
        password: String,
        replace: bool,
        error: Option<String>,
    },
    /// Sperre & Hauptkennwort
    Security {
        current: String,
        new_pw: String,
        repeat: String,
        remove_pw: bool,
        hello: bool,
        hello_available: bool,
        at_start: bool,
        idle_minutes: u32,
        error: Option<String>,
    },
    /// Passwort-Tresor (KeePass / Bitwarden)
    Vault {
        settings: model::VaultSettings,
        test_entry: String,
        /// Ergebnis von „Testen“ (ok?, Meldung)
        result: Option<(bool, String)>,
    },
    /// Update-Quelle und Prüfung
    Update {
        source: String,
        auto: bool,
    },
}

struct App {
    store: Store,
    parent: HWND,
    sessions: Vec<Session>,
    active: Option<u64>,
    next_session_id: u64,
    selected: Option<u64>,
    filter: String,
    dialog: Dialog,
    error: Option<String>,
    info: Option<String>,
    quick: String,
    quick_go: bool,
    ctx: egui::Context,
    session_cmd: Option<(u64, SessionCmd)>,
    /// Hatte ein SSH-Terminal im letzten Frame den Tastaturfokus?
    terminal_focused: bool,
    /// Inhaltsbereich der Tabs in physischen Pixeln (x, y, w, h)
    content_px: (i32, i32, i32, i32),
    scale: u32,
    /// Erreichbarkeitsprüfung für den Baum
    reach: reach::Reach,
    /// „Alle verbinden“: noch zu öffnende Verbindungen (nacheinander, wegen Passwortdialogen)
    pending_connects: VecDeque<u64>,
    /// Geteilte Ansicht: diese Sitzung wird rechts neben der aktiven angezeigt
    split: Option<u64>,
    /// Anteil der linken Hälfte in der geteilten Ansicht
    split_ratio: f32,
    /// Inhaltsbereich der rechten Hälfte (logische Punkte), falls geteilt
    split_rect: Option<egui::Rect>,
    /// scp-Übertragungen
    transfers: Vec<transfer::Transfer>,
    next_transfer_id: u64,
    show_transfers: bool,
    /// Gemeinsame (schreibgeschützte) Verbindungslisten
    shared: Vec<shared::SharedTree>,
    /// Bearbeitete Adresse je Web-Tab
    web_url_edit: std::collections::HashMap<u64, String>,
    /// Passwort, das nach dem Zeichnen gespeichert werden soll (Verbindungs-ID, Passwort)
    pending_pw_save: Option<(u64, String)>,
    /// In diesem Programmlauf schon gesichert?
    backup_done: bool,
    last_backup: Option<std::time::Instant>,
    /// Schnellsuche (Strg+P)
    palette: Option<Palette>,
    /// Tag-Filter im Baum
    tag_filter: BTreeSet<String>,
    /// Notizfenster: (Verbindungs-ID, Text, geändert)
    notes: Option<(u64, String, bool)>,
    /// SFTP-Dateibrowser je SSH-Sitzung
    sftp: HashMap<u64, sftp::SftpBrowser>,
    /// SSH-Tunnel, die gerade aufgebaut werden (mit der Verbindung, die danach geöffnet wird)
    pending_tunnels: Vec<PendingTunnel>,
    /// Laufende SSH-Tunnel
    tunnels: Vec<tunnel::Tunnel>,
    /// Abgedockte Sitzungen: (Sitzungs-ID, Fenstertitel)
    detached: Vec<(u64, String)>,
    /// Fensterhandle der abgedockten Fenster
    detached_hwnd: HashMap<u64, HWND>,
    /// Programmsperre
    lock: lock::Lock,
    /// Nach dem Entsperren den Export-Dialog öffnen
    export_after_unlock: bool,
    /// KeePass/Bitwarden
    vault: vault::Vault,
    /// Verbindungsprotokoll: Sitzungs-ID → (Protokoll-ID, gemeldeter Zustand 0/1=verbunden/2=Fehler)
    log_ids: HashMap<u64, (String, u8)>,
    log_view: Option<connlog::Viewer>,
    /// Übersicht aller Sitzungen statt des aktiven Tabs
    overview: bool,
    /// Miniaturbilder eingebetteter Sitzungen
    thumbs: HashMap<u64, (egui::TextureHandle, Instant)>,
    /// mstsc-Prozesse, die über einen SSH-Tunnel (lokaler Port) verbunden sind
    ext_procs: Vec<(std::process::Child, u16)>,
    /// Diese Verbindungen beim nächsten Öffnen über alle Monitore (mstsc)
    multimon_once: std::collections::HashSet<u64>,
    /// Laufendes „Aufwecken und verbinden“
    wakes: Vec<WakeJob>,
    /// Update-Prüfung: laufend, (true = vom Benutzer gestartet)
    upd_check: Option<(std::sync::mpsc::Receiver<Result<Option<update::Found>, String>>, bool)>,
    /// Gefundene neuere Version
    upd_found: Option<update::Found>,
    /// Ergebnis der letzten Prüfung für den Dialog (ok?, Meldung)
    upd_msg: Option<(bool, String)>,
    /// MSI wird geholt
    upd_fetch: Option<std::sync::mpsc::Receiver<Result<std::path::PathBuf, String>>>,
}

/// Zustand der Schnellsuche
#[derive(Default)]
struct Palette {
    query: String,
    sel: usize,
    opened: bool,
}

/// Eintrag der Schnellsuche
#[derive(Clone)]
enum PalItem {
    Session(u64),
    Connection(u64),
    Quick(String),
}

/// Verbindung, die nach dem Aufbau des SSH-Tunnels geöffnet wird
struct PendingTunnel {
    tunnel: tunnel::Tunnel,
    conn: Connection,
    cred_id: u64,
    password: Option<String>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let parent = match cc.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(h)) => HWND(h.hwnd.get() as *mut std::ffi::c_void),
            _ => HWND::default(),
        };
        if !parent.is_invalid() {
            rdp::prepare_parent(parent);
        }
        let (mut store, load_msg) = Store::load_checked();
        apply_theme(&cc.egui_ctx, store.theme);
        // Nach einem Update einmalig die Neuerungen zeigen (nicht bei einer frischen Installation)
        let current = env!("CARGO_PKG_VERSION");
        let existing_user = !store.root.folders.is_empty() || !store.root.connections.is_empty();
        let dialog = if store.last_seen_version != current
            && existing_user
            && (store.last_seen_version.is_empty() || notes::has_news(&store.last_seen_version))
        {
            Dialog::ReleaseNotes { since: store.last_seen_version.clone() }
        } else {
            // nichts Neues zu zeigen → Version merken (wird beim nächsten Speichern geschrieben)
            store.last_seen_version = current.to_string();
            Dialog::None
        };
        // Kommandozeile: rdp_manager.exe <ziel>  -> Quick Connect beim Start
        let start_target = std::env::args().nth(1).filter(|a| !a.trim().is_empty());
        let shared = store.shared_sources.iter().map(|s| shared::SharedTree::new(&s.name, &s.path, &cc.egui_ctx)).collect();
        let mut lock = lock::Lock::default();
        if store.lock.at_start {
            lock.lock(&store.lock);
        }
        // Tabs vom letzten Mal wieder öffnen (nacheinander, nach dem Entsperren)
        let pending_connects: VecDeque<u64> =
            if store.restore_tabs { store.open_tabs.iter().copied().collect() } else { VecDeque::new() };
        // leere Quelle = offizielle GitHub-Releases
        let upd_check = store.update_check.then(|| (update::check_async(&store.update_source, &cc.egui_ctx), false));
        Self {
            wakes: Vec::new(),
            upd_check,
            upd_found: None,
            upd_msg: None,
            upd_fetch: None,
            lock,
            export_after_unlock: false,
            vault: vault::Vault::default(),
            log_ids: HashMap::new(),
            log_view: None,
            overview: false,
            thumbs: HashMap::new(),
            ext_procs: Vec::new(),
            multimon_once: Default::default(),
            store,
            parent,
            sessions: Vec::new(),
            active: None,
            next_session_id: 1,
            selected: None,
            filter: String::new(),
            dialog,
            error: load_msg,
            info: None,
            quick_go: start_target.is_some(),
            quick: start_target.unwrap_or_default(),
            ctx: cc.egui_ctx.clone(),
            session_cmd: None,
            terminal_focused: false,
            content_px: (0, 0, 1280, 800),
            scale: 100,
            reach: reach::Reach::new(&cc.egui_ctx),
            pending_connects,
            split: None,
            split_ratio: 0.5,
            split_rect: None,
            transfers: Vec::new(),
            next_transfer_id: 1,
            show_transfers: false,
            shared,
            web_url_edit: Default::default(),
            pending_pw_save: None,
            backup_done: false,
            last_backup: None,
            palette: None,
            tag_filter: BTreeSet::new(),
            notes: None,
            sftp: HashMap::new(),
            pending_tunnels: Vec::new(),
            tunnels: Vec::new(),
            detached: Vec::new(),
            detached_hwnd: HashMap::new(),
        }
    }

    fn save(&mut self) {
        self.store.root.sort();
        // bisherigen Stand sichern: beim ersten Speichern pro Programmstart, danach höchstens stündlich
        if self.store.backup_enabled
            && (!self.backup_done || self.last_backup.is_none_or(|t| t.elapsed() > Duration::from_secs(3600)))
        {
            self.backup_now(None);
            self.backup_done = true;
        }
        if let Err(e) = self.store.save() {
            self.error = Some(format!("Speichern fehlgeschlagen: {e}"));
        }
    }

    /// Sichert die aktuelle connections.json (vor dem Überschreiben) und räumt alte Sicherungen auf.
    fn backup_now(&mut self, reason: Option<&str>) {
        if !self.store.backup_enabled {
            return;
        }
        let stamp = ssh::local_timestamp();
        let mut dirs = vec![backup::backup_dir()];
        let extra = self.store.backup_extra_dir.trim();
        if !extra.is_empty() {
            dirs.push(std::path::PathBuf::from(extra));
        }
        for dir in dirs {
            match backup::create(&Store::file(), &dir, &stamp, reason) {
                Ok(_) => backup::prune(&dir, self.store.backup_keep),
                Err(e) => self.error = Some(format!("{e} ({})", dir.display())),
            }
        }
        self.last_backup = Some(std::time::Instant::now());
    }

    /// Ersetzt alle Verbindungen durch den Stand einer Sicherung (der aktuelle Stand wird vorher gesichert).
    fn restore_backup(&mut self, path: &std::path::Path) {
        let restored = match backup::read(path) {
            Ok(s) => s,
            Err(e) => return self.error = Some(format!("Sicherung nicht lesbar: {e}")),
        };
        self.backup_now(Some("vor Wiederherstellung"));
        // Sicherungs-Einstellungen behalten, alles andere zurückholen
        let (enabled, keep, extra) =
            (self.store.backup_enabled, self.store.backup_keep, self.store.backup_extra_dir.clone());
        self.store = restored;
        self.store.backup_enabled = enabled;
        self.store.backup_keep = keep;
        self.store.backup_extra_dir = extra;
        self.shared =
            self.store.shared_sources.iter().map(|s| shared::SharedTree::new(&s.name, &s.path, &self.ctx)).collect();
        apply_theme(&self.ctx, self.store.theme);
        self.selected = None;
        if let Err(e) = self.store.save() {
            self.error = Some(format!("Speichern fehlgeschlagen: {e}"));
        }
        self.info = Some(format!("Wiederhergestellt: {}", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
    }

    /// Holt die Verbindungen einer Sicherung als neuen Ordner dazu (mit neuen IDs, Passwörter werden mitkopiert).
    fn import_backup(&mut self, path: &std::path::Path, label: &str) {
        let restored = match backup::read(path) {
            Ok(s) => s,
            Err(e) => return self.error = Some(format!("Sicherung nicht lesbar: {e}")),
        };
        let mut root = restored.root;
        fn renumber(f: &mut Folder, store: &mut Store) {
            f.id = store.new_id();
            for c in &mut f.connections {
                let new = store.new_id();
                if let Some(pw) = rdp::read_password(c.id) {
                    let _ = rdp::store_password(new, &c.full_username(), &pw);
                }
                c.id = new;
            }
            for sub in &mut f.folders {
                let old = sub.id;
                renumber(sub, store);
                if let Some(pw) = rdp::read_password(old) {
                    let _ = rdp::store_password(sub.id, &sub.full_username(), &pw);
                }
            }
        }
        renumber(&mut root, &mut self.store);
        root.name = format!("Sicherung {label}");
        root.username.clear();
        root.domain.clear();
        let n = count_connections(&root, true);
        self.store.root.folders.push(root);
        self.save();
        self.info = Some(format!("{n} Verbindungen aus der Sicherung als Ordner „Sicherung {label}“ hinzugefügt"));
    }

    /// Verbindung aus der eigenen Liste oder einer gemeinsamen Liste
    fn find_any(&self, id: u64) -> Option<&Connection> {
        self.store
            .root
            .find_connection(id)
            .or_else(|| self.shared.iter().find_map(|t| t.root.as_ref()?.find_connection(id)))
    }

    /// Wirksame Anmeldedaten: (Verbindung, ID des Passwort-Eintrags, Passwort aus gemeinsamer mRemoteNG-Datei)
    fn resolve_any(&self, conn: &Connection) -> (Connection, u64, Option<String>) {
        if self.store.root.find_connection(conn.id).is_some() {
            let (c, cred) = self.store.root.resolve_credentials(conn);
            return (c, cred, None);
        }
        for t in &self.shared {
            if let Some(root) = t.root.as_ref().filter(|r| r.find_connection(conn.id).is_some()) {
                let (c, cred) = root.resolve_credentials(conn);
                return (c, cred, t.passwords.get(&conn.id).cloned());
            }
        }
        (conn.clone(), conn.id, None)
    }

    /// Verbindung öffnen – fragt nach dem Passwort, falls keins gespeichert ist.
    fn connect(&mut self, conn_id: u64) {
        if self.find_any(conn_id).is_some() && self.store.recent.first() != Some(&conn_id) {
            self.store.recent.retain(|&r| r != conn_id);
            self.store.recent.insert(0, conn_id);
            self.store.recent.truncate(10);
            self.save();
        }
        self.connect_with(conn_id, None);
    }

    /// Wake-on-LAN für eine Verbindung; mit `then_connect` wird verbunden, sobald sie erreichbar ist
    fn wake(&mut self, conn_id: u64, then_connect: bool) {
        let Some(c) = self.find_any(conn_id).cloned() else { return };
        let Some(mac) = wol::parse_mac(&c.mac) else {
            self.error = Some(format!("Für „{}“ ist keine gültige MAC-Adresse eingetragen.", c.display_name()));
            return;
        };
        let (host, port) = if c.protocol.is_web() { url_host_port(&c.web_url(), c.port) } else { (c.host.trim().to_string(), c.port) };
        if !then_connect {
            match wol::send(mac, &host) {
                Ok(()) => self.info = Some(format!("Wake-on-LAN an {} ({}) gesendet", c.display_name(), wol::format_mac(mac))),
                Err(e) => self.error = Some(e),
            }
            return;
        }
        if c.via_ssh != 0 {
            self.error = Some("„Aufwecken und verbinden“ geht nicht über einen SSH-Sprunghost – bitte nur aufwecken.".into());
            return;
        }
        let rx = wol::wake_and_wait(mac, &host, port, Duration::from_secs(300), &self.ctx);
        self.wakes.push(WakeJob { conn_id, label: c.display_name().to_string(), started: Instant::now(), rx });
    }

    /// Fortschritt von „Aufwecken und verbinden“
    fn wake_window(&mut self, ctx: &egui::Context) {
        let mut done = Vec::new();
        self.wakes.retain(|w| match w.rx.try_recv() {
            Ok(r) => {
                done.push((w.conn_id, w.label.clone(), r));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
        });
        for (id, label, r) in done {
            match r {
                Ok(()) => {
                    self.info = Some(format!("{label} ist wach"));
                    self.connect(id);
                }
                Err(e) => self.error = Some(e),
            }
        }
        if self.wakes.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_secs(1));
        let mut cancel = None;
        egui::Window::new("⏰ Wake-on-LAN").collapsible(false).resizable(false).anchor(egui::Align2::RIGHT_BOTTOM, [-12.0, -40.0]).show(
            ctx,
            |ui| {
                for (i, w) in self.wakes.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Wecke {} … ({} s)", w.label, w.started.elapsed().as_secs()));
                        if ui.small_button("Abbrechen").clicked() {
                            cancel = Some(i);
                        }
                    });
                }
                ui.weak("Verbindet automatisch, sobald der Rechner antwortet (max. 5 min).");
            },
        );
        if let Some(i) = cancel {
            self.wakes.remove(i); // der Hintergrund-Thread läuft aus, sein Ergebnis wird verworfen
        }
    }

    /// Ergebnisse der Update-Prüfung und des Downloads auswerten
    fn poll_updates(&mut self, ctx: &egui::Context) {
        if let Some((rx, manual)) = &self.upd_check {
            let manual = *manual;
            match rx.try_recv() {
                Ok(r) => {
                    self.upd_check = None;
                    match r {
                        Ok(Some(f)) => {
                            self.upd_msg = Some((true, format!("Version {} ist verfügbar.", update::version_text(f.version))));
                            self.upd_found = Some(f);
                        }
                        Ok(None) => {
                            self.upd_found = None;
                            self.upd_msg = Some((true, format!("Du hast die neueste Version ({}).", env!("CARGO_PKG_VERSION"))));
                        }
                        Err(e) => {
                            // automatische Prüfung beim Start: Fehler nur im Dialog zeigen, nicht stören
                            if manual && !matches!(self.dialog, Dialog::Update { .. }) {
                                self.error = Some(format!("Update-Prüfung: {e}"));
                            }
                            self.upd_msg = Some((false, e));
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(300)),
                Err(_) => self.upd_check = None,
            }
        }
        if let Some(rx) = &self.upd_fetch {
            match rx.try_recv() {
                Ok(Ok(msi)) => {
                    self.upd_fetch = None;
                    match update::run_installer(&msi) {
                        // Programm beenden, damit der Installer die Dateien ersetzen kann
                        Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => self.error = Some(e),
                    }
                }
                Ok(Err(e)) => {
                    self.upd_fetch = None;
                    self.upd_msg = Some((false, e.clone()));
                    self.error = Some(format!("Update: {e}"));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(300)),
                Err(_) => self.upd_fetch = None,
            }
        }
    }

    /// Wie `connect`; `from_vault` = Zugangsdaten, die schon aus KeePass/Bitwarden geholt wurden.
    fn connect_with(&mut self, conn_id: u64, from_vault: Option<vault::Credentials>) {
        let Some(conn) = self.find_any(conn_id).cloned() else { return };
        if conn.host.trim().is_empty() {
            self.error = Some("Für diese Verbindung ist kein Host eingetragen.".into());
            return;
        }
        // ggf. Benutzer/Domäne/Passwort vom Ordner übernehmen
        let (mut conn, cred_id, known_pw) = self.resolve_any(&conn);
        let mut stored = known_pw.or_else(|| rdp::read_password(cred_id));
        // Zugangsdaten aus dem Passwort-Tresor
        if !conn.vault_entry.trim().is_empty() && self.store.vault.kind != VaultKind::None && !conn.protocol.is_web() {
            match from_vault {
                None => {
                    let label = conn.display_name().to_string();
                    let entry = conn.vault_entry.clone();
                    self.vault.request(&self.store.vault, vault::Purpose::Connect(conn_id), &entry, &label, &self.ctx);
                    return;
                }
                Some(c) => {
                    apply_vault_user(&mut conn, &c.username);
                    if !c.password.is_empty() {
                        stored = Some(c.password);
                    }
                }
            }
        }
        if self.multimon_once.remove(&conn_id) && conn.protocol == Protocol::Rdp {
            conn.multimon = true;
        }
        // über eine SSH-Verbindung (Sprunghost)
        if conn.via_ssh != 0 && conn.via_ssh != conn.id {
            let Some(jump) = self.find_any(conn.via_ssh).cloned() else {
                self.error = Some(format!("Die SSH-Verbindung für den Tunnel von „{}“ gibt es nicht mehr.", conn.display_name()));
                return;
            };
            let (jump, jump_cred, jump_pw) = self.resolve_any(&jump);
            if conn.protocol == Protocol::Ssh {
                // SSH: ProxyJump (-J benutzer@sprunghost:port)
                let user = jump.username.trim();
                let target = if user.is_empty() { jump.host.trim().to_string() } else { format!("{user}@{}", jump.host.trim()) };
                conn.ssh_extra_args = format!("-J {target}:{} {}", jump.port, conn.ssh_extra_args).trim().to_string();
            } else {
                let jump_pw = jump_pw.or_else(|| rdp::read_password(jump_cred));
                let (host, port) = if conn.host.contains("://") {
                    url_host_port(&conn.host, conn.port)
                } else {
                    (conn.host.trim().to_string(), conn.port)
                };
                let label = format!("{} über {}", conn.display_name(), jump.display_name());
                match tunnel::Tunnel::start(&jump, &host, port, jump_pw, label, &self.ctx) {
                    Ok(t) => self.pending_tunnels.push(PendingTunnel { tunnel: t, conn, cred_id, password: stored }),
                    Err(e) => self.error = Some(e),
                }
                return;
            }
        }
        self.open_resolved(conn, cred_id, stored);
    }

    /// Öffnet eine Verbindung mit bereits ermittelten Anmeldedaten.
    fn open_resolved(&mut self, conn: Connection, cred_id: u64, stored: Option<String>) {
        match conn.protocol {
            Protocol::Vnc => return self.start_vnc(&conn, stored),
            Protocol::Http | Protocol::Https => return self.start_web(&conn),
            _ => {}
        }
        if conn.protocol == Protocol::Ssh {
            // ssh fragt selbst im Terminal; ein gespeichertes Passwort wird automatisch eingegeben
            self.start_ssh(&conn, stored);
            return;
        }
        if conn.multimon {
            // über alle Monitore → mstsc (das eingebettete Control kann das nicht zuverlässig)
            return self.launch_mstsc(&conn, stored.as_deref());
        }
        match stored {
            Some(pw) => self.start_session(&conn, Some(&pw)),
            None if conn.username.trim().is_empty() => self.start_session(&conn, None),
            None => {
                self.dialog = Dialog::Password {
                    username: conn.full_username(),
                    conn,
                    cred_id,
                    password: String::new(),
                    remember: true,
                }
            }
        }
    }

    /// Quick Connect: "[benutzer@]host[:port]". Gespeicherte Verbindungen zum selben Host werden
    /// bevorzugt (inkl. Einstellungen und Passwort), sonst wird mit Standardeinstellungen verbunden.
    fn quick_connect(&mut self, input: &str) {
        let input = input.trim();
        if input.is_empty() {
            return;
        }
        // Verlauf pflegen
        self.store.quick_history.retain(|h| !h.eq_ignore_ascii_case(input));
        self.store.quick_history.insert(0, input.to_string());
        self.store.quick_history.truncate(15);
        self.save();

        // Protokoll: "ssh://…", "ssh …", "rdp://…", "vnc://…", "http(s)://…";
        // ohne Präfix RDP (bzw. SSH bei Port 22, VNC bei 5900)
        let lower = input.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            let protocol = if lower.starts_with("https://") { Protocol::Https } else { Protocol::Http };
            let name = input.split("://").nth(1).unwrap_or(input).split('/').next().unwrap_or(input).to_string();
            let conn = Connection {
                id: 0,
                name,
                host: input.to_string(),
                protocol,
                port: protocol.default_port(),
                icon: IconKind::Web,
                ignore_cert_errors: true,
                ..Default::default()
            };
            return self.connect_temp(conn);
        }
        let (mut protocol, rest) = if lower.starts_with("vnc://") {
            (Some(Protocol::Vnc), &input[6..])
        } else if lower.starts_with("ssh://") {
            (Some(Protocol::Ssh), &input[6..])
        } else if lower.starts_with("ssh ") {
            (Some(Protocol::Ssh), &input[4..])
        } else if lower.starts_with("rdp://") {
            (Some(Protocol::Rdp), &input[6..])
        } else {
            (None, input)
        };
        let rest = rest.trim().trim_end_matches('/');
        let (user, hostport) = match rest.rsplit_once('@') {
            Some((u, h)) => (u.trim().to_string(), h.trim()),
            None => (String::new(), rest),
        };
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) if p.parse::<u16>().is_ok() && !h.contains(':') => (h.to_string(), Some(p.parse::<u16>().unwrap())),
            _ => (hostport.to_string(), None),
        };
        if protocol.is_none() && port == Some(22) {
            protocol = Some(Protocol::Ssh);
        }
        if protocol.is_none() && port.is_some_and(|p| (5900..=5999).contains(&p)) {
            protocol = Some(Protocol::Vnc);
        }
        let protocol = protocol.unwrap_or(Protocol::Rdp);
        let port = port.unwrap_or(protocol.default_port());
        if user.is_empty() {
            let found = find_by_host(&self.store.root, &host, port)
                .or_else(|| self.shared.iter().find_map(|t| find_by_host(t.root.as_ref()?, &host, port)));
            if let Some(id) = found {
                self.connect(id);
                return;
            }
        }
        let icon = if protocol == Protocol::Ssh { IconKind::Linux } else { IconKind::Server };
        let conn =
            Connection { id: 0, name: host.clone(), host, port, protocol, icon, username: user.clone(), ..Default::default() };
        self.connect_temp(conn);
    }

    /// Verbindung, die nicht gespeichert ist (Quick Connect) – RDP fragt nach den Anmeldedaten.
    fn connect_temp(&mut self, conn: Connection) {
        if conn.id != 0 && self.find_any(conn.id).is_some() {
            self.connect(conn.id);
            return;
        }
        match conn.protocol {
            Protocol::Ssh => return self.start_ssh(&conn, None),
            Protocol::Vnc => return self.start_vnc(&conn, None),
            Protocol::Http | Protocol::Https => return self.start_web(&conn),
            Protocol::Rdp => {}
        }
        self.dialog =
            Dialog::Password { username: conn.full_username(), conn, cred_id: 0, password: String::new(), remember: false };
    }

    /// Startet eine scp-Übertragung für die SSH-Sitzung `session`.
    /// Upload: `local` = Dateien, `remote` = Zielordner. Download: `local` = [Zielordner], `remote` = Pfad.
    fn start_transfer(&mut self, session: u64, upload: bool, local: Vec<std::path::PathBuf>, remote: String) {
        let Some(s) = self.sessions.iter_mut().find(|s| s.id == session) else { return };
        let Kind::Ssh(t) = &mut s.kind else { return };
        if upload {
            t.remote_dir = remote.trim().to_string();
        }
        let conn = t.conn.clone();
        let password = t.password();
        let id = self.next_transfer_id;
        self.next_transfer_id += 1;
        match transfer::Transfer::start(id, &conn, upload, local, &remote, password, &self.ctx) {
            Ok(tr) => {
                self.transfers.push(tr);
                self.show_transfers = true;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Fenster mit laufenden und abgeschlossenen Übertragungen
    fn transfers_window(&mut self, ctx: &egui::Context) {
        let mut any_done = false;
        for t in &mut self.transfers {
            let before = t.state.clone();
            t.update();
            if before != t.state {
                match &t.state {
                    transfer::TState::Done => {
                        any_done = true;
                        self.info = Some(format!("Übertragung abgeschlossen: {}", t.label))
                    }
                    transfer::TState::Failed(e) => self.error = Some(format!("Übertragung fehlgeschlagen: {e}")),
                    transfer::TState::NeedsPassword => self.show_transfers = true,
                    _ => {}
                }
            }
        }
        if any_done {
            // hochgeladene Dateien im Dateibrowser anzeigen
            for b in self.sftp.values_mut() {
                if !b.is_busy() {
                    b.refresh();
                }
            }
        }
        if self.transfers.iter().any(transfer::Transfer::is_active) {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if self.transfers.is_empty() || !self.show_transfers {
            return;
        }
        let mut open = true;
        let mut remove = None;
        egui::Window::new("⇅ Übertragungen")
            .open(&mut open)
            .default_width(460.0)
            .resizable(true)
            .collapsible(true)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for t in &mut self.transfers {
                        ui.push_id(t.id, |ui| {
                            ui.label(&t.label);
                            match &t.state {
                                transfer::TState::Running => {
                                    let text = if t.current.is_empty() {
                                        "Verbinde …".to_string()
                                    } else {
                                        format!("{} – {:.0} %", t.current, t.progress * 100.0)
                                    };
                                    ui.add(egui::ProgressBar::new(t.progress).text(text).animate(t.progress == 0.0));
                                }
                                transfer::TState::NeedsPassword => {
                                    ui.horizontal(|ui| {
                                        ui.label("Passwort:");
                                        let r = ui.add(egui::TextEdit::singleline(&mut t.password_input).password(true).desired_width(180.0));
                                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                        if ui.button("Senden").clicked() || enter {
                                            t.send_password();
                                        }
                                    });
                                }
                                transfer::TState::Done => {
                                    ui.colored_label(Color32::from_rgb(60, 170, 90), "✔ Fertig");
                                }
                                transfer::TState::Failed(e) => {
                                    ui.colored_label(Color32::from_rgb(220, 70, 70), format!("⛔ {e}"));
                                }
                                transfer::TState::Cancelled => {
                                    ui.weak("Abgebrochen");
                                }
                            }
                            ui.horizontal(|ui| {
                                if t.is_active() {
                                    if ui.small_button("Abbrechen").clicked() {
                                        t.cancel();
                                    }
                                } else if ui.small_button("Entfernen").clicked() {
                                    remove = Some(t.id);
                                }
                            });
                            ui.separator();
                        });
                    }
                });
                if self.transfers.iter().any(|t| !t.is_active()) && ui.button("Abgeschlossene entfernen").clicked() {
                    self.transfers.retain(transfer::Transfer::is_active);
                }
            });
        if let Some(id) = remove {
            self.transfers.retain(|t| t.id != id);
        }
        if !open {
            self.show_transfers = false;
        }
    }

    fn open_agent_dialog(&mut self) {
        self.dialog = Dialog::SshAgent { service: ssh::agent_service(), keys: ssh::agent_keys() };
    }

    /// SSH-Tunnel aktualisieren: fertige Tunnel öffnen ihre Verbindung, unbenutzte werden beendet.
    fn update_tunnels(&mut self, ctx: &egui::Context) {
        let mut ready = Vec::new();
        let mut i = 0;
        while i < self.pending_tunnels.len() {
            let p = &mut self.pending_tunnels[i];
            p.tunnel.update();
            match &p.tunnel.state {
                tunnel::TunState::Ready => ready.push(self.pending_tunnels.remove(i)),
                tunnel::TunState::Failed(e) => {
                    self.error = Some(e.clone());
                    self.pending_tunnels.remove(i);
                }
                _ => i += 1,
            }
        }
        for p in ready {
            let conn = via_local_port(p.conn, p.tunnel.local_port);
            self.tunnels.push(p.tunnel);
            self.open_resolved(conn, p.cred_id, p.password);
        }
        // Tunnel, die keine Sitzung mehr nutzt, nach einer Schonfrist beenden
        let mut used: Vec<u16> =
            self.sessions.iter().filter(|s| s.conn.host.contains("127.0.0.1")).map(|s| s.conn.port).collect();
        // mstsc-Fenster (alle Monitore) über einen Tunnel
        self.ext_procs.retain_mut(|(c, _)| matches!(c.try_wait(), Ok(None)));
        used.extend(self.ext_procs.iter().map(|(_, p)| *p));
        if let Dialog::Password { conn, .. } = &self.dialog {
            used.push(conn.port);
        }
        let mut lost = None;
        self.tunnels.retain_mut(|t| {
            t.update();
            let in_use = used.contains(&t.local_port);
            if let tunnel::TunState::Failed(e) = &t.state {
                if in_use {
                    lost = Some(e.clone());
                }
                return false;
            }
            let keep = in_use || t.started.elapsed() < Duration::from_secs(20);
            if !keep {
                t.close();
            }
            keep
        });
        if let Some(e) = lost {
            self.error = Some(e);
        }
        if !self.tunnels.is_empty() || !self.pending_tunnels.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(if self.pending_tunnels.is_empty() { 2000 } else { 250 }));
        }
        if self.pending_tunnels.is_empty() {
            return;
        }
        let mut cancel = None;
        egui::Window::new("🔀 SSH-Tunnel wird aufgebaut").collapsible(false).resizable(false).default_width(380.0).show(
            ctx,
            |ui| {
                for (i, p) in self.pending_tunnels.iter_mut().enumerate() {
                    ui.push_id(i, |ui| {
                        ui.horizontal(|ui| {
                            if p.tunnel.state != tunnel::TunState::NeedsPassword {
                                ui.spinner();
                            }
                            ui.label(&p.tunnel.label);
                            ui.weak(format!("{} s", p.tunnel.started.elapsed().as_secs()));
                        });
                        if p.tunnel.state == tunnel::TunState::NeedsPassword {
                            ui.horizontal(|ui| {
                                ui.label(format!("Passwort für {}:", p.tunnel.via));
                                let r = ui.add(
                                    egui::TextEdit::singleline(&mut p.tunnel.password_input).password(true).desired_width(150.0),
                                );
                                r.request_focus();
                                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                if ui.button("Senden").clicked() || enter {
                                    p.tunnel.send_password();
                                }
                            });
                        }
                        if ui.small_button("Abbrechen").clicked() {
                            cancel = Some(i);
                        }
                        ui.separator();
                    });
                }
            },
        );
        if let Some(i) = cancel {
            self.pending_tunnels.remove(i);
        }
    }

    /// Notizfenster (nicht modal, bleibt neben der Sitzung offen)
    fn notes_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.notes.as_ref().map(|n| n.0) else { return };
        let Some(conn) = self.find_any(id).cloned() else {
            self.notes = None;
            return;
        };
        let own = self.store.root.find_connection(id).is_some();
        let Some((_, text, dirty)) = &mut self.notes else { return };
        let mut open = true;
        let mut save = false;
        egui::Window::new(format!("📝 Notizen – {}", conn.display_name()))
            .id(egui::Id::unique("notes_window"))
            .open(&mut open)
            .default_width(420.0)
            .default_height(260.0)
            .resizable(true)
            .show(ctx, |ui| {
                if !own {
                    ui.weak("Gemeinsame Liste – nur lesbar.");
                }
                let r = ui.add_enabled(
                    own,
                    egui::TextEdit::multiline(text)
                        .hint_text("z. B. Wartungsfenster, Ansprechpartner, Besonderheiten …")
                        .desired_rows(10)
                        .desired_width(f32::INFINITY),
                );
                if r.changed() {
                    *dirty = true;
                }
                if own {
                    ui.horizontal(|ui| {
                        if ui.add_enabled(*dirty, egui::Button::new("💾 Speichern")).clicked() {
                            save = true;
                        }
                        if *dirty {
                            ui.weak("ungespeichert");
                        }
                    });
                }
            });
        if save || (!open && *dirty) {
            let memo = text.clone();
            if let Some(f) = self.store.root.parent_of_connection(id).and_then(|p| self.store.root.find_folder_mut(p))
                && let Some(c) = f.connections.iter_mut().find(|c| c.id == id)
            {
                c.memo = memo.clone();
            }
            for s in self.sessions.iter_mut().filter(|s| s.conn.id == id) {
                s.conn.memo = memo.clone();
            }
            self.save();
            if let Some((_, _, d)) = &mut self.notes {
                *d = false;
            }
        }
        if !open {
            self.notes = None;
        }
    }

    /// Einträge der Schnellsuche: offene Sitzungen, eigene und gemeinsame Verbindungen, Quick Connect.
    fn palette_items(&self, query: &str) -> Vec<(PalItem, String, String)> {
        let terms = model::search_terms(query);
        let mut out: Vec<(u8, String, PalItem, String, String)> = Vec::new();
        let first = terms.first().map(String::as_str).unwrap_or("");
        let rank = |name: &str| {
            let n = name.to_lowercase();
            if first.is_empty() || first.starts_with('#') {
                1
            } else if n.starts_with(first) {
                0
            } else if n.contains(first) {
                1
            } else {
                2
            }
        };
        for s in &self.sessions {
            if s.conn.matches_terms(&terms, &s.title) {
                let detail = format!("Offener Tab · {} · {}", s.proto().label(), s.host);
                out.push((0, s.title.to_lowercase(), PalItem::Session(s.id), format!("{} {}", s.icon, s.title), detail));
            }
        }
        let mut add_tree = |root: &Folder, prefix: &str| {
            let mut list = Vec::new();
            collect_with_path(root, prefix, &mut list);
            for (c, path) in list {
                if terms.is_empty() || c.matches_terms(&terms, &path) {
                    let mut detail = format!("{} · {}", c.protocol.label(), c.host.trim());
                    if !path.is_empty() {
                        detail = format!("{path} · {detail}");
                    }
                    if !c.tags.trim().is_empty() {
                        detail.push_str(&format!(" · #{}", c.tag_list().join(" #")));
                    }
                    let name = c.display_name().to_string();
                    out.push((
                        1 + rank(&name),
                        name.to_lowercase(),
                        PalItem::Connection(c.id),
                        format!("{} {name}", c.icon.glyph()),
                        detail,
                    ));
                }
            }
        };
        add_tree(&self.store.root, "");
        for t in &self.shared {
            if let Some(root) = &t.root {
                add_tree(root, &format!("🌐 {}", t.name));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let mut items: Vec<(PalItem, String, String)> = out.into_iter().map(|(_, _, i, l, d)| (i, l, d)).take(200).collect();
        let q = query.trim();
        if !q.is_empty() && !q.starts_with('#') {
            items.push((PalItem::Quick(q.to_string()), format!("⚡ Quick Connect: {q}"), "Direkt verbinden".into()));
        }
        items
    }

    /// Schnellsuche (Strg+P): oben mittig über allem, Pfeiltasten + Enter
    fn palette_ui(&mut self, ctx: &egui::Context) {
        let Some(p) = &mut self.palette else { return };
        let (mut query, mut sel, first) = (p.query.clone(), p.sel, !p.opened);
        p.opened = true;
        let items = self.palette_items(&query);
        let mut pick: Option<PalItem> = None;
        let mut close = false;
        // Tasten vor dem Textfeld abfangen
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                sel = (sel + 1).min(items.len().saturating_sub(1));
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                sel = sel.saturating_sub(1);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                close = true;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                pick = items.get(sel).map(|x| x.0.clone());
            }
        });
        let area = egui::Area::new(egui::Id::unique("palette"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 70.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(10).show(ui, |ui| {
                    ui.set_width(560.0);
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut query)
                            .hint_text("🔍 Verbindung suchen – Name, Host, Notizen, #tag …")
                            .desired_width(f32::INFINITY)
                            .font(egui::TextStyle::Heading),
                    );
                    r.request_focus();
                    if r.changed() {
                        sel = 0;
                    }
                    if first {
                        // gesamten Text auswählen ist hier unnötig – Feld ist leer
                        ui.ctx().request_repaint();
                    }
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                        if items.is_empty() {
                            ui.weak("Keine Treffer");
                        }
                        for (i, (item, label, detail)) in items.iter().enumerate() {
                            let selected = i == sel;
                            let resp = ui
                                .scope(|ui| {
                                    let fill = if selected { ui.visuals().selection.bg_fill } else { Color32::TRANSPARENT };
                                    egui::Frame::new().fill(fill).corner_radius(4).inner_margin(egui::Margin::symmetric(6, 3)).show(
                                        ui,
                                        |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.label(RichText::new(label).strong());
                                            ui.weak(detail);
                                        },
                                    )
                                })
                                .inner
                                .response
                                .interact(egui::Sense::click());
                            if selected {
                                resp.scroll_to_me(None);
                            }
                            if resp.hovered() && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                                sel = i;
                            }
                            if resp.clicked() {
                                pick = Some(item.clone());
                            }
                        }
                    });
                    ui.add_space(2.0);
                    ui.weak("↑↓ auswählen · Enter öffnen · Esc schließen · #tag filtert nach Tags");
                });
            });
        // Klick daneben schließt
        if ctx.input(|i| i.pointer.any_pressed())
            && ctx.pointer_latest_pos().is_some_and(|p| !area.response.rect.contains(p))
        {
            close = true;
        }
        if let Some(p) = &mut self.palette {
            p.query = query;
            p.sel = sel.min(items.len().saturating_sub(1));
        }
        if let Some(item) = pick {
            self.palette = None;
            match item {
                PalItem::Session(id) => self.activate_session(id),
                PalItem::Connection(id) => {
                    self.selected = Some(id);
                    self.connect(id);
                }
                PalItem::Quick(q) => self.quick_connect(&q),
            }
        } else if close {
            self.palette = None;
        }
    }

    /// Tab in den Vordergrund holen (abgedockte: deren Fenster)
    fn activate_session(&mut self, id: u64) {
        if self.detached.iter().any(|(d, _)| *d == id) {
            self.ctx.send_viewport_cmd_to(detached_viewport(id), egui::ViewportCommand::Focus);
            return;
        }
        if self.split == Some(id) {
            self.split = self.active;
        }
        self.active = Some(id);
        if let Some(s) = self.sessions.iter().find(|s| s.id == id) {
            s.focus();
        }
    }

    /// Eingaben eines Terminals mit gleichzeitiger Eingabe an die anderen markierten Sitzungen weitergeben
    fn forward_mirror_input(&mut self) {
        let mut out: Vec<(u64, Vec<u8>)> = Vec::new();
        for s in &mut self.sessions {
            if let Kind::Ssh(t) = &mut s.kind
                && !t.outbox.is_empty()
            {
                out.push((s.id, std::mem::take(&mut t.outbox)));
            }
        }
        for (from, bytes) in out {
            for s in &mut self.sessions {
                if s.id != from
                    && *s.state() == State::Connected
                    && let Kind::Ssh(t) = &mut s.kind
                    && t.mirror
                {
                    t.send(&bytes);
                }
            }
        }
    }

    /// Abgedockte Sitzungen in eigenen Fenstern zeichnen
    fn detached_windows(&mut self, ctx: &egui::Context) {
        let list = self.detached.clone();
        for (id, title) in list {
            let Some(s) = self.sessions.iter().find(|s| s.id == id) else { continue };
            let (connected, proto, flags) = (*s.state() == State::Connected, s.proto(), self.menu_flags(s));
            let vid = detached_viewport(id);
            let mut builder = egui::ViewportBuilder::default().with_title(title.clone()).with_min_inner_size([400.0, 300.0]);
            if !self.detached_hwnd.contains_key(&id) {
                builder = builder.with_inner_size([1100.0, 750.0]);
            }
            let snippets = self.store.snippets.clone();
            ctx.show_viewport_immediate(vid, builder, |ui, _class| {
                let vctx = ui.ctx().clone();
                // Fensterhandle suchen (beim ersten Frame existiert das Fenster evtl. noch nicht)
                if !self.detached_hwnd.contains_key(&id) {
                    match rdp::find_own_window(&title) {
                        Some(h) => {
                            rdp::prepare_parent(h);
                            self.detached_hwnd.insert(id, h);
                            if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
                                s.set_parent(h);
                            }
                        }
                        None => vctx.request_repaint_after(Duration::from_millis(50)),
                    }
                }
                let hwnd = self.detached_hwnd.get(&id).copied();
                if let Some(h) = hwnd
                    && vctx.input(|i| i.pointer.any_pressed())
                {
                    rdp::focus_window(h);
                }
                if vctx.input(|i| i.viewport().close_requested()) {
                    self.session_cmd = Some((id, SessionCmd::Attach));
                }
                if self.lock.locked {
                    // gesperrt: Sitzung ausblenden, Fenster aber offen lassen (sonst würde das Control zerstört)
                    if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
                        s.hide();
                    }
                    egui::CentralPanel::default().show(ui, |ui| {
                        ui.centered_and_justified(|ui| ui.heading("🔒 RDP Manager ist gesperrt – im Hauptfenster entsperren"));
                    });
                    return;
                }
                let mut cmd = None;
                let mut close = None;
                egui::Panel::top(egui::Id::unique(("detached_bar", id))).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("⇤ Andocken").on_hover_text("Zurück ins Hauptfenster").clicked() {
                            cmd = Some((id, SessionCmd::Attach));
                        }
                        ui.menu_button("⌨ Tasten", |ui| {
                            if let Some(c) = session_menu(ui, connected, true, Some(proto), &snippets, flags) {
                                cmd = Some((id, c));
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("✖").on_hover_text("Verbindung trennen").clicked() {
                                close = Some(id);
                            }
                        });
                    });
                });
                let rect = egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(ui.visuals().extreme_bg_color))
                    .show(ui, |ui| {
                        let rect = ui.available_rect_before_wrap();
                        self.pane(ui, Some(id), rect, &mut close, &mut cmd);
                        rect
                    })
                    .inner;
                // eingebettetes RDP-/Web-Fenster über die Fläche legen
                let ppp = vctx.pixels_per_point();
                let mut need_thumb = false;
                if let (Some(_), Some(s)) = (hwnd, self.sessions.iter_mut().find(|s| s.id == id)) {
                    let px = (
                        (rect.min.x * ppp).round() as i32,
                        (rect.min.y * ppp).round() as i32,
                        (rect.width() * ppp).round() as i32,
                        (rect.height() * ppp).round() as i32,
                    );
                    let minimized = vctx.input(|i| i.viewport().minimized.unwrap_or(false));
                    if *s.state() == State::Connected && !minimized && px.2 > 0 && px.3 > 0 {
                        let cut = cutouts_for(&popup_rects(&vctx, ppp), px);
                        s.place(px.0, px.1, px.2, px.3, rdp::scale_percent(ppp));
                        s.set_cutouts(&cut);
                        if !cut.is_empty() {
                            vctx.request_repaint_after(Duration::from_millis(50));
                        }
                        need_thumb = self.thumbs.get(&id).is_none_or(|(_, t)| t.elapsed() > Duration::from_secs(5));
                    } else {
                        s.hide();
                    }
                }
                if need_thumb {
                    self.capture_thumb(id);
                }
                if close.is_some() {
                    self.close_session(id);
                } else if cmd.is_some() {
                    self.session_cmd = cmd;
                }
            });
        }
    }

    fn menu_flags(&self, s: &Session) -> MenuFlags {
        MenuFlags {
            logging: s.is_logging(),
            mirror: s.is_mirror(),
            sftp: self.sftp.contains_key(&s.id),
            detached: self.detached.iter().any(|(d, _)| *d == s.id),
            saved: s.conn.id != 0 && self.find_any(s.conn.id).is_some(),
            auto_resize: matches!(&s.kind, Kind::Rdp(r) if r.auto_resize()),
        }
    }

    /// Dateien aus dem Explorer auf ein SSH-Terminal gezogen → Upload-Dialog
    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<std::path::PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if dropped.is_empty() || !matches!(self.dialog, Dialog::None) {
            return;
        }
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        // auf einen SFTP-Dateibrowser gezogen → in dessen Ordner hochladen
        if let Some(p) = pointer
            && let Some((sid, b)) = self.sftp.iter().find(|(sid, b)| {
                b.rect.is_some_and(|r| r.contains(p))
                    && !self.detached.iter().any(|(d, _)| d == *sid)
                    && (self.active == Some(**sid) || self.split == Some(**sid))
            })
        {
            self.dialog = Dialog::Upload { session: *sid, files: dropped, remote_dir: b.cwd.clone() };
            return;
        }
        // Ziel: die Sitzung unter dem Mauszeiger (rechte Hälfte bei geteilter Ansicht), sonst die aktive
        let target = match (self.split, self.split_rect, pointer) {
            (Some(id), Some(r), Some(p)) if r.contains(p) => Some(id),
            _ => self.active,
        };
        let session = target.and_then(|id| self.sessions.iter().find(|s| s.id == id));
        match session.map(|s| &s.kind) {
            Some(Kind::Ssh(t)) => {
                self.dialog = Dialog::Upload { session: target.unwrap(), files: dropped, remote_dir: t.remote_dir.clone() };
            }
            _ => self.info = Some("Dateien können auf SSH-Tabs abgelegt werden (Upload per scp).".into()),
        }
    }

    fn start_web(&mut self, conn: &Connection) {
        if self.parent.is_invalid() {
            self.error = Some("Fensterhandle nicht verfügbar.".into());
            return;
        }
        let id = self.next_session_id;
        self.next_session_id += 1;
        match web::WebSession::start(id, conn, self.parent) {
            Ok(s) => self.add_session(Session::web(s)),
            Err(e) => self.error = Some(e),
        }
    }

    /// Neue Sitzung als aktiven Tab übernehmen und im Verbindungsprotokoll vermerken
    fn add_session(&mut self, s: Session) {
        if self.store.conn_log {
            let host = if s.conn.protocol.is_web() { s.conn.web_url() } else { s.conn.host.trim().to_string() };
            let lid = connlog::open(&s.title, &host, s.proto_label(), &login_of(&s.conn), "");
            self.log_ids.insert(s.id, (lid, 0));
        }
        self.active = Some(s.id);
        self.overview = false;
        self.sessions.push(s);
    }

    /// Extern gestartete Verbindung (mstsc, Konsole, Browser) im Protokoll vermerken
    fn log_external(&self, conn: &Connection, how: &str) {
        if self.store.conn_log {
            let host = if conn.protocol.is_web() { conn.web_url() } else { conn.host.trim().to_string() };
            connlog::open(conn.display_name(), &host, conn.protocol.label(), &login_of(conn), how);
        }
    }

    /// RDP in mstsc.exe öffnen (bei `multimon` über alle Monitore)
    fn launch_mstsc(&mut self, conn: &Connection, password: Option<&str>) {
        match rdp::launch_external(conn, password) {
            Ok(child) => {
                if conn.host.contains("127.0.0.1") {
                    self.ext_procs.push((child, conn.port)); // Tunnel offen halten, solange mstsc läuft
                }
                let how = if conn.multimon { "extern (mstsc, alle Monitore)" } else { "extern (mstsc)" };
                self.log_external(conn, how);
                if conn.multimon {
                    self.info = Some(format!("{} wird in mstsc über alle Monitore geöffnet (zurück: Verbindungsleiste oben)", conn.display_name()));
                }
            }
            Err(e) => self.error = Some(format!("mstsc konnte nicht gestartet werden: {e}")),
        }
    }

    /// Bild eines eingebetteten Fensters für die Übersicht aufnehmen
    fn capture_thumb(&mut self, id: u64) {
        let Some(h) = self.sessions.iter().find(|s| s.id == id && *s.state() == State::Connected).and_then(Session::hwnd) else {
            return;
        };
        if let Some((w, hh, px)) = rdp::capture(h, 480) {
            let img = egui::ColorImage::from_rgba_unmultiplied([w, hh], &px);
            match self.thumbs.get_mut(&id) {
                Some((tex, t)) => {
                    tex.set(img, egui::TextureOptions::LINEAR);
                    *t = Instant::now();
                }
                None => {
                    let tex = self.ctx.load_texture(format!("thumb_{id}"), img, egui::TextureOptions::LINEAR);
                    self.thumbs.insert(id, (tex, Instant::now()));
                }
            }
        }
    }

    fn start_vnc(&mut self, conn: &Connection, password: Option<String>) {
        let id = self.next_session_id;
        self.next_session_id += 1;
        self.add_session(Session::vnc(vnc::VncSession::start(id, conn, password, &self.ctx)));
    }

    fn start_ssh(&mut self, conn: &Connection, password: Option<String>) {
        let id = self.next_session_id;
        self.next_session_id += 1;
        match SshSession::start(id, conn, password, &self.ctx) {
            Ok(mut s) => {
                if self.store.ssh_auto_log {
                    if let Err(e) = s.start_log() {
                        self.error = Some(e);
                    }
                }
                self.add_session(Session::ssh(s));
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn start_session(&mut self, conn: &Connection, password: Option<&str>) {
        if self.parent.is_invalid() {
            self.error = Some("Fensterhandle nicht verfügbar.".into());
            return;
        }
        let id = self.next_session_id;
        self.next_session_id += 1;
        match RdpSession::start(id, conn, password, self.parent, self.content_px, self.scale) {
            Ok(s) => self.add_session(Session::rdp(s)),
            Err(e) => {
                rdp::log(&e);
                self.error = Some(e);
            }
        }
    }

    fn close_session(&mut self, id: u64) {
        if let Some((lid, _)) = self.log_ids.remove(&id) {
            connlog::event(&lid, "closed", "");
        }
        self.thumbs.remove(&id);
        self.sftp.remove(&id);
        self.detached.retain(|(d, _)| *d != id);
        self.detached_hwnd.remove(&id);
        if let Some(pos) = self.sessions.iter().position(|s| s.id == id) {
            let mut s = self.sessions.remove(pos);
            s.close();
            if self.split == Some(id) {
                self.split = None;
            } else if self.active == Some(id) && self.split.is_some() {
                // linke Sitzung geschlossen → rechte übernimmt
                self.active = self.split.take();
            } else if self.active == Some(id) {
                let docked = |s: &&Session| !self.detached.iter().any(|(d, _)| *d == s.id);
                self.active = self
                    .sessions
                    .iter()
                    .skip(pos)
                    .find(docked)
                    .or_else(|| self.sessions.iter().take(pos).rev().find(docked))
                    .map(|s| s.id);
            }
        }
    }

    /// Passwort einer gespeicherten Verbindung (bzw. ihres Ordners) im Windows-Tresor ablegen.
    fn save_session_password(&mut self, conn_id: u64, password: &str) {
        if conn_id == 0 {
            return; // Quick Connect – nicht gespeichert
        }
        let Some(conn) = self.find_any(conn_id).cloned() else { return };
        let (eff, cred, _) = self.resolve_any(&conn);
        if let Err(e) = rdp::store_password(cred, &eff.full_username(), password) {
            self.error = Some(e);
        }
    }

    /// Eigene Verbindungen (ohne Passwörter) als Datei für eine gemeinsame Liste speichern.
    fn export_shared(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Als gemeinsame Verbindungsliste speichern")
            .set_file_name("verbindungen.json")
            .add_filter("Verbindungsliste", &["json"])
            .save_file()
        else {
            return;
        };
        // Nur der Baum – Passwörter liegen ohnehin nicht in der Datei, sondern im Windows-Tresor
        let out = Store { root: self.store.root.clone(), ..Default::default() };
        match serde_json::to_string_pretty(&out).map_err(|e| e.to_string()).and_then(|j| std::fs::write(&path, j).map_err(|e| e.to_string())) {
            Ok(()) => self.info = Some(format!("Gemeinsame Liste gespeichert: {} (ohne Passwörter)", path.display())),
            Err(e) => self.error = Some(format!("Speichern fehlgeschlagen: {e}")),
        }
    }

    /// Baut die Passwortübersicht aus Baum und Windows-Tresor auf.
    fn open_passwords_dialog(&mut self) {
        let stored: std::collections::HashSet<u64> = rdp::stored_password_ids().into_iter().collect();
        let mut rows = Vec::new();
        let mut known = std::collections::HashSet::new();
        fn walk(
            root: &Folder,
            f: &Folder,
            prefix: &str,
            stored: &std::collections::HashSet<u64>,
            known: &mut std::collections::HashSet<u64>,
            rows: &mut Vec<PwRow>,
        ) {
            for sub in &f.folders {
                let path = if prefix.is_empty() { sub.name.clone() } else { format!("{prefix} / {}", sub.name) };
                known.insert(sub.id);
                if sub.has_credentials() || stored.contains(&sub.id) {
                    rows.push(PwRow {
                        id: sub.id,
                        is_folder: true,
                        path: path.clone(),
                        user: sub.full_username(),
                        stored: stored.contains(&sub.id),
                        inherits_from: None,
                    });
                }
                walk(root, sub, &path, stored, known, rows);
            }
            for c in &f.connections {
                known.insert(c.id);
                let (eff, cred) = root.resolve_credentials(c);
                let inherits_from = (cred != c.id).then(|| root.find_folder(cred).map(|f| f.name.clone()).unwrap_or_default());
                let path = if prefix.is_empty() { c.display_name().to_string() } else { format!("{prefix} / {}", c.display_name()) };
                rows.push(PwRow {
                    id: c.id,
                    is_folder: false,
                    path,
                    user: eff.full_username(),
                    stored: stored.contains(&c.id),
                    inherits_from,
                });
            }
        }
        walk(&self.store.root, &self.store.root, "", &stored, &mut known, &mut rows);
        let mut orphans: Vec<u64> = stored.iter().copied().filter(|id| !known.contains(id)).collect();
        orphans.sort_unstable();
        self.dialog =
            Dialog::Passwords { rows, orphans, only_stored: true, filter: String::new(), edit: None, confirm_all: false };
    }

    fn handle(&mut self, action: Action) {
        match action {
            Action::Connect(id) => self.connect(id),
            Action::RunTool(id, i) => {
                if let (Some(c), Some(tool)) = (self.find_any(id).cloned(), self.store.tools.get(i).cloned()) {
                    let (c, _, _) = self.resolve_any(&c);
                    if let Err(e) = tools::run(&tool, &c) {
                        self.error = Some(e);
                    }
                }
            }
            Action::ManageTools => self.dialog = Dialog::Tools { list: self.store.tools.clone() },
            Action::ReleaseNotes => self.dialog = Dialog::ReleaseNotes { since: self.store.last_seen_version.clone() },
            Action::ManageBackups => {
                self.dialog = Dialog::Backups { list: backup::list(&backup::backup_dir()), confirm: None }
            }
            Action::ManageShared => self.dialog = Dialog::Shared { list: self.store.shared_sources.clone() },
            Action::ReloadShared => {
                for t in &mut self.shared {
                    t.reload(true, &self.ctx);
                }
            }
            Action::ExportShared => self.export_shared(),
            Action::EditNotes(id) => {
                if let Some(c) = self.find_any(id) {
                    self.notes = Some((id, c.memo.clone(), false));
                }
            }
            Action::SshAgent => self.open_agent_dialog(),
            Action::OpenPalette => self.palette = Some(Palette::default()),
            Action::ConnectAllMonitors(id) => {
                // normaler Ablauf (Tresor, Tunnel …), aber am Ende mstsc über alle Monitore
                self.multimon_once.insert(id);
                self.connect(id);
            }
            Action::ConnectExternal(id) => {
                if let Some(c) = self.find_any(id).cloned() {
                    let (c, cred, known) = self.resolve_any(&c);
                    let r = match c.protocol {
                        Protocol::Ssh => ssh::launch_external(&c).map(|_| self.log_external(&c, "extern (Konsole)")),
                        Protocol::Rdp => {
                            let pw = known.or_else(|| rdp::read_password(cred));
                            self.launch_mstsc(&c, pw.as_deref());
                            Ok(())
                        }
                        Protocol::Http | Protocol::Https => {
                            tools::shell_open(&c.web_url(), "").map(|_| self.log_external(&c, "extern (Browser)"))
                        }
                        Protocol::Vnc => Err("Für VNC gibt es keinen externen Start – unter „Werkzeuge“ kann ein VNC-Viewer eingetragen werden.".into()),
                    };
                    if let Err(e) = r {
                        self.error = Some(e);
                    }
                }
            }
            Action::ShowConnLog => self.log_view = Some(connlog::Viewer::new()),
            Action::ToggleFavorite(id) => {
                if self.store.favorites.contains(&id) {
                    self.store.favorites.retain(|&f| f != id);
                } else {
                    self.store.favorites.push(id);
                }
                self.save();
            }
            Action::ClearRecent => {
                self.store.recent.clear();
                self.save();
            }
            Action::Wake(id, then_connect) => self.wake(id, then_connect),
            Action::Updates => {
                self.dialog = Dialog::Update { source: self.store.update_source.clone(), auto: self.store.update_check }
            }
            Action::LockNow => self.lock_now(),
            Action::Security => {
                let l = &self.store.lock;
                self.dialog = Dialog::Security {
                    current: String::new(),
                    new_pw: String::new(),
                    repeat: String::new(),
                    remove_pw: false,
                    hello: l.hello,
                    hello_available: lock::hello_available(),
                    at_start: l.at_start,
                    idle_minutes: l.idle_minutes,
                    error: None,
                };
            }
            Action::VaultSettings => {
                self.dialog = Dialog::Vault { settings: self.store.vault.clone(), test_entry: String::new(), result: None }
            }
            Action::Import(src) => self.import_from(src),
            Action::ToggleOverview => {
                if !self.overview {
                    // aktuelle Bilder der sichtbaren Sitzungen aufnehmen, bevor sie ausgeblendet werden
                    for id in self.active.into_iter().chain(self.split) {
                        self.capture_thumb(id);
                    }
                }
                self.overview = !self.overview;
            }
            Action::NewConnection(folder) => {
                // liegt der Ordner unter einem Ordner mit Anmeldedaten, diese standardmäßig übernehmen
                let inherit = self.store.root.credential_folder(folder).is_some();
                self.dialog = Dialog::Connection {
                    draft: Connection { inherit_credentials: inherit, ..Default::default() },
                    folder,
                    password: String::new(),
                    has_password: false,
                    delete_password: false,
                    is_new: true,
                };
            }
            Action::EditConnection(id) | Action::DuplicateConnection(id) => {
                let dup = matches!(action, Action::DuplicateConnection(_));
                let own = self.store.root.find_connection(id).is_some();
                if let Some(mut draft) = self.find_any(id).cloned() {
                    let folder = self.store.root.parent_of_connection(id).unwrap_or(ROOT_ID);
                    if dup && own {
                        draft.name = format!("{} (Kopie)", draft.display_name());
                    } else if !own {
                        // aus gemeinsamer Liste: Anmeldedaten mitnehmen, Ordner gibt es hier nicht
                        let (eff, _, _) = self.resolve_any(&draft);
                        draft = Connection { inherit_credentials: false, ..eff };
                    }
                    self.dialog = Dialog::Connection {
                        draft,
                        folder,
                        password: String::new(),
                        has_password: !dup && own && rdp::has_password(id),
                        delete_password: false,
                        is_new: dup || !own,
                    };
                }
            }
            Action::DeleteConnection(id) => {
                if let Some(c) = self.store.root.find_connection(id) {
                    self.dialog = Dialog::ConfirmDelete {
                        folder: None,
                        connection: Some(id),
                        label: format!("Verbindung \"{}\" löschen?", c.display_name()),
                    };
                }
            }
            Action::NewFolder(parent) => {
                self.dialog = Dialog::Folder {
                    id: None,
                    parent,
                    name: String::new(),
                    username: String::new(),
                    domain: String::new(),
                    password: String::new(),
                    has_password: false,
                    delete_password: false,
                };
            }
            Action::RenameFolder(id) => {
                if let Some(f) = self.store.root.find_folder(id) {
                    self.dialog = Dialog::Folder {
                        id: Some(id),
                        parent: ROOT_ID,
                        name: f.name.clone(),
                        username: f.username.clone(),
                        domain: f.domain.clone(),
                        password: String::new(),
                        has_password: rdp::has_password(id),
                        delete_password: false,
                    };
                }
            }
            Action::DeleteFolder(id) => {
                if let Some(f) = self.store.root.find_folder_mut(id) {
                    self.dialog = Dialog::ConfirmDelete {
                        folder: Some(id),
                        connection: None,
                        label: format!("Ordner \"{}\" inklusive aller enthaltenen Verbindungen löschen?", f.name),
                    };
                }
            }
            Action::ManagePasswords => self.open_passwords_dialog(),
            Action::ManageSnippets => self.dialog = Dialog::Snippets { list: self.store.snippets.clone() },
            Action::ConnectFolder(id, recursive) => {
                let folder = self
                    .store
                    .root
                    .find_folder(id)
                    .or_else(|| self.shared.iter().find_map(|t| t.root.as_ref()?.find_folder(id)));
                if let Some(f) = folder {
                    let mut ids = Vec::new();
                    collect_connections(f, recursive, &mut ids);
                    self.pending_connects.extend(ids);
                }
            }
            Action::Move(item, target) => {
                if self.store.root.move_item(item, target) {
                    self.save();
                    if let Item::Connection(id) = item {
                        self.selected = Some(id);
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------ UI

    fn menu_bar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("Datei", |ui| {
                if ui.button("Neue Verbindung…").clicked() {
                    actions.push(Action::NewConnection(self.selected_folder()));
                }
                if ui.button("Neuer Ordner…").clicked() {
                    actions.push(Action::NewFolder(ROOT_ID));
                }
                ui.separator();
                if ui.button("🔑 Gespeicherte Passwörter…").clicked() {
                    actions.push(Action::ManagePasswords);
                }
                if ui.button("📝 Befehls-Schnipsel…").clicked() {
                    actions.push(Action::ManageSnippets);
                }
                if ui.button("🔧 Externe Werkzeuge…").clicked() {
                    actions.push(Action::ManageTools);
                }
                if ui.button("🗝 SSH-Agent & Schlüssel…").clicked() {
                    actions.push(Action::SshAgent);
                }
                if ui.button("🔐 Passwort-Tresor (KeePass/Bitwarden)…").clicked() {
                    actions.push(Action::VaultSettings);
                }
                if self.vault.is_unlocked() && ui.button("🔐 Passwort-Tresor sperren").clicked() {
                    self.vault.lock();
                    self.info = Some("Passwort-Tresor gesperrt".into());
                }
                ui.separator();
                if ui.button("🔒 Sperre & Hauptkennwort…").clicked() {
                    actions.push(Action::Security);
                }
                if self.store.lock.enabled() && ui.button("🔒 Jetzt sperren").clicked() {
                    actions.push(Action::LockNow);
                }
                if ui.button("📜 Verbindungsprotokoll…").clicked() {
                    actions.push(Action::ShowConnLog);
                }
                ui.separator();
                if ui.button("🌐 Gemeinsame Verbindungslisten…").clicked() {
                    actions.push(Action::ManageShared);
                }
                if ui.add_enabled(!self.shared.is_empty(), egui::Button::new("🔄 Gemeinsame Listen neu laden")).clicked() {
                    actions.push(Action::ReloadShared);
                }
                if ui.button("📤 Eigene Verbindungen als gemeinsame Liste speichern…").clicked() {
                    actions.push(Action::ExportShared);
                }
                ui.separator();
                if ui.button("🕘 Sicherungen…").clicked() {
                    actions.push(Action::ManageBackups);
                }
                ui.separator();
                ui.menu_button("📥 Importieren", |ui| {
                    if ui.button("mRemoteNG (confCons.xml)…").clicked() {
                        actions.push(Action::Import(ImportSource::MRemoteNg));
                    }
                    if ui.button("Remote Desktop Connection Manager (.rdg)…").clicked() {
                        actions.push(Action::Import(ImportSource::RdcMan));
                    }
                    if ui.button("PuTTY-Sitzungen").on_hover_text("Liest die gespeicherten SSH-Sitzungen von PuTTY aus der Registrierung").clicked() {
                        actions.push(Action::Import(ImportSource::Putty));
                    }
                    if ui.button("OpenSSH-Konfiguration (~/.ssh/config)").clicked() {
                        actions.push(Action::Import(ImportSource::SshConfig));
                    }
                    if ui
                        .button("CSV (Royal TS u. a.)…")
                        .on_hover_text("Spalten werden am Namen erkannt: Name, Host/URI/ComputerName, Port, Protokoll/Type, Benutzer, Domäne, Passwort, Beschreibung, Ordner/Path")
                        .clicked()
                    {
                        actions.push(Action::Import(ImportSource::Csv));
                    }
                });
                if ui.button("📤 Exportieren (mRemoteNG-XML)…").clicked() {
                    if self.store.lock.enabled() {
                        // Export enthält Passwörter → erst entsperren
                        self.export_after_unlock = true;
                        actions.push(Action::LockNow);
                    } else {
                        self.dialog = Dialog::Export { use_master: false, master: String::new(), master_repeat: String::new() };
                    }
                }
                ui.separator();
                if ui.button("Datenordner öffnen").clicked() {
                    let _ = std::process::Command::new("explorer.exe").arg(Store::data_dir()).spawn();
                }
                ui.separator();
                if ui.button("Beenden").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Ansicht", |ui| {
                ui.label(RichText::new("Farbmodus").strong());
                let before = self.store.theme;
                ui.radio_value(&mut self.store.theme, ThemeMode::System, "Windows-Einstellung");
                ui.radio_value(&mut self.store.theme, ThemeMode::Light, "Hell");
                ui.radio_value(&mut self.store.theme, ThemeMode::Dark, "Dunkel");
                if before != self.store.theme {
                    apply_theme(ui.ctx(), self.store.theme);
                    self.save();
                }
                ui.separator();
                ui.label(RichText::new("SSH-Terminal").strong());
                let r = ui.add(egui::Slider::new(&mut self.store.terminal_font_size, 9.0..=24.0).step_by(1.0).text("Schriftgröße"));
                if r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged()) {
                    self.save();
                }
                if ui.checkbox(&mut self.store.ssh_auto_log, "Jede SSH-Sitzung mitschneiden (Logdatei)").changed() {
                    self.save();
                }
                if self.transfers.is_empty() {
                    ui.add_enabled(false, egui::Button::new("⇅ Übertragungen anzeigen"));
                } else if ui.button(format!("⇅ Übertragungen anzeigen ({})", self.transfers.len())).clicked() {
                    self.show_transfers = true;
                }
                ui.separator();
                ui.label(RichText::new("Verbindungsbaum").strong());
                if ui.checkbox(&mut self.store.check_reachability, "Erreichbarkeit anzeigen").changed() {
                    self.save();
                }
                if ui
                    .add_enabled(self.store.check_reachability, egui::Button::new("🔄 Erreichbarkeit neu prüfen"))
                    .clicked()
                {
                    self.reach.clear();
                }
                ui.separator();
                ui.label(RichText::new("Sitzungen").strong());
                if ui
                    .checkbox(&mut self.store.restore_tabs, "Offene Tabs beim nächsten Start wieder öffnen")
                    .on_hover_text("Gespeicherte Verbindungen, die beim Beenden offen sind, werden beim nächsten Start nacheinander wieder verbunden")
                    .changed()
                {
                    self.save();
                }
                if ui
                    .add_enabled(!self.sessions.is_empty(), egui::Button::selectable(self.overview, "▦ Übersicht aller Sitzungen (Strg+Umschalt+A)"))
                    .clicked()
                {
                    actions.push(Action::ToggleOverview);
                }
                if ui.add_enabled(self.split.is_some(), egui::Button::new("Teilung aufheben")).clicked() {
                    self.split = None;
                }
            });
            let active_connected = self.active_session().is_some_and(|s| *s.state() == State::Connected);
            let active_proto = self.active_session().map(Session::proto);
            let active_flags = self.active_session().map(|s| self.menu_flags(s)).unwrap_or_default();
            ui.menu_button("Sitzung", |ui| {
                if self.active.is_none() {
                    ui.weak("Keine Sitzung geöffnet");
                }
                if let Some(cmd) =
                    session_menu(ui, active_connected, self.active.is_some(), active_proto, &self.store.snippets, active_flags)
                {
                    self.session_cmd = self.active.map(|id| (id, cmd));
                }
            });

            ui.menu_button("Hilfe", |ui| {
                if ui.button("📋 Versionshinweise…").clicked() {
                    actions.push(Action::ReleaseNotes);
                }
                if ui.button("🔄 Nach Updates suchen…").clicked() {
                    actions.push(Action::Updates);
                }
                if ui.button("📂 Datenordner öffnen").clicked() {
                    let _ = std::process::Command::new("explorer.exe").arg(Store::data_dir()).spawn();
                }
                ui.separator();
                ui.weak(concat!("RDP Manager ", env!("CARGO_PKG_VERSION")));
            });

            // --- Schnellsuche
            ui.separator();
            if ui.button("🔍 Suchen").on_hover_text("Schnellsuche: Verbindung oder Tab finden und öffnen (Strg+P)").clicked() {
                actions.push(Action::OpenPalette);
            }
            if !self.terminal_focused
                && self.palette.is_none()
                && matches!(self.dialog, Dialog::None)
                && ui.input(|i| i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::P))
            {
                actions.push(Action::OpenPalette);
            }
            if !self.terminal_focused
                && !self.sessions.is_empty()
                && matches!(self.dialog, Dialog::None)
                && ui.input(|i| i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::A))
            {
                actions.push(Action::ToggleOverview);
            }

            // --- Quick Connect
            let r = ui
                .add(
                    egui::TextEdit::singleline(&mut self.quick)
                        .hint_text("⚡ Quick Connect: [ssh://][benutzer@]host[:port]")
                        .desired_width(300.0),
                )
                .on_hover_text(
                    "Enter verbindet sofort (Strg+K springt hierher).\nRDP: server01, server01:3390, corp\\jdoe@server01\nSSH: ssh://root@linux01, ssh admin@linux01:2222, linux01:22",
                );
            if !self.terminal_focused && ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::K)) {
                r.request_focus();
            }
            let mut go = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("▶").on_hover_text("Verbinden").clicked() {
                go = true;
            }
            ui.add_enabled_ui(!self.store.quick_history.is_empty(), |ui| {
                ui.menu_button("🕘", |ui| {
                    let mut pick = None;
                    for h in &self.store.quick_history {
                        if ui.button(h).clicked() {
                            pick = Some(h.clone());
                        }
                    }
                    ui.separator();
                    if ui.button("Verlauf löschen").clicked() {
                        self.store.quick_history.clear();
                        self.save();
                    }
                    if let Some(h) = pick {
                        self.quick = h;
                        self.quick_go = true;
                    }
                })
                .response
                .on_hover_text("Zuletzt verwendet");
            });
            if go {
                self.quick_go = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.store.lock.enabled() && ui.button("🔒").on_hover_text("Programm sperren").clicked() {
                    actions.push(Action::LockNow);
                }
                let (icon, next, tip) = match self.store.theme {
                    ThemeMode::System => ("💻", ThemeMode::Light, "Farbmodus: Windows-Einstellung"),
                    ThemeMode::Light => ("☀", ThemeMode::Dark, "Farbmodus: Hell"),
                    ThemeMode::Dark => ("🌙", ThemeMode::System, "Farbmodus: Dunkel"),
                };
                if ui.button(icon).on_hover_text(tip).clicked() {
                    self.store.theme = next;
                    apply_theme(ui.ctx(), next);
                    self.save();
                }
            });
        });
    }

    /// Einstellungen aus dem Dialog „Sperre & Hauptkennwort“ übernehmen
    fn save_security(&mut self) {
        let Dialog::Security { current, new_pw, repeat, remove_pw, hello, at_start, idle_minutes, error, .. } = &mut self.dialog
        else {
            return;
        };
        let old = &self.store.lock;
        if !old.password_hash.is_empty() && !lock::verify_password(current, &old.password_hash) {
            *error = Some("Das aktuelle Kennwort ist falsch.".into());
            return;
        }
        let mut hash = old.password_hash.clone();
        if *remove_pw {
            hash.clear();
        } else if !new_pw.is_empty() {
            if new_pw != repeat {
                *error = Some("Die beiden Kennwörter stimmen nicht überein.".into());
                return;
            }
            if new_pw.chars().count() < 4 {
                *error = Some("Das Kennwort muss mindestens 4 Zeichen haben.".into());
                return;
            }
            hash = lock::hash_password(new_pw);
        }
        let new = model::LockSettings { password_hash: hash, hello: *hello, at_start: *at_start, idle_minutes: *idle_minutes };
        self.dialog = Dialog::None;
        self.info = Some(
            if new.enabled() {
                "Sperre eingerichtet – sperren über 🔒 oben rechts oder Datei > Jetzt sperren"
            } else {
                "Sperre ausgeschaltet"
            }
            .into(),
        );
        self.store.lock = new;
        self.save();
    }

    /// Programm sperren (Sitzungen bleiben verbunden, werden aber ausgeblendet)
    fn lock_now(&mut self) {
        if !self.store.lock.enabled() {
            self.handle(Action::Security);
            return;
        }
        self.lock.lock(&self.store.lock);
        self.vault.lock();
        self.palette = None;
        self.overview = false;
    }

    /// Import aus anderen Programmen
    fn import_from(&mut self, src: ImportSource) {
        let pick = |title: &str, name: &str, ext: &[&str]| -> Option<std::path::PathBuf> {
            rfd::FileDialog::new().set_title(title).add_filter(name, ext).add_filter("Alle Dateien", &["*"]).pick_file()
        };
        let read = |p: &std::path::Path| -> Result<String, String> {
            std::fs::read(p)
                .map(|b| String::from_utf8_lossy(&b).trim_start_matches('\u{feff}').to_string())
                .map_err(|e| format!("Datei konnte nicht gelesen werden: {e}"))
        };
        let result = match src {
            ImportSource::MRemoteNg => return self.pick_import_file(),
            ImportSource::RdcMan => {
                let Some(p) = pick("RDCMan-Datei importieren", "Remote Desktop Connection Manager", &["rdg"]) else { return };
                let store = &mut self.store;
                read(&p).and_then(|t| importers::rdcman(&t, &mut || store.new_id()))
            }
            ImportSource::Putty => {
                let store = &mut self.store;
                importers::read_putty_sessions().map(|list| importers::putty(&list, &mut || store.new_id()))
            }
            ImportSource::SshConfig => {
                let mut path = importers::ssh_config_path();
                if !path.exists() {
                    match pick("OpenSSH-Konfiguration wählen", "SSH-Konfiguration", &["*"]) {
                        Some(p) => path = p,
                        None => return,
                    }
                }
                let store = &mut self.store;
                read(&path).map(|t| importers::ssh_config(&t, &mut || store.new_id()))
            }
            ImportSource::Csv => {
                let Some(p) = pick("CSV-Datei importieren (z. B. Royal TS)", "CSV", &["csv", "txt"]) else { return };
                let store = &mut self.store;
                read(&p).and_then(|t| importers::csv(&t, &mut || store.new_id()))
            }
        };
        match result {
            Ok(imp) => self.finish_import(imp),
            Err(e) => self.error = Some(e),
        }
    }

    fn finish_import(&mut self, imp: importers::Imported) {
        if imp.connections == 0 {
            let mut msg = String::from("Keine passenden Verbindungen gefunden.");
            if imp.skipped > 0 {
                msg.push_str(&format!(" {} Einträge mit anderem Protokoll übersprungen.", imp.skipped));
            }
            self.error = Some(msg);
            return;
        }
        let mut root = imp.root;
        root.name = format!("Import {}", root.name);
        let folder_name = root.name.clone();
        let user_of = |id: u64| {
            root.find_connection(id).map(|c| c.full_username()).or_else(|| root.find_folder(id).map(|f| f.full_username()))
        };
        let mut pw_ok = 0;
        for (id, pw) in &imp.passwords {
            if rdp::store_password(*id, &user_of(*id).unwrap_or_default(), pw).is_ok() {
                pw_ok += 1;
            }
        }
        self.store.root.folders.push(root);
        self.save();
        let mut msg = format!("{} Verbindung(en) in den Ordner „{folder_name}“ importiert", imp.connections);
        if pw_ok > 0 {
            msg.push_str(&format!(", {pw_ok} Passwort/Passwörter übernommen"));
        }
        if imp.skipped > 0 {
            msg.push_str(&format!(", {} übersprungen (anderes Protokoll)", imp.skipped));
        }
        for n in &imp.notes {
            msg.push_str(&format!(". {n}"));
        }
        self.info = Some(msg);
    }

    fn pick_import_file(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("mRemoteNG-Verbindungsdatei importieren")
            .add_filter("mRemoteNG XML", &["xml"])
            .add_filter("Alle Dateien", &["*"])
            .pick_file()
        else {
            return;
        };
        let xml = match std::fs::read(&path) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').to_string(),
            Err(e) => {
                self.error = Some(format!("Datei konnte nicht gelesen werden: {e}"));
                return;
            }
        };
        match mremote::needs_password(&xml) {
            Ok(needs_password) => {
                self.dialog = Dialog::Import { path, xml, needs_password, password: String::new(), replace: false, error: None }
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn do_export(&mut self, master: Option<String>) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Verbindungen exportieren")
            .add_filter("mRemoteNG XML", &["xml"])
            .set_file_name("confCons.xml")
            .save_file()
        else {
            return;
        };
        // Vom Ordner übernommene Anmeldedaten in die Verbindungen schreiben (mRemoteNG kennt das so nicht)
        let mut tree = self.store.root.clone();
        let mut cred_of = std::collections::HashMap::new();
        resolve_tree(&self.store.root, &mut tree, &mut cred_of);
        let password_of = |id: u64| rdp::read_password(*cred_of.get(&id).unwrap_or(&id));
        let xml = mremote::export(&tree, master.as_deref(), &password_of);
        match std::fs::write(&path, xml) {
            Ok(()) => self.info = Some(format!("Exportiert nach {}", path.display())),
            Err(e) => self.error = Some(format!("Export fehlgeschlagen: {e}")),
        }
    }

    fn run_import(&mut self) {
        let Dialog::Import { path, xml, password, replace, error, .. } = &mut self.dialog else { return };
        let pw = (!password.is_empty()).then(|| password.clone());
        let store = &mut self.store;
        let result = mremote::import(xml, pw.as_deref(), &mut || store.new_id());
        let imported = match result {
            Ok(i) => i,
            Err(mremote::ImportError::NeedsPassword) => {
                *error = Some(if pw.is_some() { "Master-Passwort ist falsch." } else { "Master-Passwort erforderlich." }.into());
                if let Dialog::Import { needs_password, .. } = &mut self.dialog {
                    *needs_password = true;
                }
                return;
            }
            Err(mremote::ImportError::Other(e)) => {
                *error = Some(e);
                return;
            }
        };
        let replace = *replace;
        let folder_name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Import".into());
        self.dialog = Dialog::None;

        let mut imported_root = imported.root;
        if replace {
            // Stand vorher sichern. Alte Passwörter bleiben im Tresor, damit eine Wiederherstellung
            // sie wiederfindet (aufräumen über „Gespeicherte Passwörter“ → verwaiste Einträge).
            self.backup_now(Some("vor Import"));
            self.store.root.folders = std::mem::take(&mut imported_root.folders);
            self.store.root.connections = std::mem::take(&mut imported_root.connections);
        } else {
            imported_root.name = format!("Import {folder_name}");
            self.store.root.folders.push(imported_root);
        }
        let mut pw_errors = 0;
        for (id, pw) in &imported.passwords {
            let user = self.store.root.find_connection(*id).map(|c| c.full_username()).unwrap_or_default();
            if rdp::store_password(*id, &user, pw).is_err() {
                pw_errors += 1;
            }
        }
        self.save();
        let mut msg = format!(
            "{} Verbindung(en) importiert, {} Passwort/Passwörter übernommen",
            imported.connections,
            imported.passwords.len() - pw_errors
        );
        if imported.skipped > 0 {
            msg.push_str(&format!(", {} Nicht-RDP-Verbindung(en) übersprungen", imported.skipped));
        }
        self.info = Some(msg);
    }

    /// Ordner, in den neue Verbindungen kommen: der Ordner der aktuell gewählten Verbindung.
    fn selected_folder(&self) -> u64 {
        self.selected.and_then(|id| self.store.root.parent_of_connection(id)).unwrap_or(ROOT_ID)
    }

    fn tree_panel(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("➕ Verbindung").on_hover_text("Neue Verbindung").clicked() {
                actions.push(Action::NewConnection(self.selected_folder()));
            }
            if ui.button("📁 Ordner").on_hover_text("Neuer Ordner").clicked() {
                actions.push(Action::NewFolder(ROOT_ID));
            }
        });
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("🔍 Filtern… (auch #tag)").desired_width(f32::INFINITY))
            .on_hover_text("Filtert nach Name, Host, Benutzer, Beschreibung, Notizen, Tags und Ordner.\n#prod zeigt nur Verbindungen mit dem Tag „prod“.\nSchnellsuche zum Öffnen: Strg+P");
        // --- Tag-Filter
        let mut roots: Vec<&Folder> = vec![&self.store.root];
        roots.extend(self.shared.iter().filter_map(|t| t.root.as_ref()));
        let tags = all_tags(&roots);
        self.tag_filter.retain(|t| tags.contains(t));
        if !tags.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                for t in &tags {
                    let on = self.tag_filter.contains(t);
                    if ui.selectable_label(on, format!("#{t}")).on_hover_text("Nur Verbindungen mit diesem Tag zeigen").clicked() {
                        if on {
                            self.tag_filter.remove(t);
                        } else {
                            self.tag_filter.insert(t.clone());
                        }
                    }
                }
                if !self.tag_filter.is_empty() && ui.small_button("✖").on_hover_text("Tag-Filter aufheben").clicked() {
                    self.tag_filter.clear();
                }
            });
        }
        ui.separator();

        let terms = model::search_terms(&self.filter);
        let filter = self.filter.trim().to_lowercase();
        let tag_filter = self.tag_filter.clone();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let root = &self.store.root;
            if root.folders.is_empty() && root.connections.is_empty() {
                ui.weak("Noch keine Verbindungen.\nÜber „➕ Verbindung“ anlegen.");
            }
            let mut paths = vec![(ROOT_ID, "(Oberste Ebene)".to_string())];
            root.folder_paths("", &mut paths);
            let reach = self.store.check_reachability.then_some(&self.reach);
            let tools = &self.store.tools;
            let favorites = &self.store.favorites;
            let tree = Tree { root, paths: &paths, terms: &terms, tags: &tag_filter, reach, read_only: false, tools, favorites };

            // --- Favoriten und zuletzt verwendet (nicht beim Filtern)
            if !tree.filtering() {
                let mut roots: Vec<&Folder> = vec![root];
                roots.extend(self.shared.iter().filter_map(|t| t.root.as_ref()));
                let empty: Vec<(u64, String)> = Vec::new();
                let lists = [("⭐ Favoriten", "quick_fav", &self.store.favorites, false), ("🕘 Zuletzt verwendet", "quick_recent", &self.store.recent, true)];
                let mut any = false;
                for (title, salt, ids, is_recent) in lists {
                    let found: Vec<_> = ids.iter().filter_map(|&id| locate_connection(&roots, id)).collect();
                    if found.is_empty() {
                        continue;
                    }
                    any = true;
                    let resp = CollapsingHeader::new(RichText::new(title).strong()).id_salt(salt).default_open(true).show(ui, |ui| {
                        for (r, parent, c) in &found {
                            ui.push_id((salt, c.id), |ui| {
                                if std::ptr::eq(*r, root) {
                                    tree.connection_row(ui, parent, c, &mut self.selected, actions);
                                } else {
                                    let t = Tree { root: r, paths: &empty, read_only: true, ..tree };
                                    t.connection_row(ui, parent, c, &mut self.selected, actions);
                                }
                            });
                        }
                    });
                    resp.header_response.context_menu(|ui| {
                        if is_recent {
                            if ui.button("Liste leeren").clicked() {
                                actions.push(Action::ClearRecent);
                            }
                        } else {
                            ui.weak("Favoriten per Rechtsklick auf eine Verbindung");
                        }
                    });
                }
                if any {
                    ui.separator();
                }
            }

            tree.contents(ui, root, "", &mut self.selected, actions);

            // --- gemeinsame Listen (schreibgeschützt)
            for (i, t) in self.shared.iter().enumerate() {
                let label = format!("🌐 {}{}", t.name, if t.is_loading() { " …" } else { "" });
                let mut header = CollapsingHeader::new(RichText::new(label).strong()).id_salt(("shared", i)).default_open(true);
                if !filter.is_empty() || !tag_filter.is_empty() {
                    header = header.open(Some(true));
                }
                let resp = header.show(ui, |ui| {
                    if let Some(e) = &t.error {
                        ui.colored_label(Color32::from_rgb(220, 70, 70), format!("⚠ {e}")).on_hover_text(&t.path);
                    }
                    match &t.root {
                        Some(root) => {
                            let empty: Vec<(u64, String)> = Vec::new();
                            let tree = Tree {
                                root,
                                paths: &empty,
                                terms: &terms,
                                tags: &tag_filter,
                                reach,
                                read_only: true,
                                tools,
                                favorites,
                            };
                            tree.contents(ui, root, "", &mut self.selected, actions);
                        }
                        None if t.error.is_none() => {
                            ui.weak("wird geladen …");
                        }
                        None => {}
                    }
                });
                resp.header_response
                    .on_hover_text(format!("Gemeinsame Liste (schreibgeschützt)\n{}", t.path))
                    .context_menu(|ui| {
                        if ui.button("🔄 Neu laden").clicked() {
                            actions.push(Action::ReloadShared);
                        }
                        if ui.button("⚙ Gemeinsame Listen verwalten…").clicked() {
                            actions.push(Action::ManageShared);
                        }
                    });
            }

            let rest = ui.allocate_response(ui.available_size().max(egui::vec2(0.0, 40.0)), egui::Sense::click());
            // Ablegen im freien Bereich → oberste Ebene
            tree.drop_target(ui, &rest, ROOT_ID, actions);
            rest.context_menu(|ui| {
                if ui.button("➕ Neue Verbindung").clicked() {
                    actions.push(Action::NewConnection(ROOT_ID));
                }
                if ui.button("📁 Neuer Ordner").clicked() {
                    actions.push(Action::NewFolder(ROOT_ID));
                }
            });
        });
    }

    fn tabs_and_content(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let mut close = None;
        let mut cmd: Option<(u64, SessionCmd)> = None;
        // ⌨-Knopf rechts in der Tableiste für die aktive Sitzung
        let active_state =
            self.active_session().map(|s| (s.id, *s.state() == State::Connected, s.proto(), self.menu_flags(s)));
        let snippets = self.store.snippets.clone();
        let mut key_cmd = None;
        let mut toggle_overview = false;
        let (has_sessions, overview) = (!self.sessions.is_empty(), self.overview);
        egui::Sides::new().show(
            ui,
            |ui| self.tab_strip(ui, &mut close, &mut cmd),
            |ui| {
                if has_sessions
                    && ui
                        .selectable_label(overview, "▦")
                        .on_hover_text("Übersicht aller Sitzungen (Strg+Umschalt+A)")
                        .clicked()
                {
                    toggle_overview = true;
                }
                if let Some((id, connected, proto, flags)) = active_state.filter(|_| !overview) {
                    ui.menu_button("⌨ Tasten", |ui| {
                        if let Some(c) = session_menu(ui, connected, true, Some(proto), &snippets, flags) {
                            key_cmd = Some((id, c));
                        }
                    })
                    .response
                    .on_hover_text("Tastenkombinationen & Sitzungsfunktionen");
                }
            },
        );
        if key_cmd.is_some() {
            cmd = key_cmd;
        }
        if toggle_overview {
            self.handle(Action::ToggleOverview);
        }

        let rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
        self.terminal_focused = false;

        if self.overview && !self.sessions.is_empty() {
            self.overview_ui(ui, rect, &mut close);
            ui.advance_cursor_after_rect(rect);
            if let Some(id) = close {
                self.close_session(id);
            }
            return rect;
        }
        self.overview = false;

        // --- Geteilte Ansicht: aktive Sitzung links, self.split rechts
        let split_valid = self.split.is_some_and(|id| Some(id) != self.active && self.sessions.iter().any(|s| s.id == id));
        if !split_valid {
            self.split = None;
        }
        let (left, right) = if self.split.is_some() {
            const GAP: f32 = 6.0;
            let x = rect.min.x + rect.width() * self.split_ratio;
            let divider = egui::Rect::from_min_max(egui::pos2(x - GAP / 2.0, rect.min.y), egui::pos2(x + GAP / 2.0, rect.max.y));
            let resp = ui.interact(divider, ui.scope_id().with("split_divider"), egui::Sense::drag());
            if resp.hovered() || resp.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            if resp.dragged()
                && let Some(p) = resp.interact_pointer_pos()
            {
                self.split_ratio = ((p.x - rect.min.x) / rect.width()).clamp(0.15, 0.85);
            }
            if resp.double_clicked() {
                self.split_ratio = 0.5;
            }
            let color = if resp.hovered() || resp.dragged() {
                ui.visuals().selection.stroke.color
            } else {
                ui.visuals().widgets.noninteractive.bg_stroke.color
            };
            ui.painter().rect_filled(divider, 0.0, ui.visuals().panel_fill);
            ui.painter().vline(x, rect.y_range(), egui::Stroke::new(2.0, color));
            let left = egui::Rect::from_min_max(rect.min, egui::pos2(divider.min.x, rect.max.y));
            let right = egui::Rect::from_min_max(egui::pos2(divider.max.x, rect.min.y), rect.max);
            (left, Some(right))
        } else {
            (rect, None)
        };
        self.split_rect = right;

        let active = self.active;
        self.pane(ui, active, left, &mut close, &mut cmd);
        if let Some(r) = right {
            let split = self.split;
            self.pane(ui, split, r, &mut close, &mut cmd);
        }
        ui.advance_cursor_after_rect(rect);

        if let Some(id) = close {
            self.close_session(id);
        }
        if cmd.is_some() {
            self.session_cmd = cmd;
        }
        left
    }

    /// Übersicht: alle Sitzungen als Kacheln mit Vorschaubild
    fn overview_ui(&mut self, ui: &mut egui::Ui, rect: egui::Rect, close: &mut Option<u64>) {
        const TILE: egui::Vec2 = egui::vec2(300.0, 228.0);
        const IMG_H: f32 = 168.0;
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.overview = false;
        }
        let mut activate = None;
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(12.0)), |ui| {
            ui.horizontal(|ui| {
                ui.heading("▦ Alle Sitzungen");
                ui.weak("Klick öffnet die Sitzung · Mittelklick trennt · Esc schließt die Übersicht");
            });
            ui.add_space(8.0);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
                    for s in &self.sessions {
                        let (r, resp) = ui.allocate_exact_size(TILE, egui::Sense::click());
                        let v = ui.visuals();
                        let hovered = resp.hovered();
                        let p = ui.painter();
                        p.rect_filled(r, 6.0, v.panel_fill);
                        p.rect_stroke(
                            r,
                            6.0,
                            if hovered || self.active == Some(s.id) {
                                egui::Stroke::new(2.0, v.selection.stroke.color)
                            } else {
                                v.widgets.noninteractive.bg_stroke
                            },
                            egui::StrokeKind::Inside,
                        );
                        if let Some(c) = s.color() {
                            p.rect_filled(
                                egui::Rect::from_min_max(r.min, egui::pos2(r.max.x, r.min.y + 4.0)),
                                egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 },
                                c,
                            );
                        }
                        let img = egui::Rect::from_min_size(r.min + egui::vec2(6.0, 8.0), egui::vec2(TILE.x - 12.0, IMG_H));
                        p.rect_filled(img, 3.0, v.extreme_bg_color);
                        // Vorschau
                        let tex = match &s.kind {
                            Kind::Vnc(vs) => vs.texture().map(|t| (t.id(), t.size_vec2(), None)),
                            _ => self.thumbs.get(&s.id).map(|(t, at)| (t.id(), t.size_vec2(), Some(at.elapsed().as_secs()))),
                        };
                        let mut age = None;
                        if let Kind::Ssh(t) = &s.kind {
                            let text = t.screen_text();
                            let lines: Vec<&str> = text.lines().collect();
                            let end = lines.iter().rposition(|l| !l.trim().is_empty()).map(|i| i + 1).unwrap_or(0);
                            let start = end.saturating_sub(14);
                            let shown: String = lines[start..end].iter().map(|l| l.chars().take(70).collect::<String>()).collect::<Vec<_>>().join("\n");
                            let painter = p.with_clip_rect(img.shrink(4.0));
                            painter.text(
                                img.min + egui::vec2(5.0, 4.0),
                                egui::Align2::LEFT_TOP,
                                shown,
                                egui::FontId::monospace(8.5),
                                v.text_color(),
                            );
                        } else if let Some((id, size, a)) = tex {
                            age = a;
                            let scale = (img.width() / size.x).min(img.height() / size.y);
                            let fit = egui::Rect::from_center_size(img.center(), size * scale);
                            p.image(id, fit, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                        } else {
                            let msg = match s.state() {
                                State::Connected => format!("{}\nVorschau erscheint, sobald der Tab einmal angezeigt wurde", s.icon),
                                State::Connecting => "⏳ verbindet …".to_string(),
                                _ => "⛔ getrennt".to_string(),
                            };
                            p.text(img.center(), egui::Align2::CENTER_CENTER, msg, egui::FontId::proportional(13.0), v.weak_text_color());
                        }
                        // Titelzeile
                        let state = match s.state() {
                            State::Connecting => "⏳".to_string(),
                            State::Connected => connlog::duration_text(s.seconds_running()),
                            State::Failed(_) | State::Closed => "⛔ getrennt".to_string(),
                        };
                        let detached = self.detached.iter().any(|(d, _)| *d == s.id);
                        let title: String = s.title.chars().take(34).collect();
                        p.text(
                            egui::pos2(r.min.x + 8.0, img.max.y + 8.0),
                            egui::Align2::LEFT_TOP,
                            format!("{} {title}", s.icon),
                            egui::FontId::proportional(14.0),
                            v.strong_text_color(),
                        );
                        p.text(
                            egui::pos2(r.min.x + 8.0, img.max.y + 28.0),
                            egui::Align2::LEFT_TOP,
                            format!("{} · {}{}{}", s.host, state, if detached { " · ⧉ eigenes Fenster" } else { "" }, if s.is_mirror() { " · 📡" } else { "" }),
                            egui::FontId::proportional(11.5),
                            v.weak_text_color(),
                        );
                        let resp = match age {
                            Some(a) if a > 10 => resp.on_hover_text(format!("Vorschau von vor {}", connlog::duration_text(a))),
                            _ => resp,
                        };
                        if resp.clicked() {
                            activate = Some(s.id);
                        }
                        if resp.middle_clicked() {
                            *close = Some(s.id);
                        }
                        resp.context_menu(|ui| {
                            if ui.button("▶ Anzeigen").clicked() {
                                activate = Some(s.id);
                            }
                            if ui.button("✖ Trennen / Tab schließen").clicked() {
                                *close = Some(s.id);
                            }
                        });
                    }
                });
            });
        });
        if let Some(id) = activate {
            self.overview = false;
            self.activate_session(id);
        }
        // Laufzeiten und SSH-Vorschau aktuell halten
        ui.ctx().request_repaint_after(Duration::from_secs(1));
    }

    /// Zeichnet eine Sitzung (SSH-Terminal oder RDP-Platzhalter/Status) in `rect`.
    /// Das RDP-Fenster selbst wird in `layout_sessions` über diesen Bereich gelegt.
    fn pane(
        &mut self,
        ui: &mut egui::Ui,
        sid: Option<u64>,
        rect: egui::Rect,
        close: &mut Option<u64>,
        cmd: &mut Option<(u64, SessionCmd)>,
    ) {
        let font_size = self.store.terminal_font_size;
        let dialog_open = !matches!(self.dialog, Dialog::None);
        let mut rect = rect;
        let mut save_vnc_pw: Option<(u64, String)> = None;
        if let Some(s) = sid.and_then(|id| self.sessions.iter_mut().find(|s| s.id == id)) {
            let grab = !dialog_open && s.grab_focus.replace(false);
            let (sid, failed) = (s.id, if let State::Failed(m) = s.state() { Some(m.clone()) } else { None });

            // --- Web: Adressleiste oben, darunter liegt das WebView2-Fenster
            if let Kind::Web(w) = &s.kind {
                let bar = egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rect.min.y + WEB_TOOLBAR));
                rect = egui::Rect::from_min_max(egui::pos2(rect.min.x, bar.max.y), rect.max);
                let edit = self.web_url_edit.entry(sid).or_default();
                ui.scope_builder(egui::UiBuilder::new().max_rect(bar.shrink2(egui::vec2(6.0, 3.0))), |ui| {
                    ui.horizontal_centered(|ui| {
                        if ui.add_enabled(w.can_go_back(), egui::Button::new("⬅")).on_hover_text("Zurück").clicked() {
                            w.go_back();
                        }
                        if ui.add_enabled(w.can_go_forward(), egui::Button::new("➡")).on_hover_text("Vorwärts").clicked() {
                            w.go_forward();
                        }
                        if ui.button("🔄").on_hover_text("Neu laden").clicked() {
                            w.reload();
                        }
                        let id = egui::Id::unique(("web_url", sid));
                        let focused = ui.memory(|m| m.has_focus(id));
                        if !focused {
                            *edit = w.url();
                        }
                        let r = ui.add(
                            egui::TextEdit::singleline(edit).id(id).desired_width((ui.available_width() - 40.0).max(80.0)),
                        );
                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            w.navigate(edit.trim());
                        }
                        if ui.button("↗").on_hover_text("Im Standardbrowser öffnen").clicked() {
                            *cmd = Some((sid, SessionCmd::WebExternal));
                        }
                    });
                });
                if w.is_loading() && *s.state() == State::Connected {
                    ui.ctx().request_repaint_after(Duration::from_millis(300));
                }
            }

            // --- VNC: eigener Viewer
            if let Kind::Vnc(v) = &mut s.kind {
                if *v.state() == State::Connected {
                    let focused = v.ui(ui, rect, grab);
                    self.terminal_focused |= focused;
                    return;
                }
                if v.needs_password() {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space(rect.height() / 3.0);
                            ui.heading(format!("🔒 {} verlangt ein VNC-Passwort", v.host));
                            ui.add_space(6.0);
                            let r = ui.add(egui::TextEdit::singleline(&mut v.password_input).password(true).desired_width(220.0));
                            r.request_focus();
                            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            ui.checkbox(&mut v.remember_password, "Passwort speichern");
                            ui.horizontal(|ui| {
                                ui.add_space((ui.available_width() - 200.0).max(0.0) / 2.0);
                                if ui.button("Verbinden").clicked() || enter {
                                    if let Some(pw) = v.retry_with_password() {
                                        save_vnc_pw = Some((v.conn.id, pw));
                                    }
                                }
                                if ui.button("✖ Tab schließen").clicked() {
                                    *close = Some(sid);
                                }
                            });
                        });
                    });
                    if save_vnc_pw.is_some() {
                        self.pending_pw_save = save_vnc_pw.take();
                    }
                    return;
                }
            }

            if let Kind::Ssh(term) = &mut s.kind {
                // SFTP-Dateibrowser als Seitenleiste rechts
                let mut rect = rect;
                if let Some(b) = self.sftp.get_mut(&sid) {
                    let w = sftp::WIDTH.min(rect.width() * 0.5);
                    let side = egui::Rect::from_min_max(egui::pos2(rect.max.x - w, rect.min.y), rect.max);
                    rect = egui::Rect::from_min_max(rect.min, egui::pos2(side.min.x, rect.max.y));
                    match b.ui(ui, side, &term.host) {
                        Some(sftp::BrowserAction::Close) => *cmd = Some((sid, SessionCmd::Sftp)),
                        Some(sftp::BrowserAction::Download(path)) => {
                            let local_dir =
                                std::env::var("USERPROFILE").map(|p| format!(r"{p}\Downloads")).unwrap_or_default();
                            self.dialog = Dialog::Download { session: sid, remote_path: path, local_dir };
                        }
                        Some(sftp::BrowserAction::Upload(dir)) => {
                            if let Some(files) = rfd::FileDialog::new().set_title(format!("Hochladen nach {dir}")).pick_files() {
                                self.dialog = Dialog::Upload { session: sid, files, remote_dir: dir };
                            }
                        }
                        Some(sftp::BrowserAction::CdInTerminal(dir)) => {
                            let q = dir.replace('\'', "'\\''");
                            term.input(format!("cd '{q}'\r").as_bytes());
                            s.grab_focus.set(true);
                        }
                        None => {}
                    }
                }
                let term_rect = if failed.is_some() {
                    egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rect.max.y - 36.0))
                } else {
                    rect
                };
                self.terminal_focused |= term.ui(ui, term_rect, font_size, grab);
                if std::mem::take(&mut term.wants_password) {
                    *cmd = Some((sid, SessionCmd::SendPassword));
                }
                // Dateien werden gerade über das Fenster gezogen → Hinweis zum Ablegen
                if ui.ctx().input(|i| !i.raw.hovered_files.is_empty()) {
                    let p = ui.painter();
                    p.rect_filled(rect.shrink(8.0), 8.0, ui.visuals().selection.bg_fill.gamma_multiply(0.35));
                    p.rect_stroke(
                        rect.shrink(8.0),
                        8.0,
                        egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
                        egui::StrokeKind::Inside,
                    );
                    p.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("⬆ Hier ablegen zum Hochladen nach {}", term.host),
                        egui::FontId::proportional(20.0),
                        ui.visuals().strong_text_color(),
                    );
                }
                if let Some(msg) = failed {
                    let bar = egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - 36.0), rect.max);
                    ui.scope_builder(egui::UiBuilder::new().max_rect(bar.shrink(6.0)), |ui| {
                        ui.horizontal_centered(|ui| {
                            ui.colored_label(Color32::from_rgb(220, 70, 70), format!("⛔ {msg}"));
                            if ui.button("🔄 Neu verbinden").clicked() {
                                *cmd = Some((sid, SessionCmd::Reconnect));
                            }
                            if ui.button("✖ Tab schließen").clicked() {
                                *close = Some(sid);
                            }
                        });
                    });
                }
                return;
            }
        }

        let session = sid.and_then(|id| self.sessions.iter().find(|s| s.id == id));
        let info = session.map(|s| (s.id, s.state().clone(), s.host.clone(), s.seconds_running()));
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(rect.height() / 3.0);
                match info {
                    None => {
                        ui.heading("RDP Manager");
                        ui.label("Doppelklick auf eine Verbindung links öffnet sie in einem neuen Tab.");
                        ui.weak("Oder oben im Quick-Connect-Feld einen Host eingeben und Enter drücken (Strg+K).");
                    }
                    Some((_, State::Connecting, host, secs)) => {
                        ui.spinner();
                        ui.label(format!("Verbinde mit {host} … ({secs} s)"));
                        ui.weak("Ggf. Zertifikats- oder Anmeldedialog beachten.");
                    }
                    Some((id, State::Failed(msg), host, _)) => {
                        ui.heading(format!("⛔ Verbindung zu {host} getrennt"));
                        ui.add_space(6.0);
                        ui.label(msg);
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.add_space((ui.available_width() - 230.0).max(0.0) / 2.0);
                            if ui.button("🔄 Neu verbinden").clicked() {
                                *cmd = Some((id, SessionCmd::Reconnect));
                            }
                            if ui.button("✖ Tab schließen").clicked() {
                                *close = Some(id);
                            }
                        });
                    }
                    Some(_) => {}
                }
            });
        });
    }

    fn tab_strip(&mut self, ui: &mut egui::Ui, close: &mut Option<u64>, cmd: &mut Option<(u64, SessionCmd)>) {
        egui::ScrollArea::horizontal().id_salt("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                let several = self.sessions.len() > 1;
                let mut activate = None;
                for s in &self.sessions {
                    let is_active = self.active == Some(s.id);
                    let is_split = self.split == Some(s.id);
                    let is_detached = self.detached.iter().any(|(d, _)| *d == s.id);
                    let icon = match s.state() {
                        State::Connecting => "⏳",
                        State::Connected => s.icon,
                        State::Failed(_) | State::Closed => "⛔",
                    };
                    let color = s.color();
                    let fill = match (is_active, color) {
                        (true, _) => ui.visuals().selection.bg_fill,
                        (false, Some(c)) => c.gamma_multiply(0.25),
                        (false, None) => ui.visuals().faint_bg_color,
                    };
                    let frame = egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(6, 3))
                        .corner_radius(egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 })
                        .fill(fill)
                        .stroke(if is_split {
                            egui::Stroke::new(1.0, ui.visuals().selection.bg_fill)
                        } else {
                            egui::Stroke::NONE
                        });
                    let shown = frame.show(ui, |ui| {
                        let mark = if is_detached {
                            "⧉ "
                        } else if is_split {
                            "◨ "
                        } else if is_active && self.split.is_some() {
                            "◧ "
                        } else {
                            ""
                        };
                        let text = RichText::new(format!("{mark}{icon} {}", s.title));
                        let text = if is_active { text.color(ui.visuals().selection.stroke.color) } else { text };
                        let mut tip = s.host.clone();
                        if is_detached {
                            tip.push_str("\nIn eigenem Fenster – Klick holt es nach vorne");
                        }
                        for f in s.conn.forwards.iter().filter(|f| f.args().is_some()) {
                            tip.push_str(&format!("\n🔀 {}", f.describe()));
                        }
                        if !s.conn.memo.trim().is_empty() {
                            tip.push_str(&format!("\n\n📝 {}", s.conn.memo.trim()));
                        }
                        let r = ui.add(egui::Label::new(text).sense(egui::Sense::click())).on_hover_text(tip);
                        if r.clicked() {
                            activate = Some(s.id);
                        }
                        if r.middle_clicked() {
                            *close = Some(s.id);
                        }
                        let connected = *s.state() == State::Connected;
                        let proto = s.proto();
                        let logging = s.is_logging();
                        let flags = self.menu_flags(s);
                        let snippets = &self.store.snippets;
                        let tools = &self.store.tools;
                        r.context_menu(|ui| {
                            if several && !is_detached {
                                if is_split || (is_active && self.split.is_some()) {
                                    if ui.button("◻ Teilung aufheben").clicked() {
                                        *cmd = Some((s.id, SessionCmd::Unsplit));
                                    }
                                } else if ui.button("◨ Rechts daneben anzeigen").clicked() {
                                    *cmd = Some((s.id, SessionCmd::SplitRight));
                                }
                                ui.separator();
                            }
                            if let Some(c) = session_menu(ui, connected, true, Some(proto), snippets, flags) {
                                *cmd = Some((s.id, c));
                            }
                            ui.separator();
                            ui.menu_button("🔧 Werkzeuge", |ui| {
                                for (i, t) in tools.iter().enumerate() {
                                    let name = if t.name.trim().is_empty() { t.program.as_str() } else { t.name.as_str() };
                                    if ui.button(name).clicked() {
                                        *cmd = Some((s.id, SessionCmd::Tool(i)));
                                    }
                                }
                            });
                        });
                        if logging {
                            ui.label(RichText::new("●").color(Color32::from_rgb(220, 70, 70)))
                                .on_hover_text("Sitzung wird mitgeschnitten");
                        }
                        if s.is_mirror() {
                            ui.label(RichText::new("📡").color(Color32::from_rgb(235, 135, 30)))
                                .on_hover_text("Gleichzeitige Eingabe aktiv");
                        }
                        if ui.small_button("✖").on_hover_text("Verbindung trennen").clicked() {
                            *close = Some(s.id);
                        }
                    });
                    // Farbstreifen oben am Tab
                    if let Some(c) = color {
                        let r = shown.response.rect;
                        ui.painter().rect_filled(
                            egui::Rect::from_min_max(r.min, egui::pos2(r.max.x, r.min.y + 3.0)),
                            egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 },
                            c,
                        );
                    }
                }
                if let Some(id) = activate {
                    self.activate_session(id);
                }
            });
        });
    }

    fn active_session(&self) -> Option<&Session> {
        self.active.and_then(|id| self.sessions.iter().find(|s| s.id == id))
    }

    fn run_session_cmd(&mut self, id: u64, cmd: SessionCmd) {
        let Some(pos) = self.sessions.iter().position(|s| s.id == id) else { return };
        match cmd {
            SessionCmd::Keys(label, keys) => {
                let title = self.sessions[pos].title.clone();
                match self.sessions[pos].rdp_mut().and_then(|s| s.send_combo(keys)) {
                    Ok(()) => self.info = Some(format!("{label} gesendet an {title}")),
                    Err(e) => self.error = Some(e),
                }
            }
            SessionCmd::SshKeys(label, bytes) => {
                if let Kind::Ssh(t) = &mut self.sessions[pos].kind {
                    t.send(bytes);
                    self.info = Some(format!("{label} gesendet"));
                }
                self.sessions[pos].focus();
            }
            SessionCmd::Paste => {
                match &mut self.sessions[pos].kind {
                    Kind::Ssh(t) => t.paste_clipboard(),
                    Kind::Vnc(v) => {
                        if let Ok(text) = arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                            v.type_text(&text);
                        }
                    }
                    _ => {}
                }
                self.sessions[pos].focus();
            }
            SessionCmd::CopyScreen => {
                if let Kind::Ssh(t) = &self.sessions[pos].kind {
                    self.ctx.copy_text(t.screen_text());
                    self.info = Some("Bildschirminhalt in die Zwischenablage kopiert".into());
                }
            }
            SessionCmd::Fullscreen => {
                if let Err(e) = self.sessions[pos].rdp_mut().and_then(|s| s.set_fullscreen()) {
                    self.error = Some(e);
                }
            }
            SessionCmd::SmartSize => match self.sessions[pos].rdp_mut().and_then(|s| s.toggle_smart_sizing()) {
                Ok(on) => self.info = Some(format!("Smart Sizing {}", if on { "an" } else { "aus" })),
                Err(e) => self.error = Some(e),
            },
            SessionCmd::Reconnect => {
                let conn = self.sessions[pos].conn.clone();
                self.close_session(id);
                if conn.id != 0 && self.find_any(conn.id).is_some() {
                    self.connect(conn.id);
                } else {
                    self.connect_temp(conn);
                }
            }
            SessionCmd::Close => self.close_session(id),
            SessionCmd::SplitRight => {
                if self.active == Some(id) {
                    // aktive Sitzung nach rechts → links eine andere zeigen
                    self.active = self.sessions.iter().map(|s| s.id).find(|&o| o != id);
                }
                if self.active.is_some() {
                    self.split = Some(id);
                }
            }
            SessionCmd::Unsplit => self.split = None,
            SessionCmd::Upload => {
                let remote_dir = match &self.sessions[pos].kind {
                    Kind::Ssh(t) => t.remote_dir.clone(),
                    _ => return,
                };
                if let Some(files) = rfd::FileDialog::new().set_title("Dateien hochladen").pick_files() {
                    self.dialog = Dialog::Upload { session: id, files, remote_dir };
                }
            }
            SessionCmd::Download => {
                let local_dir = std::env::var("USERPROFILE").map(|p| format!(r"{p}\Downloads")).unwrap_or_default();
                self.dialog = Dialog::Download { session: id, remote_path: String::new(), local_dir };
            }
            SessionCmd::Snippet(i, all) => {
                let Some(snip) = self.store.snippets.get(i).cloned() else { return };
                let mut text = snip.command.replace("\r\n", "\r").replace('\n', "\r");
                if snip.enter {
                    text.push('\r');
                }
                let mut sent = 0;
                for s in &mut self.sessions {
                    if (all || s.id == id) && *s.state() == State::Connected {
                        if let Kind::Ssh(t) = &mut s.kind {
                            t.send(text.as_bytes());
                            sent += 1;
                        }
                    }
                }
                if all {
                    self.info = Some(format!("Schnipsel an {sent} SSH-Sitzung(en) gesendet"));
                }
                self.sessions[pos].focus();
            }
            SessionCmd::ManageSnippets => self.handle(Action::ManageSnippets),
            SessionCmd::Search => {
                if let Kind::Ssh(t) = &mut self.sessions[pos].kind {
                    t.open_search();
                }
            }
            SessionCmd::ToggleLog => {
                if let Kind::Ssh(t) = &mut self.sessions[pos].kind {
                    if t.is_logging() {
                        let path = t.log_path().map(|p| p.display().to_string()).unwrap_or_default();
                        t.stop_log();
                        self.info = Some(format!("Mitschnitt beendet: {path}"));
                    } else {
                        match t.start_log() {
                            Ok(p) => self.info = Some(format!("Mitschnitt läuft: {}", p.display())),
                            Err(e) => self.error = Some(e),
                        }
                    }
                }
            }
            SessionCmd::SendPassword => {
                if let Kind::Ssh(t) = &mut self.sessions[pos].kind {
                    match t.password() {
                        Some(pw) => t.send_password(&pw),
                        None => {
                            // kein Passwort bekannt → einmal abfragen
                            self.dialog = Dialog::SshPassword { session: id, password: String::new(), remember: true };
                            return;
                        }
                    }
                }
                self.sessions[pos].focus();
            }
            SessionCmd::VncCtrlAltDel => {
                if let Kind::Vnc(v) = &self.sessions[pos].kind {
                    v.send_ctrl_alt_del();
                    self.info = Some("Strg+Alt+Entf gesendet".into());
                }
            }
            SessionCmd::WebReload => {
                if let Kind::Web(w) = &self.sessions[pos].kind {
                    w.reload();
                }
            }
            SessionCmd::WebExternal => {
                let url = match &self.sessions[pos].kind {
                    Kind::Web(w) => w.url(),
                    _ => self.sessions[pos].conn.web_url(),
                };
                if let Err(e) = tools::shell_open(&url, "") {
                    self.error = Some(e);
                }
            }
            SessionCmd::Tool(i) => {
                if let Some(tool) = self.store.tools.get(i).cloned() {
                    if let Err(e) = tools::run(&tool, &self.sessions[pos].conn) {
                        self.error = Some(e);
                    }
                }
            }
            SessionCmd::OpenLogs => {
                let dir = ssh::log_dir();
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::process::Command::new("explorer.exe").arg(dir).spawn();
            }
            SessionCmd::Detach => {
                if self.detached.iter().any(|(d, _)| *d == id) {
                    return;
                }
                let title = format!("{} – RDP Manager (Tab {id})", self.sessions[pos].title);
                self.sessions[pos].hide();
                self.detached.push((id, title));
                if self.split == Some(id) {
                    self.split = None;
                }
                if self.active == Some(id) {
                    self.active = self.split.take().or_else(|| {
                        self.sessions.iter().map(|s| s.id).find(|o| *o != id && !self.detached.iter().any(|(d, _)| d == o))
                    });
                }
            }
            SessionCmd::Attach => {
                if let Some(i) = self.detached.iter().position(|(d, _)| *d == id) {
                    self.detached.remove(i);
                    self.detached_hwnd.remove(&id);
                    let parent = self.parent;
                    self.sessions[pos].set_parent(parent);
                    self.active = Some(id);
                    self.sessions[pos].focus();
                }
            }
            SessionCmd::ToggleMirror => {
                if let Kind::Ssh(t) = &mut self.sessions[pos].kind {
                    t.mirror = !t.mirror;
                }
                self.sessions[pos].focus();
            }
            SessionCmd::MirrorAll => {
                let mut n = 0;
                for s in &mut self.sessions {
                    if let Kind::Ssh(t) = &mut s.kind
                        && *t.state() == State::Connected
                    {
                        t.mirror = true;
                        n += 1;
                    }
                }
                self.info = Some(format!("Gleichzeitige Eingabe an {n} SSH-Sitzungen"));
                self.sessions[pos].focus();
            }
            SessionCmd::MirrorOff => {
                for s in &mut self.sessions {
                    if let Kind::Ssh(t) = &mut s.kind {
                        t.mirror = false;
                    }
                }
            }
            SessionCmd::Sftp => {
                if self.sftp.remove(&id).is_none() {
                    if let Kind::Ssh(t) = &self.sessions[pos].kind {
                        match sftp::SftpBrowser::start(&t.conn, t.password(), &self.ctx) {
                            Ok(b) => {
                                self.sftp.insert(id, b);
                            }
                            Err(e) => self.error = Some(e),
                        }
                    }
                }
            }
            SessionCmd::InstallKey => {
                let start = std::env::var_os("USERPROFILE").map(|p| std::path::PathBuf::from(p).join(".ssh"));
                let mut dlg = rfd::FileDialog::new()
                    .set_title("Öffentlichen Schlüssel (.pub) wählen")
                    .add_filter("Öffentlicher Schlüssel", &["pub"]);
                if let Some(dir) = start.filter(|d| d.exists()) {
                    dlg = dlg.set_directory(dir);
                }
                let Some(path) = dlg.pick_file() else { return };
                let key = std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|k| ssh::install_key_command(&k));
                match key {
                    Ok(cmd) => {
                        if let Kind::Ssh(t) = &mut self.sessions[pos].kind {
                            t.send(cmd.as_bytes());
                            self.info = Some(format!(
                                "{} wird auf {} eingetragen – künftig per Schlüssel anmelden",
                                path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
                                t.host
                            ));
                        }
                        self.sessions[pos].focus();
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            SessionCmd::Notes => {
                let cid = self.sessions[pos].conn.id;
                self.handle(Action::EditNotes(cid));
            }
            SessionCmd::FitNow => match self.sessions[pos].rdp_mut().and_then(|s| s.fit_now()) {
                Ok(()) => self.info = Some("Auflösung wird an die Fenstergröße angepasst".into()),
                Err(e) => self.error = Some(e),
            },
            SessionCmd::ToggleAutoResize => {
                if let Ok(s) = self.sessions[pos].rdp_mut() {
                    let on = !s.auto_resize();
                    s.set_auto_resize(on);
                    self.info = Some(format!("Automatische Auflösungsanpassung {}", if on { "an" } else { "aus" }));
                }
            }
            SessionCmd::AllMonitors => {
                let conn = Connection { multimon: true, ..self.sessions[pos].conn.clone() };
                self.close_session(id);
                if conn.id != 0 && self.find_any(conn.id).is_some() {
                    self.handle(Action::ConnectAllMonitors(conn.id));
                } else {
                    self.launch_mstsc(&conn, None);
                }
            }
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let mut close = false;
        let mut save_conn = None;
        let mut save_folder = false;
        let mut delete = None;
        let mut password_ok = false;
        let mut export_now: Option<Option<String>> = None;
        let mut import_now = false;
        let mut save_security = false;
        // 0 = speichern, 1 = testen, 2 = Konsole für „bw login“
        let mut vault_cmd: Option<u8> = None;
        // 0 = speichern, 1 = prüfen, 2 = installieren
        let mut upd_cmd: Option<u8> = None;
        let mut pw_error = None;
        let mut save_snippets = false;
        let mut save_tools = false;
        let mut save_shared = false;
        let mut restore_backup: Option<std::path::PathBuf> = None;
        let mut import_backup: Option<(std::path::PathBuf, String)> = None;
        let mut save_backup_settings = false;
        let mut backup_now = false;
        let mut close_notes = false;
        let mut send_ssh_password = false;
        let mut start_transfer = false;
        let mut agent_cmd: Option<AgentCmd> = None;

        match &mut self.dialog {
            Dialog::None => return,
            Dialog::Connection { draft, folder, password, has_password, delete_password, is_new } => {
                let mut folders = vec![(ROOT_ID, "(Oberste Ebene)".to_string())];
                self.store.root.folder_paths("", &mut folders);
                // Ordner, von dem Anmeldedaten übernommen würden (Name, Benutzer)
                let cred_source = self.store.root.credential_folder(*folder).map(|f| (f.name.clone(), f.full_username()));
                // SSH-Verbindungen, die als Sprunghost/Tunnel dienen können
                let mut ssh_hosts: Vec<(u64, String)> = Vec::new();
                for_each_connection(&self.store.root, &mut |c| {
                    if c.protocol == Protocol::Ssh && c.id != draft.id {
                        ssh_hosts.push((c.id, format!("{} ({})", c.display_name(), c.host.trim())));
                    }
                });
                ssh_hosts.sort_by_key(|(_, n)| n.to_lowercase());
                let resp = egui::Modal::new(egui::Id::unique("conn_dialog")).show(ctx, |ui| {
                    ui.set_width(520.0);
                    ui.heading(if *is_new { "Neue Verbindung" } else { "Verbindung bearbeiten" });
                    ui.add_space(4.0);
                    let max_h = (ctx.content_rect().height() - 160.0).max(200.0);
                    egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
                        connection_form(
                            ui,
                            draft,
                            folder,
                            &folders,
                            &cred_source,
                            &ssh_hosts,
                            password,
                            *has_password,
                            delete_password,
                            self.store.vault.kind,
                        );
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        let valid = !draft.host.trim().is_empty();
                        if ui.add_enabled(valid, egui::Button::new("💾 Speichern")).clicked() {
                            save_conn = Some(false);
                        }
                        if ui.add_enabled(valid, egui::Button::new("💾 Speichern & verbinden")).clicked() {
                            save_conn = Some(true);
                        }
                        if ui.button("Abbrechen").clicked() {
                            close = true;
                        }
                        if !valid {
                            ui.weak("Hostname/IP fehlt");
                        }
                    });
                });
                if resp.should_close() && save_conn.is_none() {
                    close = true;
                }
            }
            Dialog::Password { conn, cred_id, username, password, remember } => {
                let saved = conn.id != 0;
                let from_folder = saved && *cred_id != conn.id;
                let resp = egui::Modal::new(egui::Id::unique("pw_dialog")).show(ctx, |ui| {
                    ui.set_width(360.0);
                    ui.heading(format!("Anmelden: {}", conn.display_name()));
                    ui.add_space(4.0);
                    let mut enter = false;
                    if saved {
                        ui.label(format!("Benutzer: {username}"));
                    } else {
                        ui.label(format!("Host: {}:{}", conn.host, conn.port));
                        let r = ui.add(
                            egui::TextEdit::singleline(username)
                                .hint_text("Benutzer (DOMÄNE\\benutzer)")
                                .desired_width(f32::INFINITY),
                        );
                        if username.is_empty() {
                            r.request_focus();
                        }
                        enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    }
                    let r = ui.add(
                        egui::TextEdit::singleline(password).password(true).hint_text("Passwort").desired_width(f32::INFINITY),
                    );
                    if saved || !username.is_empty() && !ui.memory(|m| m.focused().is_some()) {
                        r.request_focus();
                    }
                    enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if saved {
                        ui.checkbox(
                            remember,
                            if from_folder {
                                "Passwort für den Ordner speichern (gilt für alle Verbindungen, die es übernehmen)"
                            } else {
                                "Passwort speichern (Windows-Anmeldeinformationsverwaltung)"
                            },
                        );
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Verbinden").clicked() || enter {
                            password_ok = true;
                        }
                        if ui.button("Abbrechen").clicked() {
                            close = true;
                        }
                    });
                });
                if resp.should_close() && !password_ok {
                    close = true;
                }
            }
            Dialog::Folder { id, name, username, domain, password, has_password, delete_password, .. } => {
                let resp = egui::Modal::new(egui::Id::unique("folder_dialog")).show(ctx, |ui| {
                    ui.set_width(380.0);
                    ui.heading(if id.is_some() { "Ordner bearbeiten" } else { "Neuer Ordner" });
                    let r = ui.add(egui::TextEdit::singleline(name).hint_text("Name").desired_width(f32::INFINITY));
                    if ui.memory(|m| m.focused().is_none()) {
                        r.request_focus();
                    }
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.add_space(8.0);
                    ui.label(RichText::new("🔑 Gemeinsame Anmeldedaten (optional)").strong());
                    ui.weak("Verbindungen mit „Anmeldedaten vom Ordner übernehmen“ verwenden diese Daten – auch in Unterordnern.");
                    egui::Grid::new("folder_cred").num_columns(2).show(ui, |ui| {
                        ui.label("Benutzer");
                        ui.add(egui::TextEdit::singleline(username).hint_text("benutzer oder DOMÄNE\\benutzer").desired_width(240.0));
                        ui.end_row();
                        ui.label("Domäne");
                        ui.add(egui::TextEdit::singleline(domain).hint_text("nur für RDP, optional").desired_width(240.0));
                        ui.end_row();
                        ui.label("Passwort");
                        ui.add(
                            egui::TextEdit::singleline(password)
                                .password(true)
                                .hint_text(if *has_password { "•••••••• (gespeichert)" } else { "leer = beim Verbinden fragen" })
                                .desired_width(240.0),
                        );
                        ui.end_row();
                        if *has_password {
                            ui.label("");
                            ui.checkbox(delete_password, "Gespeichertes Passwort entfernen");
                            ui.end_row();
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let valid = !name.trim().is_empty();
                        if (ui.add_enabled(valid, egui::Button::new("OK")).clicked() || enter) && valid {
                            save_folder = true;
                        }
                        if ui.button("Abbrechen").clicked() {
                            close = true;
                        }
                    });
                });
                if resp.should_close() && !save_folder {
                    close = true;
                }
            }
            Dialog::ConfirmDelete { folder, connection, label } => {
                let resp = egui::Modal::new(egui::Id::unique("delete_dialog")).show(ctx, |ui| {
                    ui.set_width(340.0);
                    ui.heading("Löschen bestätigen");
                    ui.label(label.as_str());
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("🗑 Löschen").color(Color32::from_rgb(220, 70, 70))).clicked() {
                            delete = Some((*folder, *connection));
                        }
                        if ui.button("Abbrechen").clicked() {
                            close = true;
                        }
                    });
                });
                if resp.should_close() && delete.is_none() {
                    close = true;
                }
            }
            Dialog::Export { use_master, master, master_repeat } => {
                let resp = egui::Modal::new(egui::Id::unique("export_dialog")).show(ctx, |ui| {
                    ui.set_width(420.0);
                    ui.heading("Exportieren (mRemoteNG-XML)");
                    ui.add_space(4.0);
                    ui.label("Alle Ordner und Verbindungen inkl. gespeicherter Passwörter werden als confCons.xml (ConfVersion 2.6) gespeichert.");
                    ui.add_space(6.0);
                    ui.checkbox(use_master, "Mit Master-Passwort schützen");
                    if *use_master {
                        egui::Grid::new("export_grid").num_columns(2).show(ui, |ui| {
                            ui.label("Master-Passwort");
                            ui.add(egui::TextEdit::singleline(master).password(true).desired_width(220.0));
                            ui.end_row();
                            ui.label("Wiederholen");
                            ui.add(egui::TextEdit::singleline(master_repeat).password(true).desired_width(220.0));
                            ui.end_row();
                        });
                    } else {
                        ui.weak("Ohne Master-Passwort werden die Passwörter mit dem Standardschlüssel von mRemoteNG verschlüsselt – jeder mit mRemoteNG kann sie lesen.");
                    }
                    let valid = !*use_master || (!master.is_empty() && master == master_repeat);
                    if *use_master && !master.is_empty() && master != master_repeat {
                        ui.colored_label(Color32::from_rgb(220, 70, 70), "Passwörter stimmen nicht überein");
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(valid, egui::Button::new("💾 Speichern unter…")).clicked() {
                            export_now = Some(use_master.then(|| master.clone()));
                        }
                        if ui.button("Abbrechen").clicked() {
                            close = true;
                        }
                    });
                });
                if resp.should_close() && export_now.is_none() {
                    close = true;
                }
            }
            Dialog::Passwords { rows, orphans, only_stored, filter, edit, confirm_all } => {
                let resp = egui::Modal::new(egui::Id::unique("pw_overview")).show(ctx, |ui| {
                    ui.set_width(640.0);
                    ui.heading("🔑 Gespeicherte Passwörter");
                    ui.weak("Verschlüsselt in der Windows-Anmeldeinformationsverwaltung (Einträge „RdpManager/…“), nicht in der Konfigurationsdatei.");
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(filter).hint_text("🔍 Filtern…").desired_width(220.0));
                        ui.checkbox(only_stored, "nur gespeicherte");
                    });
                    ui.separator();
                    let needle = filter.trim().to_lowercase();
                    let max_h = (ctx.content_rect().height() - 260.0).clamp(120.0, 420.0);
                    egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                        egui::Grid::new("pw_grid").num_columns(4).striped(true).spacing([12.0, 6.0]).show(ui, |ui| {
                            ui.strong("Ordner / Verbindung");
                            ui.strong("Benutzer");
                            ui.strong("Passwort");
                            ui.strong("");
                            ui.end_row();
                            for row in rows.iter_mut() {
                                if *only_stored && !row.stored {
                                    continue;
                                }
                                if !needle.is_empty()
                                    && !row.path.to_lowercase().contains(&needle)
                                    && !row.user.to_lowercase().contains(&needle)
                                {
                                    continue;
                                }
                                ui.label(format!("{} {}", if row.is_folder { "📁" } else { "🖥" }, row.path));
                                ui.label(&row.user);
                                match (&row.inherits_from, row.stored) {
                                    (_, true) => ui.colored_label(Color32::from_rgb(60, 170, 90), "✔ gespeichert"),
                                    (Some(f), false) => ui.weak(format!("vom Ordner „{f}“")),
                                    (None, false) => ui.weak("–"),
                                };
                                ui.horizontal(|ui| {
                                    if let Some((id, pw)) = edit.as_mut().filter(|(id, _)| *id == row.id) {
                                        let r = ui.add(egui::TextEdit::singleline(pw).password(true).desired_width(140.0));
                                        r.request_focus();
                                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                        if (ui.button("💾").on_hover_text("Speichern").clicked() || enter) && !pw.is_empty() {
                                            match rdp::store_password(*id, &row.user, pw) {
                                                Ok(()) => row.stored = true,
                                                Err(e) => pw_error = Some(e),
                                            }
                                            *edit = None;
                                        } else if ui.button("✖").on_hover_text("Abbrechen").clicked() {
                                            *edit = None;
                                        }
                                    } else if row.inherits_from.is_none() || row.stored {
                                        let label = if row.stored { "Ändern" } else { "Setzen" };
                                        if ui.small_button(label).clicked() {
                                            *edit = Some((row.id, String::new()));
                                        }
                                        if row.stored && ui.small_button("🗑").on_hover_text("Passwort löschen").clicked() {
                                            rdp::delete_password(row.id);
                                            row.stored = false;
                                        }
                                    }
                                });
                                ui.end_row();
                            }
                        });
                    });
                    ui.separator();
                    let count = rows.iter().filter(|r| r.stored).count();
                    ui.label(format!("{count} Passwörter gespeichert"));
                    if !orphans.is_empty() {
                        ui.horizontal(|ui| {
                            ui.colored_label(
                                Color32::from_rgb(220, 140, 40),
                                format!("{} verwaiste Einträge (Verbindung/Ordner existiert nicht mehr)", orphans.len()),
                            );
                            if ui.button("Entfernen").clicked() {
                                for id in orphans.drain(..) {
                                    rdp::delete_password(id);
                                }
                            }
                        });
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if *confirm_all {
                            ui.colored_label(Color32::from_rgb(220, 70, 70), format!("Wirklich alle {count} Passwörter löschen?"));
                            if ui.button(RichText::new("Ja, alle löschen").color(Color32::from_rgb(220, 70, 70))).clicked() {
                                for r in rows.iter_mut().filter(|r| r.stored) {
                                    rdp::delete_password(r.id);
                                    r.stored = false;
                                }
                                *confirm_all = false;
                            }
                            if ui.button("Nein").clicked() {
                                *confirm_all = false;
                            }
                        } else {
                            if ui.add_enabled(count > 0, egui::Button::new("🗑 Alle löschen…")).clicked() {
                                *confirm_all = true;
                            }
                            if ui.button("Schließen").clicked() {
                                close = true;
                            }
                        }
                    });
                });
                if resp.should_close() {
                    close = true;
                }
            }
            Dialog::Snippets { list } => {
                let resp = egui::Modal::new(egui::Id::unique("snippet_dialog")).show(ctx, |ui| {
                    ui.set_width(620.0);
                    ui.heading("📝 Befehls-Schnipsel");
                    ui.weak("Werden per Klick an die SSH-Sitzung gesendet (Menü „Sitzung“, ⌨ Tasten oder Rechtsklick auf den Tab).");
                    ui.add_space(4.0);
                    let max_h = (ctx.content_rect().height() - 240.0).clamp(120.0, 420.0);
                    let mut remove = None;
                    let mut swap = None;
                    egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                        let n = list.len();
                        for (i, s) in list.iter_mut().enumerate() {
                            ui.push_id(i, |ui| {
                                egui::Frame::group(ui.style()).show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.add(egui::TextEdit::singleline(&mut s.name).hint_text("Name").desired_width(220.0));
                                        ui.checkbox(&mut s.enter, "Enter senden");
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.small_button("🗑").on_hover_text("Löschen").clicked() {
                                                remove = Some(i);
                                            }
                                            if ui.add_enabled(i + 1 < n, egui::Button::new("⏷").small()).clicked() {
                                                swap = Some((i, i + 1));
                                            }
                                            if ui.add_enabled(i > 0, egui::Button::new("⏶").small()).clicked() {
                                                swap = Some((i - 1, i));
                                            }
                                        });
                                    });
                                    ui.add(
                                        egui::TextEdit::multiline(&mut s.command)
                                            .code_editor()
                                            .hint_text("Befehl, z. B. sudo systemctl restart nginx")
                                            .desired_rows(1)
                                            .desired_width(f32::INFINITY),
                                    );
                                });
                            });
                        }
                    });
                    if let Some(i) = remove {
                        list.remove(i);
                    }
                    if let Some((a, b)) = swap {
                        list.swap(a, b);
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("➕ Neuer Schnipsel").clicked() {
                            list.push(model::Snippet { name: String::new(), command: String::new(), enter: true });
                        }
                        ui.separator();
                        if ui.button("💾 Speichern").clicked() {
                            save_snippets = true;
                        }
                        if ui.button("Abbrechen").clicked() {
                            close = true;
                        }
                    });
                });
                if resp.should_close() && !save_snippets {
                    close = true;
                }
            }
            Dialog::SshPassword { password, remember, .. } => {
                let resp = egui::Modal::new(egui::Id::unique("ssh_pw_dialog")).show(ctx, |ui| {
                    ui.set_width(360.0);
                    ui.heading("🔑 Passwort senden");
                    ui.weak("Für diese Verbindung ist kein Passwort gespeichert. Es wird eingetippt und mit Enter abgeschickt.");
                    ui.add_space(4.0);
                    let r = ui.add(egui::TextEdit::singleline(password).password(true).hint_text("Passwort").desired_width(f32::INFINITY));
                    r.request_focus();
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.checkbox(remember, "Für diese Verbindung speichern (Windows-Anmeldeinformationsverwaltung)");
                    ui.horizontal(|ui| {
                        if (ui.button("Senden").clicked() || enter) && !password.is_empty() {
                            send_ssh_password = true;
                        }
                        if ui.button("Abbrechen").clicked() {
                            close = true;
                        }
                    });
                });
                if resp.should_close() && !send_ssh_password {
                    close = true;
                }
            }
            Dialog::SshAgent { service, keys } => {
                let resp = egui::Modal::new(egui::Id::unique("agent_dialog")).show(ctx, |ui| {
                    ui.set_width(600.0);
                    ui.heading("🗝 SSH-Agent & Schlüssel");
                    ui.weak("Der Windows-Dienst „OpenSSH Authentication Agent“ hält entschlüsselte Schlüssel bereit – \
                             die Passphrase wird dann nur einmal eingegeben. ssh, scp und sftp nutzen ihn automatisch.");
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Dienst:");
                        match service {
                            ssh::AgentService::Running => ui.colored_label(Color32::from_rgb(60, 170, 90), "✔ läuft"),
                            ssh::AgentService::Stopped => ui.colored_label(Color32::from_rgb(220, 140, 40), "angehalten"),
                            ssh::AgentService::Disabled => ui.colored_label(Color32::from_rgb(220, 140, 40), "deaktiviert"),
                            ssh::AgentService::Missing => ui.colored_label(Color32::from_rgb(220, 70, 70), "nicht installiert"),
                        };
                        if matches!(service, ssh::AgentService::Stopped | ssh::AgentService::Disabled)
                            && ui
                                .button("▶ Dienst starten (Administrator)…")
                                .on_hover_text("Stellt den Dienst auf „Automatisch“ und startet ihn – Windows fragt nach Administratorrechten")
                                .clicked()
                        {
                            agent_cmd = Some(AgentCmd::StartService);
                        }
                    });
                    ui.add_space(4.0);
                    ui.label(RichText::new("Geladene Schlüssel").strong());
                    match keys {
                        Ok(k) if k.is_empty() => {
                            ui.weak("Keine Schlüssel geladen.");
                        }
                        Ok(k) => {
                            for line in k.iter() {
                                ui.monospace(line);
                            }
                        }
                        Err(e) => {
                            ui.colored_label(Color32::from_rgb(220, 140, 40), format!("Agent nicht erreichbar: {e}"));
                        }
                    }
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("➕ Schlüssel hinzufügen…").on_hover_text("ssh-add – die Passphrase wird in einem Konsolenfenster abgefragt").clicked() {
                            agent_cmd = Some(AgentCmd::Add);
                        }
                        if ui.button("🆕 Neuen Schlüssel erzeugen…").on_hover_text("ssh-keygen -t ed25519 in einem Konsolenfenster").clicked() {
                            agent_cmd = Some(AgentCmd::Generate);
                        }
                        if ui.button("📂 .ssh-Ordner öffnen").clicked() {
                            agent_cmd = Some(AgentCmd::OpenDir);
                        }
                        if ui.button("🔄 Aktualisieren").clicked() {
                            agent_cmd = Some(AgentCmd::Refresh);
                        }
                    });
                    ui.add_space(4.0);
                    ui.weak("Öffentlichen Schlüssel auf einen Server bringen: SSH-Sitzung öffnen > Sitzung > „Öffentlichen Schlüssel eintragen…“. \
                             Agent-Weiterleitung (-A) und Schlüsseldatei stellst du je Verbindung ein.");
                    ui.separator();
                    if ui.button("Schließen").clicked() {
                        close = true;
                    }
                });
                if resp.should_close() {
                    close = true;
                }
            }
            Dialog::ReleaseNotes { since } => {
                let resp = egui::Modal::new(egui::Id::unique("notes_dialog")).show(ctx, |ui| {
                    ui.set_width(620.0);
                    ui.horizontal(|ui| {
                        ui.heading("📋 Versionshinweise");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.weak(concat!("RDP Manager ", env!("CARGO_PKG_VERSION")));
                        });
                    });
                    if !since.is_empty() && notes::has_news(since) {
                        ui.label(format!("Neu seit Version {since}:"));
                    } else if since.is_empty() && self.store.last_seen_version != env!("CARGO_PKG_VERSION") {
                        ui.label("Das ist neu in dieser Version:");
                    }
                    ui.separator();
                    let max_h = (ctx.content_rect().height() - 200.0).clamp(160.0, 560.0);
                    egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                        notes::show(ui, since);
                    });
                    ui.separator();
                    if ui.button("OK").clicked() {
                        close_notes = true;
                    }
                });
                if resp.should_close() {
                    close_notes = true;
                }
            }
            Dialog::Backups { list, confirm } => {
                let resp = egui::Modal::new(egui::Id::unique("backup_dialog")).show(ctx, |ui| {
                    ui.set_width(640.0);
                    ui.heading("🕘 Sicherungen");
                    ui.weak(
                        "Der bisherige Stand wird automatisch gesichert: beim ersten Speichern pro Programmstart, danach höchstens \
                         stündlich und immer vor Import (ersetzen), Ordner löschen und Wiederherstellen. Passwörter liegen nicht in \
                         den Sicherungen, sondern bleiben im Windows-Tresor.",
                    );
                    ui.add_space(6.0);
                    let max_h = (ctx.content_rect().height() - 330.0).clamp(120.0, 360.0);
                    egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                        if list.is_empty() {
                            ui.weak("Noch keine Sicherungen vorhanden.");
                        }
                        egui::Grid::new("backup_grid").num_columns(4).striped(true).spacing([14.0, 6.0]).show(ui, |ui| {
                            for (i, b) in list.iter().enumerate() {
                                ui.label(&b.when);
                                ui.label(if b.reason.is_empty() { "automatisch".to_string() } else { b.reason.clone() });
                                if b.readable {
                                    ui.weak(format!("{} Ordner · {} Verbindungen", b.folders, b.connections));
                                } else {
                                    ui.colored_label(Color32::from_rgb(220, 70, 70), "nicht lesbar");
                                }
                                ui.horizontal(|ui| {
                                    ui.add_enabled_ui(b.readable, |ui| {
                                        if *confirm == Some(i) {
                                            if ui
                                                .button(RichText::new("Ja, ersetzen").color(Color32::from_rgb(220, 70, 70)))
                                                .clicked()
                                            {
                                                restore_backup = Some(b.path.clone());
                                            }
                                            if ui.small_button("Nein").clicked() {
                                                *confirm = None;
                                            }
                                        } else {
                                            if ui
                                                .small_button("Wiederherstellen")
                                                .on_hover_text("Ersetzt alle Verbindungen durch diesen Stand (der aktuelle wird vorher gesichert)")
                                                .clicked()
                                            {
                                                *confirm = Some(i);
                                            }
                                            if ui
                                                .small_button("Als Ordner holen")
                                                .on_hover_text("Fügt die Verbindungen dieses Stands als neuen Ordner hinzu – nichts wird ersetzt")
                                                .clicked()
                                            {
                                                import_backup = Some((b.path.clone(), b.when.clone()));
                                            }
                                        }
                                    });
                                });
                                ui.end_row();
                            }
                        });
                    });
                    ui.separator();
                    ui.label(RichText::new("Einstellungen").strong());
                    let mut changed = ui.checkbox(&mut self.store.backup_enabled, "Automatisch sichern").changed();
                    ui.add_enabled_ui(self.store.backup_enabled, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Aufbewahren:");
                            changed |= ui.add(egui::DragValue::new(&mut self.store.backup_keep).range(3..=200)).changed();
                            ui.label("Sicherungen, zusätzlich je eine pro Tag der letzten 14 Tage");
                        });
                        ui.horizontal(|ui| {
                            ui.label("Zusätzlicher Ordner:");
                            changed |= ui
                                .add(
                                    egui::TextEdit::singleline(&mut self.store.backup_extra_dir)
                                        .hint_text("optional, z. B. Netzlaufwerk oder OneDrive")
                                        .desired_width(330.0),
                                )
                                .lost_focus();
                            if ui.small_button("…").clicked() {
                                if let Some(d) = rfd::FileDialog::new().pick_folder() {
                                    self.store.backup_extra_dir = d.display().to_string();
                                    changed = true;
                                }
                            }
                        });
                    });
                    if changed {
                        save_backup_settings = true;
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("💾 Jetzt sichern").clicked() {
                            backup_now = true;
                        }
                        if ui.button("📂 Ordner öffnen").clicked() {
                            let dir = backup::backup_dir();
                            let _ = std::fs::create_dir_all(&dir);
                            let _ = std::process::Command::new("explorer.exe").arg(dir).spawn();
                        }
                        if ui.button("Schließen").clicked() {
                            close = true;
                        }
                    });
                });
                if resp.should_close() && restore_backup.is_none() && import_backup.is_none() {
                    close = true;
                }
            }
            Dialog::Tools { list } => {
                let resp = egui::Modal::new(egui::Id::unique("tools_dialog")).show(ctx, |ui| {
                    ui.set_width(660.0);
                    ui.heading("🔧 Externe Werkzeuge");
                    ui.weak("Erscheinen im Rechtsklick-Menü jeder Verbindung und jedes Tabs. Programm darf auch eine .msc-Datei, ein UNC-Pfad oder eine URL sein.");
                    ui.label(RichText::new(format!("Platzhalter: {}", tools::PLACEHOLDERS)).monospace().small());
                    ui.add_space(4.0);
                    let max_h = (ctx.content_rect().height() - 260.0).clamp(120.0, 420.0);
                    let mut remove = None;
                    let mut swap = None;
                    egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                        let n = list.len();
                        egui::Grid::new("tools_grid").num_columns(4).spacing([8.0, 6.0]).show(ui, |ui| {
                            ui.strong("Name");
                            ui.strong("Programm / Datei / URL");
                            ui.strong("Argumente");
                            ui.strong("");
                            ui.end_row();
                            for (i, t) in list.iter_mut().enumerate() {
                                ui.add(egui::TextEdit::singleline(&mut t.name).desired_width(150.0));
                                ui.horizontal(|ui| {
                                    ui.add(egui::TextEdit::singleline(&mut t.program).desired_width(150.0));
                                    if ui.small_button("…").on_hover_text("Programm wählen").clicked() {
                                        if let Some(p) = rfd::FileDialog::new().pick_file() {
                                            t.program = p.display().to_string();
                                        }
                                    }
                                });
                                ui.add(egui::TextEdit::singleline(&mut t.args).desired_width(220.0));
                                ui.horizontal(|ui| {
                                    if ui.add_enabled(i > 0, egui::Button::new("⏶").small()).clicked() {
                                        swap = Some((i - 1, i));
                                    }
                                    if ui.add_enabled(i + 1 < n, egui::Button::new("⏷").small()).clicked() {
                                        swap = Some((i, i + 1));
                                    }
                                    if ui.small_button("🗑").clicked() {
                                        remove = Some(i);
                                    }
                                });
                                ui.end_row();
                            }
                        });
                    });
                    if let Some(i) = remove {
                        list.remove(i);
                    }
                    if let Some((a, b)) = swap {
                        list.swap(a, b);
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("➕ Neues Werkzeug").clicked() {
                            list.push(model::ExtTool::default());
                        }
                        if ui.button("Standardwerkzeuge wiederherstellen").clicked() {
                            for d in tools::defaults() {
                                if !list.iter().any(|t| t.name == d.name) {
                                    list.push(d);
                                }
                            }
                        }
                        ui.separator();
                        if ui.button("💾 Speichern").clicked() {
                            save_tools = true;
                        }
                        if ui.button("Abbrechen").clicked() {
                            close = true;
                        }
                    });
                });
                if resp.should_close() && !save_tools {
                    close = true;
                }
            }
            Dialog::Shared { list } => {
                let resp = egui::Modal::new(egui::Id::unique("shared_dialog")).show(ctx, |ui| {
                    ui.set_width(620.0);
                    ui.heading("🌐 Gemeinsame Verbindungslisten");
                    ui.weak(
                        "Eine Datei, z. B. auf einem Netzlaufwerk, die mehrere Kollegen einbinden: connections.json des RDP Managers \
                         oder confCons.xml von mRemoteNG (ohne Master-Passwort). Sie erscheint schreibgeschützt im Baum und wird \
                         beim Start sowie bei Änderungen automatisch neu eingelesen.",
                    );
                    ui.add_space(6.0);
                    let mut remove = None;
                    egui::Grid::new("shared_grid").num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
                        ui.strong("Anzeigename");
                        ui.strong("Datei");
                        ui.strong("");
                        ui.end_row();
                        for (i, s) in list.iter_mut().enumerate() {
                            ui.add(egui::TextEdit::singleline(&mut s.name).hint_text("z. B. Team-Server").desired_width(150.0));
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut s.path).hint_text(r"\\server\freigabe\verbindungen.json").desired_width(330.0));
                                if ui.small_button("…").clicked() {
                                    if let Some(p) = rfd::FileDialog::new()
                                        .add_filter("Verbindungslisten", &["json", "xml"])
                                        .pick_file()
                                    {
                                        if s.name.trim().is_empty() {
                                            s.name = p.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                                        }
                                        s.path = p.display().to_string();
                                    }
                                }
                            });
                            if ui.small_button("🗑").clicked() {
                                remove = Some(i);
                            }
                            ui.end_row();
                        }
                    });
                    if let Some(i) = remove {
                        list.remove(i);
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("➕ Liste hinzufügen").clicked() {
                            list.push(model::SharedSource::default());
                        }
                        ui.separator();
                        if ui.button("💾 Speichern").clicked() {
                            save_shared = true;
                        }
                        if ui.button("Abbrechen").clicked() {
                            close = true;
                        }
                    });
                });
                if resp.should_close() && !save_shared {
                    close = true;
                }
            }
            Dialog::Upload { files, remote_dir, .. } => {
                let resp = egui::Modal::new(egui::Id::unique("upload_dialog")).show(ctx, |ui| {
                    ui.set_width(460.0);
                    ui.heading("⬆ Hochladen (scp)");
                    ui.add_space(4.0);
                    for f in files.iter().take(8) {
                        ui.label(format!("📄 {}", f.display()));
                    }
                    if files.len() > 8 {
                        ui.weak(format!("… und {} weitere", files.len() - 8));
                    }
                    ui.add_space(6.0);
                    ui.label("Zielordner auf dem Server:");
                    let r = ui.add(
                        egui::TextEdit::singleline(remote_dir)
                            .hint_text("leer = Home-Verzeichnis, z. B. /tmp oder ~/upload")
                            .desired_width(f32::INFINITY),
                    );
                    r.request_focus();
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("⬆ Hochladen").clicked() || enter {
                            start_transfer = true;
                        }
                        if ui.button("Abbrechen").clicked() {
                            close = true;
                        }
                    });
                });
                if resp.should_close() && !start_transfer {
                    close = true;
                }
            }
            Dialog::Download { remote_path, local_dir, .. } => {
                let resp = egui::Modal::new(egui::Id::unique("download_dialog")).show(ctx, |ui| {
                    ui.set_width(460.0);
                    ui.heading("⬇ Herunterladen (scp)");
                    ui.add_space(4.0);
                    ui.label("Datei oder Ordner auf dem Server:");
                    let r = ui.add(
                        egui::TextEdit::singleline(remote_path)
                            .hint_text("z. B. /var/log/syslog oder ~/backup")
                            .desired_width(f32::INFINITY),
                    );
                    if remote_path.is_empty() {
                        r.request_focus();
                    }
                    ui.add_space(4.0);
                    ui.label("Speichern in:");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(local_dir).desired_width(360.0));
                        if ui.button("📂").on_hover_text("Ordner wählen").clicked() {
                            if let Some(d) = rfd::FileDialog::new().set_directory(&*local_dir).pick_folder() {
                                *local_dir = d.display().to_string();
                            }
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let valid = !remote_path.trim().is_empty() && !local_dir.trim().is_empty();
                        if ui.add_enabled(valid, egui::Button::new("⬇ Herunterladen")).clicked() {
                            start_transfer = true;
                        }
                        if ui.button("Abbrechen").clicked() {
                            close = true;
                        }
                    });
                });
                if resp.should_close() && !start_transfer {
                    close = true;
                }
            }
            Dialog::Import { path, needs_password, password, replace, error, .. } => {
                let resp = egui::Modal::new(egui::Id::unique("import_dialog")).show(ctx, |ui| {
                    ui.set_width(440.0);
                    ui.heading("Importieren (mRemoteNG-XML)");
                    ui.add_space(4.0);
                    ui.label(format!("Datei: {}", path.display()));
                    ui.add_space(6.0);
                    if *needs_password {
                        ui.label("Die Datei ist mit einem Master-Passwort geschützt:");
                        let r = ui.add(egui::TextEdit::singleline(password).password(true).desired_width(f32::INFINITY));
                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            import_now = true;
                        }
                    }
                    ui.radio_value(replace, false, "Hinzufügen (als neuen Ordner)");
                    ui.radio_value(replace, true, "Alle vorhandenen Verbindungen ersetzen");
                    if *replace {
                        ui.colored_label(
                            Color32::from_rgb(220, 140, 40),
                            "⚠ Vorhandene Verbindungen werden ersetzt. Der bisherige Stand wird vorher gesichert (Datei > Sicherungen).",
                        );
                    }
                    if let Some(e) = error {
                        ui.colored_label(Color32::from_rgb(220, 70, 70), e.as_str());
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("📥 Importieren").clicked() {
                            import_now = true;
                        }
                        if ui.button("Abbrechen").clicked() {
                            close = true;
                        }
                    });
                });
                if resp.should_close() && !import_now {
                    close = true;
                }
            }
            Dialog::Security { current, new_pw, repeat, remove_pw, hello, hello_available, at_start, idle_minutes, error } => {
                let has_pw = !self.store.lock.password_hash.is_empty();
                let resp = egui::Modal::new(egui::Id::unique("security_dialog")).show(ctx, |ui| {
                    ui.set_width(440.0);
                    ui.heading("🔒 Sperre & Hauptkennwort");
                    ui.weak("Sperrt RDP Manager beim Start und nach Leerlauf – gegen Benutzung durch andere am entsperrten PC. \
                             Offene Sitzungen bleiben dabei verbunden. Die Passwörter selbst liegen weiterhin im Windows-Tresor.");
                    ui.add_space(6.0);
                    egui::Grid::new("sec_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                        if has_pw {
                            ui.label("Aktuelles Kennwort *");
                            ui.add(egui::TextEdit::singleline(current).password(true).desired_width(220.0));
                            ui.end_row();
                        }
                        if !*remove_pw {
                            ui.label(if has_pw { "Neues Hauptkennwort" } else { "Hauptkennwort" });
                            ui.add(
                                egui::TextEdit::singleline(new_pw)
                                    .password(true)
                                    .hint_text(if has_pw { "leer = unverändert" } else { "leer = keins" })
                                    .desired_width(220.0),
                            );
                            ui.end_row();
                            ui.label("Wiederholen");
                            ui.add(egui::TextEdit::singleline(repeat).password(true).desired_width(220.0));
                            ui.end_row();
                        }
                        if has_pw {
                            ui.label("");
                            ui.checkbox(remove_pw, "Hauptkennwort entfernen");
                            ui.end_row();
                        }
                        ui.label("Windows Hello");
                        ui.add_enabled_ui(*hello_available || *hello, |ui| {
                            ui.checkbox(hello, "Mit PIN, Fingerabdruck oder Gesicht entsperren");
                        })
                        .response
                        .on_disabled_hover_text("Windows Hello ist auf diesem PC nicht eingerichtet (Einstellungen > Konten > Anmeldeoptionen).");
                        ui.end_row();
                        ui.label("Beim Start sperren");
                        ui.checkbox(at_start, "");
                        ui.end_row();
                        ui.label("Nach Leerlauf sperren");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(idle_minutes).range(0..=480).suffix(" min"));
                            ui.weak("ohne Eingabe am PC (0 = nie)");
                        });
                        ui.end_row();
                    });
                    ui.weak("Ein vergessenes Hauptkennwort lässt sich nicht wiederherstellen – dann in connections.json den Eintrag „lock“ löschen.");
                    if let Some(e) = error {
                        ui.colored_label(Color32::from_rgb(220, 70, 70), e.as_str());
                    }
                    ui.horizontal(|ui| {
                        if ui.button("💾 Speichern").clicked() {
                            save_security = true;
                        }
                        if ui.button("Abbrechen").clicked() {
                            close = true;
                        }
                    });
                });
                if resp.should_close() && !save_security {
                    close = true;
                }
            }
            Dialog::Update { source, auto } => {
                let busy = self.upd_check.is_some() || self.upd_fetch.is_some();
                let resp = egui::Modal::new(egui::Id::unique("update_dialog")).show(ctx, |ui| {
                    ui.set_width(520.0);
                    ui.heading("🔄 Updates");
                    ui.label(format!("Installiert: Version {}", env!("CARGO_PKG_VERSION")));
                    ui.add_space(6.0);
                    ui.label("Update-Quelle");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(source)
                                .hint_text(format!("leer = {}", update::DEFAULT_SOURCE))
                                .desired_width(420.0),
                        );
                        if ui.button("…").on_hover_text("Ordner wählen").clicked()
                            && let Some(d) = rfd::FileDialog::new().pick_folder()
                        {
                            *source = d.display().to_string();
                        }
                    });
                    ui.weak("Leer lassen für die offiziellen Releases auf GitHub. Alternativ ein GitHub-Repository (github.com/besitzer/repo), \
                             ein Ordner (z. B. Netzlaufwerk) oder eine Webadresse, unter der die Dateien „RdpManager-x.y.z-x64.msi“ liegen. \
                             Die höchste Version wird gefunden.");
                    ui.checkbox(auto, "Beim Programmstart nach Updates suchen");
                    ui.add_space(6.0);
                    if busy {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(if self.upd_fetch.is_some() { "Lade Installer …" } else { "Suche …" });
                        });
                    } else if let Some((ok, msg)) = &self.upd_msg {
                        let color = if *ok { Color32::from_rgb(60, 170, 90) } else { Color32::from_rgb(220, 70, 70) };
                        ui.colored_label(color, msg.as_str());
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!busy, egui::Button::new("🔍 Jetzt prüfen")).clicked() {
                            upd_cmd = Some(1);
                        }
                        if let Some(f) = &self.upd_found
                            && ui
                                .add_enabled(!busy, egui::Button::new(format!("⬆ Version {} installieren", update::version_text(f.version))))
                                .on_hover_text("Startet den Installer und beendet RDP Manager. Offene Verbindungen werden getrennt und beim nächsten Start wieder geöffnet (falls eingeschaltet).")
                                .clicked()
                        {
                            upd_cmd = Some(2);
                        }
                        if ui.button("💾 Speichern").clicked() {
                            upd_cmd = Some(0);
                        }
                        if ui.button("Schließen").clicked() {
                            close = true;
                        }
                    });
                });
                if resp.should_close() && upd_cmd.is_none() {
                    close = true;
                }
            }
            Dialog::Vault { settings, test_entry, result } => {
                let resp = egui::Modal::new(egui::Id::unique("vault_dialog")).show(ctx, |ui| {
                    ui.set_width(520.0);
                    ui.heading("🔐 Passwort-Tresor");
                    ui.weak("Verbindungen mit einem „Tresor-Eintrag“ holen Benutzer und Passwort beim Verbinden aus KeePass oder Bitwarden. \
                             Das Masterkennwort wird einmal pro Programmstart abgefragt.");
                    ui.add_space(6.0);
                    match vault::settings_ui(ui, settings, test_entry) {
                        vault::SettingsAction::Test => vault_cmd = Some(1),
                        vault::SettingsAction::Login => vault_cmd = Some(2),
                        vault::SettingsAction::None => {}
                    }
                    if self.vault.busy() {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Teste …");
                        });
                    } else if let Some((ok, msg)) = result {
                        let color = if *ok { Color32::from_rgb(60, 170, 90) } else { Color32::from_rgb(220, 70, 70) };
                        ui.colored_label(color, msg.as_str());
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("💾 Speichern").clicked() {
                            vault_cmd = Some(0);
                        }
                        if ui.button("Abbrechen").clicked() {
                            close = true;
                        }
                    });
                });
                if resp.should_close() && vault_cmd.is_none() {
                    close = true;
                }
            }
        }

        if let Some(e) = pw_error {
            self.error = Some(e);
        }
        if save_security {
            self.save_security();
            return;
        }
        if let Some(c) = upd_cmd {
            if let Dialog::Update { source, auto } = &self.dialog {
                let (source, auto) = (source.trim().trim_matches('"').to_string(), *auto);
                if source != self.store.update_source || auto != self.store.update_check {
                    self.store.update_source = source;
                    self.store.update_check = auto;
                    self.save();
                }
            }
            match c {
                0 => self.dialog = Dialog::None,
                1 => {
                    self.upd_msg = None;
                    self.upd_check = Some((update::check_async(&self.store.update_source, &self.ctx), true));
                }
                _ => {
                    if let Some(f) = &self.upd_found {
                        self.upd_fetch = Some(update::fetch_async(f, &self.ctx));
                    }
                }
            }
            return;
        }
        if let Some(c) = vault_cmd {
            let Dialog::Vault { settings, test_entry, .. } = &self.dialog else { return };
            let (settings, entry) = (settings.clone(), test_entry.trim().to_string());
            let s = &self.store.vault;
            if s.kind != settings.kind || s.database != settings.database || s.key_file != settings.key_file {
                self.vault.lock(); // anderes Ziel → Masterkennwort neu abfragen
            }
            self.store.vault = settings;
            self.save();
            match c {
                0 => self.dialog = Dialog::None,
                1 => {
                    let settings = self.store.vault.clone();
                    self.vault.request(&settings, vault::Purpose::Test, &entry, &entry, &self.ctx);
                }
                _ => {
                    use std::os::windows::process::CommandExt;
                    let prog = vault::program(&self.store.vault);
                    let r = std::process::Command::new("cmd.exe")
                        .raw_arg(format!("/k \"\"{}\" login\"", prog.display()))
                        .creation_flags(0x10) // CREATE_NEW_CONSOLE
                        .spawn();
                    if let Err(e) = r {
                        self.error = Some(e.to_string());
                    }
                }
            }
            return;
        }
        if save_snippets {
            if let Dialog::Snippets { list } = std::mem::replace(&mut self.dialog, Dialog::None) {
                self.store.snippets =
                    list.into_iter().filter(|s| !s.command.trim().is_empty() || !s.name.trim().is_empty()).collect();
                self.save();
            }
            return;
        }
        if send_ssh_password {
            if let Dialog::SshPassword { session, password, remember } = std::mem::replace(&mut self.dialog, Dialog::None) {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session) {
                    let conn_id = s.conn.id;
                    if let Kind::Ssh(t) = &mut s.kind {
                        t.send_password(&password);
                        t.set_password(password.clone()); // für weitere sudo-Abfragen in dieser Sitzung
                    }
                    s.focus();
                    if remember {
                        self.pending_pw_save = Some((conn_id, password));
                    }
                }
            }
            return;
        }
        if close_notes {
            self.dialog = Dialog::None;
            if self.store.last_seen_version != env!("CARGO_PKG_VERSION") {
                self.store.last_seen_version = env!("CARGO_PKG_VERSION").to_string();
                self.save();
            }
            return;
        }
        if save_backup_settings {
            if let Err(e) = self.store.save() {
                self.error = Some(format!("Speichern fehlgeschlagen: {e}"));
            }
        }
        if let Some(p) = restore_backup {
            self.dialog = Dialog::None;
            self.restore_backup(&p);
            return;
        }
        if let Some((p, label)) = import_backup {
            self.dialog = Dialog::None;
            self.import_backup(&p, &label);
            return;
        }
        if backup_now {
            // den aktuellen Stand sofort sichern (auch innerhalb der Stunde)
            if let Err(e) = self.store.save() {
                self.error = Some(format!("Speichern fehlgeschlagen: {e}"));
            }
            self.backup_now(Some("manuell"));
            if let Dialog::Backups { list, confirm } = &mut self.dialog {
                *list = backup::list(&backup::backup_dir());
                *confirm = None;
            }
            self.info = Some("Sicherung angelegt".into());
            return;
        }
        if let Some(c) = agent_cmd {
            let ssh_dir = std::env::var_os("USERPROFILE").map(|p| std::path::PathBuf::from(p).join(".ssh"));
            let r = match c {
                AgentCmd::StartService => tools::shell_runas(
                    "powershell.exe",
                    "-NoProfile -Command \"Set-Service ssh-agent -StartupType Automatic; Start-Service ssh-agent\"",
                ),
                AgentCmd::Add => {
                    let mut dlg = rfd::FileDialog::new().set_title("Privaten Schlüssel wählen (nicht die .pub-Datei)");
                    if let Some(d) = ssh_dir.clone().filter(|d| d.exists()) {
                        dlg = dlg.set_directory(d);
                    }
                    match dlg.pick_file() {
                        Some(p) => ssh::run_in_console("ssh-add.exe", &[p.display().to_string()]),
                        None => Ok(()),
                    }
                }
                // ssh-keygen fragt im Konsolenfenster nach Dateiname und Passphrase (und vor dem Überschreiben)
                AgentCmd::Generate => ssh::run_in_console("ssh-keygen.exe", &["-t".into(), "ed25519".into()]),
                AgentCmd::OpenDir => {
                    if let Some(d) = &ssh_dir {
                        let _ = std::fs::create_dir_all(d);
                        let _ = std::process::Command::new("explorer.exe").arg(d).spawn();
                    }
                    Ok(())
                }
                AgentCmd::Refresh => Ok(()),
            };
            if let Err(e) = r {
                self.error = Some(e);
            }
            // Status neu einlesen (nach dem Start des Dienstes / ssh-add etwas später erneut über „Aktualisieren“)
            if let Dialog::SshAgent { service, keys } = &mut self.dialog {
                *service = ssh::agent_service();
                *keys = ssh::agent_keys();
            }
            return;
        }
        if save_tools {
            if let Dialog::Tools { list } = std::mem::replace(&mut self.dialog, Dialog::None) {
                self.store.tools = list.into_iter().filter(|t| !t.name.trim().is_empty() || !t.program.trim().is_empty()).collect();
                self.save();
            }
            return;
        }
        if save_shared {
            if let Dialog::Shared { list } = std::mem::replace(&mut self.dialog, Dialog::None) {
                self.store.shared_sources = list.into_iter().filter(|s| !s.path.trim().is_empty()).collect();
                for s in &mut self.store.shared_sources {
                    s.path = s.path.trim().trim_matches('"').to_string();
                    if s.name.trim().is_empty() {
                        s.name = std::path::Path::new(&s.path).file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    }
                }
                self.shared =
                    self.store.shared_sources.iter().map(|s| shared::SharedTree::new(&s.name, &s.path, &self.ctx)).collect();
                self.save();
            }
            return;
        }
        if start_transfer {
            match std::mem::replace(&mut self.dialog, Dialog::None) {
                Dialog::Upload { session, files, remote_dir } => self.start_transfer(session, true, files, remote_dir),
                Dialog::Download { session, remote_path, local_dir } => {
                    self.start_transfer(session, false, vec![local_dir.into()], remote_path)
                }
                other => self.dialog = other,
            }
            return;
        }
        if let Some(master) = export_now {
            self.dialog = Dialog::None;
            self.do_export(master);
            return;
        }
        if import_now {
            self.run_import();
            return;
        }

        if let Some(connect_after) = save_conn {
            if let Dialog::Connection { mut draft, folder, password, delete_password, is_new, .. } =
                std::mem::replace(&mut self.dialog, Dialog::None)
            {
                draft.host = draft.host.trim().to_string();
                if is_new {
                    draft.id = self.store.new_id();
                } else {
                    self.store.root.remove_connection(draft.id);
                }
                if delete_password {
                    rdp::delete_password(draft.id);
                }
                if !password.is_empty() {
                    if let Err(e) = rdp::store_password(draft.id, &draft.full_username(), &password) {
                        self.error = Some(e);
                    }
                }
                let id = draft.id;
                match self.store.root.find_folder_mut(folder) {
                    Some(f) => f.connections.push(draft),
                    None => self.store.root.connections.push(draft),
                }
                self.selected = Some(id);
                self.save();
                if connect_after {
                    self.connect(id);
                }
            }
        } else if password_ok {
            if let Dialog::Password { mut conn, cred_id, username, password, remember } =
                std::mem::replace(&mut self.dialog, Dialog::None)
            {
                if conn.id != 0 {
                    if remember && !password.is_empty() {
                        if let Err(e) = rdp::store_password(cred_id, &username, &password) {
                            self.error = Some(e);
                        }
                    }
                } else {
                    // Quick Connect: eingegebenen Benutzer übernehmen ("DOMÄNE\benutzer" oder UPN)
                    conn.username = username.trim().to_string();
                    conn.domain.clear();
                }
                self.start_session(&conn, Some(&password));
            }
        } else if save_folder {
            if let Dialog::Folder { id, parent, name, username, domain, password, delete_password, .. } =
                std::mem::replace(&mut self.dialog, Dialog::None)
            {
                let name = name.trim().to_string();
                let (username, domain) = (username.trim().to_string(), domain.trim().to_string());
                let folder_id = match id {
                    Some(id) => {
                        if let Some(f) = self.store.root.find_folder_mut(id) {
                            f.name = name;
                            f.username = username.clone();
                            f.domain = domain.clone();
                        }
                        id
                    }
                    None => {
                        let id = self.store.new_id();
                        let new =
                            Folder { id, name, username: username.clone(), domain: domain.clone(), ..Default::default() };
                        if let Some(p) = self.store.root.find_folder_mut(parent) {
                            p.folders.push(new);
                        }
                        id
                    }
                };
                // Passwort nur zusammen mit einem Benutzer behalten
                if delete_password || username.is_empty() {
                    rdp::delete_password(folder_id);
                }
                if !password.is_empty() && !username.is_empty() {
                    if let Err(e) = rdp::store_password(folder_id, &model::join_user(&domain, &username), &password) {
                        self.error = Some(e);
                    }
                }
                self.save();
            }
        } else if let Some((folder, conn)) = delete {
            // Mit Sicherungen bleiben die Passwörter im Tresor, damit eine Wiederherstellung sie findet
            // (aufräumen über „Gespeicherte Passwörter“ → verwaiste Einträge).
            let keep_pw = self.store.backup_enabled;
            if folder.is_some() {
                self.backup_now(Some("vor Löschen"));
            }
            if let Some(id) = folder {
                if let Some(f) = self.store.root.remove_folder(id) {
                    if !keep_pw {
                        for_each_connection(&f, &mut |c| rdp::delete_password(c.id));
                        f.for_each_folder(&mut |sub| rdp::delete_password(sub.id));
                    }
                }
            }
            if let Some(id) = conn {
                self.store.root.remove_connection(id);
                if !keep_pw {
                    rdp::delete_password(id);
                }
            }
            self.dialog = Dialog::None;
            self.save();
        } else if close {
            self.dialog = Dialog::None;
        }
    }

    /// Eingebettete RDP-Controls an die aktuelle Tab-Fläche anpassen.
    fn layout_sessions(&mut self, ctx: &egui::Context, content: egui::Rect) {
        let ppp = ctx.pixels_per_point();
        self.scale = rdp::scale_percent(ppp);
        let to_px = |r: egui::Rect| {
            (
                (r.min.x * ppp).round() as i32,
                (r.min.y * ppp).round() as i32,
                (r.width() * ppp).round() as i32,
                (r.height() * ppp).round() as i32,
            )
        };
        let px = to_px(content);
        if px.2 > 50 && px.3 > 50 {
            self.content_px = px;
        }
        // Eingebettete Fenster liegen immer über egui. Menüs, Popups und Tooltips werden deshalb
        // aus dem RDP-Fenster ausgeschnitten; nur bei modalen Dialogen wird die Sitzung ausgeblendet.
        let modal_open = !matches!(self.dialog, Dialog::None) || self.overview || self.vault.busy();
        let popups = popup_rects(ctx, ppp);
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        let scale = self.scale;
        let split_px = self.split.zip(self.split_rect).map(|(id, r)| (id, to_px(r)));
        let mut any_cutout = false;
        let mut visible = Vec::new();
        for s in &mut self.sessions {
            if self.detached.iter().any(|(d, _)| *d == s.id) {
                continue; // liegt im eigenen Fenster
            }
            let target = if Some(s.id) == self.active {
                Some(px)
            } else {
                split_px.filter(|(id, _)| *id == s.id).map(|(_, p)| p)
            };
            let target = target.filter(|p| {
                *s.state() == State::Connected && !modal_open && !minimized && p.2 > 0 && p.3 > 0
            });
            if let Some(p) = target {
                let cutouts = cutouts_for(&popups, p);
                any_cutout |= !cutouts.is_empty();
                s.place(p.0, p.1, p.2, p.3, scale);
                s.set_cutouts(&cutouts);
                visible.push(s.id);
            } else {
                s.hide();
            }
        }
        // Vorschaubilder für die Übersicht alle paar Sekunden erneuern
        for id in visible {
            if self.thumbs.get(&id).is_none_or(|(_, t)| t.elapsed() > Duration::from_secs(5)) {
                self.capture_thumb(id);
            }
        }
        if any_cutout {
            // Popups können sich bewegen/schließen -> Ausschnitt zeitnah aktualisieren
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    fn update_sessions(&mut self) {
        let mut ended = Vec::new();
        for s in &mut self.sessions {
            let was_connected = *s.state() == State::Connected;
            s.update();
            match s.state() {
                State::Closed => ended.push(s.id),
                State::Connected if !was_connected && Some(s.id) == self.active => s.focus(),
                _ => {}
            }
            // Verbindungsprotokoll: Verbindungsaufbau und Fehler vermerken
            if let Some((lid, seen)) = self.log_ids.get_mut(&s.id) {
                match s.state() {
                    State::Connected if *seen == 0 => {
                        connlog::event(lid, "connected", "");
                        *seen = 1;
                    }
                    State::Failed(msg) if *seen != 2 => {
                        connlog::event(lid, "failed", msg);
                        *seen = 2;
                    }
                    _ => {}
                }
            }
        }
        for id in ended {
            self.close_session(id);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.update_sessions();

        // --- Sperre: nur den Sperrbildschirm zeigen, Sitzungen ausblenden (sie bleiben verbunden)
        let lock = &self.store.lock;
        if !self.lock.locked && lock.enabled() && lock.idle_minutes > 0 {
            if rdp::idle_seconds() >= lock.idle_minutes as u64 * 60 {
                self.lock_now();
            } else {
                ctx.request_repaint_after(Duration::from_secs(15));
            }
        }
        if self.lock.locked {
            if !self.store.lock.enabled() {
                self.lock.locked = false; // Sperre wurde ausgeschaltet
            }
            for s in &mut self.sessions {
                s.hide();
            }
            self.detached_windows(&ctx);
            if self.lock.ui(ui, &self.store.lock, self.parent) && std::mem::take(&mut self.export_after_unlock) {
                self.dialog = Dialog::Export { use_master: false, master: String::new(), master_repeat: String::new() };
            }
            ctx.request_repaint_after(Duration::from_secs(1));
            return;
        }

        // Klick in die egui-Oberfläche -> Tastaturfokus zurück an das Hauptfenster.
        if !self.parent.is_invalid() && ctx.input(|i| i.pointer.any_pressed()) {
            rdp::focus_window(self.parent);
        }

        let mut actions = Vec::new();
        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui, &mut actions));
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(err) = self.error.clone() {
                    ui.colored_label(Color32::from_rgb(220, 70, 70), format!("⚠ {err}"));
                    if ui.small_button("OK").clicked() {
                        self.error = None;
                    }
                } else if let Some(info) = self.info.clone() {
                    ui.colored_label(Color32::from_rgb(60, 170, 90), format!("✔ {info}"));
                    if ui.small_button("OK").clicked() {
                        self.info = None;
                    }
                } else {
                    ui.weak(format!("{} offene Sitzung(en)", self.sessions.len()));
                }
                let mirrored = self.sessions.iter().filter(|s| s.is_mirror()).count();
                if mirrored > 0 {
                    ui.separator();
                    ui.colored_label(Color32::from_rgb(235, 135, 30), format!("📡 Gleichzeitige Eingabe: {mirrored} Sitzung(en)"));
                    if ui.small_button("beenden").clicked() {
                        for s in &mut self.sessions {
                            if let Kind::Ssh(t) = &mut s.kind {
                                t.mirror = false;
                            }
                        }
                    }
                }
                if !self.tunnels.is_empty() {
                    ui.separator();
                    let list: Vec<String> =
                        self.tunnels.iter().map(|t| format!("{} (localhost:{})", t.label, t.local_port)).collect();
                    ui.weak(format!("🔀 {} SSH-Tunnel", self.tunnels.len())).on_hover_text(list.join("\n"));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(f) = &self.upd_found
                        && ui
                            .add(egui::Button::new(
                                RichText::new(format!("⬆ Version {} verfügbar", update::version_text(f.version)))
                                    .color(Color32::from_rgb(60, 170, 90)),
                            ))
                            .on_hover_text("Update installieren …")
                            .clicked()
                    {
                        actions.push(Action::Updates);
                    }
                    let r = ui
                        .add(egui::Label::new(RichText::new(concat!("Version ", env!("CARGO_PKG_VERSION"))).weak()).sense(egui::Sense::click()))
                        .on_hover_text("Versionshinweise anzeigen");
                    if r.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if r.clicked() {
                        actions.push(Action::ReleaseNotes);
                    }
                });
            });
        });
        egui::Panel::left("tree")
            .resizable(true)
            .default_size(270.0)
            .min_size(180.0)
            .show(ui, |ui| self.tree_panel(ui, &mut actions));
        let content = egui::CentralPanel::default().show(ui, |ui| self.tabs_and_content(ui)).inner;

        self.forward_mirror_input();
        self.dialogs(&ctx);
        // Passwort-Tresor: Masterkennwort-Abfrage, Ergebnisse
        for ev in self.vault.update(&ctx, &self.store.vault) {
            match ev {
                vault::Event::Fetched(vault::Purpose::Connect(id), c) => self.connect_with(id, Some(c)),
                vault::Event::Fetched(vault::Purpose::Test, c) => {
                    let msg = format!(
                        "✔ Gefunden: Benutzer „{}“, Passwort mit {} Zeichen",
                        c.username,
                        c.password.chars().count()
                    );
                    match &mut self.dialog {
                        Dialog::Vault { result, .. } => *result = Some((true, msg)),
                        _ => self.info = Some(msg),
                    }
                }
                vault::Event::Failed(p, e) => match (&mut self.dialog, p) {
                    (Dialog::Vault { result, .. }, vault::Purpose::Test) => *result = Some((false, e)),
                    _ => self.error = Some(e),
                },
            }
        }
        if let Some(v) = &mut self.log_view {
            let (open, changed) = v.ui(&ctx, &mut self.store.conn_log);
            if !open {
                self.log_view = None;
            }
            if changed {
                self.save();
            }
        }
        self.transfers_window(&ctx);
        self.wake_window(&ctx);
        self.poll_updates(&ctx);
        self.notes_window(&ctx);
        self.palette_ui(&ctx);
        self.update_tunnels(&ctx);
        for b in self.sftp.values_mut() {
            b.update();
            if b.is_busy() {
                ctx.request_repaint_after(Duration::from_millis(200));
            }
        }
        self.detached_windows(&ctx);
        self.handle_dropped_files(&ctx);
        if let Some((id, pw)) = self.pending_pw_save.take() {
            self.save_session_password(id, &pw);
        }
        for t in &mut self.shared {
            t.poll(&ctx);
        }
        if !self.shared.is_empty() {
            ctx.request_repaint_after(Duration::from_secs(30));
        }
        for a in actions {
            self.handle(a);
        }
        if std::mem::take(&mut self.quick_go) {
            let target = self.quick.clone();
            self.quick_connect(&target);
        }
        if let Some((id, cmd)) = self.session_cmd.take() {
            self.run_session_cmd(id, cmd);
        }
        // „Alle verbinden“: nacheinander öffnen; bei einem Passwortdialog warten, bis er zu ist
        while matches!(self.dialog, Dialog::None) {
            let Some(id) = self.pending_connects.pop_front() else { break };
            self.connect(id);
        }
        self.layout_sessions(&ctx, content);

        if self.sessions.iter().any(|s| *s.state() == State::Connecting) {
            ctx.request_repaint_after(Duration::from_millis(200));
        } else if !self.sessions.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(500));
        } else if self.store.check_reachability {
            // abgelaufene Erreichbarkeits-Ergebnisse auch ohne Mausbewegung erneuern
            ctx.request_repaint_after(Duration::from_secs(15));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // offene Tabs fürs nächste Mal merken (aktiver zuletzt, damit er wieder vorne ist)
        let mut tabs: Vec<u64> = self
            .sessions
            .iter()
            .filter(|s| s.conn.id != 0 && Some(s.id) != self.active && self.find_any(s.conn.id).is_some())
            .map(|s| s.conn.id)
            .collect();
        if let Some(a) = self.active_session().map(|s| s.conn.id).filter(|&id| id != 0 && self.find_any(id).is_some()) {
            tabs.push(a);
        }
        tabs.dedup();
        let tabs = if self.store.restore_tabs { tabs } else { Vec::new() };
        if tabs != self.store.open_tabs {
            self.store.open_tabs = tabs;
            self.save();
        }
        for (_, (lid, _)) in self.log_ids.drain() {
            connlog::event(&lid, "closed", "Programm beendet");
        }
        for s in &mut self.sessions {
            s.close();
        }
        self.sessions.clear();
    }
}

// ---------------------------------------------------------------------------
// Verbindungs-Formular (Abschnitte wie die Konfiguration in mRemoteNG)
// ---------------------------------------------------------------------------

fn section(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    CollapsingHeader::new(RichText::new(title).strong()).default_open(true).show(ui, |ui| {
        egui::Grid::new(title).num_columns(2).spacing([14.0, 6.0]).min_col_width(200.0).show(ui, add);
    });
}

fn yes_no(ui: &mut egui::Ui, label: &str, value: &mut bool) {
    ui.label(label);
    ui.horizontal(|ui| {
        ui.radio_value(value, true, "Ja");
        ui.radio_value(value, false, "Nein");
    });
    ui.end_row();
}

fn combo<T: Copy + PartialEq>(ui: &mut egui::Ui, id: &str, value: &mut T, items: &[T], label: impl Fn(T) -> String) {
    egui::ComboBox::from_id_salt(id).selected_text(label(*value)).width(300.0).show_ui(ui, |ui| {
        for &it in items {
            ui.selectable_value(value, it, label(it));
        }
    });
}

fn text(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) {
    ui.label(label);
    ui.add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(300.0));
    ui.end_row();
}

#[allow(clippy::too_many_arguments)]
fn connection_form(
    ui: &mut egui::Ui,
    c: &mut Connection,
    folder: &mut u64,
    folders: &[(u64, String)],
    cred_source: &Option<(String, String)>,
    ssh_hosts: &[(u64, String)],
    password: &mut String,
    has_password: bool,
    delete_password: &mut bool,
    vault_kind: VaultKind,
) {
    // Fehlermeldung von „MAC ermitteln“ (bleibt, bis die Adresse ermittelt wurde)
    let mac_id = egui::Id::unique("conn_form_mac_error");
    section(ui, "Anzeige", |ui| {
        text(ui, "Name", &mut c.name, "Anzeigename");
        ui.label("Beschreibung");
        ui.add(egui::TextEdit::multiline(&mut c.description).desired_rows(2).desired_width(300.0));
        ui.end_row();
        ui.label("Symbol");
        combo(ui, "icon", &mut c.icon, &IconKind::ALL, |i| format!("{} {}", i.glyph(), i.label()));
        ui.end_row();
        text(ui, "Tags", &mut c.tags, "z. B. prod, linux, dc");
        ui.label("Farbe");
        egui::ComboBox::from_id_salt("color").selected_text(c.color.label()).width(300.0).show_ui(ui, |ui| {
            for col in TabColor::ALL {
                let label = match col.rgb() {
                    Some((r, g, b)) => RichText::new(format!("■ {}", col.label())).color(Color32::from_rgb(r, g, b)),
                    None => RichText::new(col.label()),
                };
                ui.selectable_value(&mut c.color, col, label);
            }
        })
        .response
        .on_hover_text("Färbt Tab und Baumeintrag ein – z. B. Rot für Produktivsysteme");
        ui.end_row();
        ui.label("Notizen");
        ui.add(
            egui::TextEdit::multiline(&mut c.memo)
                .hint_text("Wartungsfenster, Ansprechpartner, Besonderheiten …")
                .desired_rows(3)
                .desired_width(300.0),
        );
        ui.end_row();
        ui.label("Ordner");
        let current = folders.iter().find(|(id, _)| id == folder).map(|(_, n)| n.clone()).unwrap_or_default();
        egui::ComboBox::from_id_salt("folder").selected_text(current).width(300.0).show_ui(ui, |ui| {
            for (id, name) in folders {
                ui.selectable_value(folder, *id, name);
            }
        });
        ui.end_row();
    });

    section(ui, "Verbindung", |ui| {
        let hint = if c.protocol.is_web() { "server.domain.local oder vollständige URL" } else { "server.domain.local" };
        text(ui, "Hostname/IP *", &mut c.host, hint);
        ui.label("Über SSH-Sprunghost");
        let current = ssh_hosts
            .iter()
            .find(|(id, _)| *id == c.via_ssh)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| if c.via_ssh == 0 { "– direkt verbinden –".into() } else { "(Verbindung fehlt)".into() });
        egui::ComboBox::from_id_salt("via_ssh").selected_text(current).width(300.0).show_ui(ui, |ui| {
            ui.selectable_value(&mut c.via_ssh, 0, "– direkt verbinden –");
            for (id, name) in ssh_hosts {
                ui.selectable_value(&mut c.via_ssh, *id, name);
            }
        })
        .response
        .on_hover_text(if c.protocol == Protocol::Ssh {
            "Verbindet über diese SSH-Verbindung als Sprunghost (ssh -J)."
        } else {
            "Baut zuerst einen SSH-Tunnel über diese Verbindung auf und verbindet dann über localhost – \
             für Server, die nur über einen Sprunghost erreichbar sind. Der Hostname wird vom Sprunghost aus aufgelöst."
        });
        ui.end_row();
        ui.label("MAC-Adresse (Wake-on-LAN)");
        ui.horizontal(|ui| {
            let valid = c.mac.trim().is_empty() || wol::parse_mac(&c.mac).is_some();
            let mut edit = egui::TextEdit::singleline(&mut c.mac).hint_text("optional, z. B. 00:1A:2B:3C:4D:5E").desired_width(200.0);
            if !valid {
                edit = edit.text_color(Color32::from_rgb(220, 70, 70));
            }
            ui.add(edit).on_hover_text("Damit lässt sich der Rechner per Rechtsklick > Aufwecken einschalten");
            if ui
                .add_enabled(!c.host.trim().is_empty() && !c.host.contains("://"), egui::Button::new("Ermitteln"))
                .on_hover_text("Fragt die MAC-Adresse per ARP ab – nur wenn der Rechner läuft und im selben Netz liegt")
                .clicked()
            {
                match wol::lookup_mac(&c.host) {
                    Ok(m) => {
                        c.mac = m;
                        ui.data_mut(|d| d.remove::<String>(mac_id));
                    }
                    Err(e) => {
                        ui.data_mut(|d| d.insert_temp(mac_id, e));
                    }
                }
            }
        });
        ui.end_row();
        if let Some(e) = ui.data(|d| d.get_temp::<String>(mac_id)) {
            ui.label("");
            ui.colored_label(Color32::from_rgb(220, 140, 40), e.as_str());
            ui.end_row();
        }
        if c.protocol.is_web() {
            return; // Anmeldung erfolgt auf der Webseite
        }
        ui.label("Anmeldedaten");
        ui.vertical(|ui| {
            ui.checkbox(&mut c.inherit_credentials, "Vom Ordner übernehmen");
            if c.inherit_credentials {
                match cred_source {
                    Some((folder, user)) => ui.weak(format!("🔑 {user} (aus Ordner „{folder}“)")),
                    None => ui.colored_label(
                        Color32::from_rgb(220, 140, 40),
                        "Kein übergeordneter Ordner hat Anmeldedaten – es gelten die eigenen unten.",
                    ),
                };
            }
        });
        ui.end_row();
        if vault_kind != VaultKind::None || !c.vault_entry.is_empty() {
            ui.label("Tresor-Eintrag");
            ui.add(
                egui::TextEdit::singleline(&mut c.vault_entry)
                    .hint_text(match vault_kind {
                        VaultKind::Bitwarden => "Name oder Element-ID in Bitwarden",
                        VaultKind::None => "Tresor ist nicht eingerichtet",
                        _ => "Titel des KeePass-Eintrags",
                    })
                    .desired_width(300.0),
            )
            .on_hover_text("Benutzer und Passwort kommen beim Verbinden aus diesem Eintrag (statt aus dem Windows-Tresor). Leer = wie bisher.");
            ui.end_row();
        }
        if c.inherit_credentials && cred_source.is_some() {
            return;
        }
        if c.protocol != Protocol::Vnc {
            text(ui, "Benutzername", &mut c.username, "");
        }
        ui.label("Passwort");
        ui.add(
            egui::TextEdit::singleline(password)
                .password(true)
                .hint_text(if has_password { "•••••••• (gespeichert)" } else { "leer = beim Verbinden fragen" })
                .desired_width(300.0),
        )
        .on_hover_text("Wird verschlüsselt in der Windows-Anmeldeinformationsverwaltung gespeichert, nicht in der Konfigurationsdatei.");
        ui.end_row();
        if has_password {
            ui.label("");
            ui.checkbox(delete_password, "Gespeichertes Passwort entfernen");
            ui.end_row();
        }
        if c.protocol == Protocol::Rdp {
            text(ui, "Domäne", &mut c.domain, "");
        }
    });

    section(ui, "Protokoll", |ui| {
        ui.label("Protokoll");
        let before = c.protocol;
        ui.horizontal(|ui| {
            for p in Protocol::ALL {
                ui.radio_value(&mut c.protocol, p, p.label());
            }
        });
        ui.end_row();
        if before != c.protocol {
            // Standardport und Symbol mitwechseln, sofern nicht angepasst
            if c.port == before.default_port() {
                c.port = c.protocol.default_port();
            }
            let default_icon = |p: Protocol| match p {
                Protocol::Ssh => IconKind::Linux,
                Protocol::Http | Protocol::Https => IconKind::Web,
                _ => IconKind::Server,
            };
            if c.icon == default_icon(before) {
                c.icon = default_icon(c.protocol);
            }
        }
        ui.label("Port");
        ui.add(egui::DragValue::new(&mut c.port).range(1..=65535));
        ui.end_row();
        if c.protocol.is_web() {
            text(ui, "Pfad", &mut c.web_path, "optional, z. B. /login.html");
            yes_no(ui, "Zertifikatsfehler ignorieren", &mut c.ignore_cert_errors);
            ui.label("Adresse");
            ui.weak(c.web_url());
            ui.end_row();
            return;
        }
        if c.protocol == Protocol::Vnc {
            return;
        }
        if c.protocol == Protocol::Ssh {
            ui.label("Schlüsseldatei");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut c.ssh_key_file).hint_text("optional, z. B. ~/.ssh/id_ed25519").desired_width(260.0));
                if ui.button("…").on_hover_text("Datei auswählen").clicked() {
                    let start = std::env::var_os("USERPROFILE").map(|p| std::path::PathBuf::from(p).join(".ssh"));
                    let mut dlg = rfd::FileDialog::new().set_title("Private-Key-Datei wählen");
                    if let Some(dir) = start.filter(|d| d.exists()) {
                        dlg = dlg.set_directory(dir);
                    }
                    if let Some(p) = dlg.pick_file() {
                        c.ssh_key_file = p.display().to_string();
                    }
                }
            });
            ui.end_row();
            yes_no(ui, "Agent-Weiterleitung (-A)", &mut c.ssh_agent_forward);
            ui.label("Portweiterleitungen");
            ui.vertical(|ui| {
                let mut remove = None;
                for (i, f) in c.forwards.iter_mut().enumerate() {
                    ui.push_id(("fwd", i), |ui| {
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt("kind").selected_text(f.kind.label()).width(110.0).show_ui(ui, |ui| {
                                for k in ForwardKind::ALL {
                                    ui.selectable_value(&mut f.kind, k, k.label());
                                }
                            });
                            ui.add(egui::DragValue::new(&mut f.listen_port).range(0..=65535)).on_hover_text(
                                if f.kind == ForwardKind::Remote { "Port auf dem Server" } else { "Port auf diesem PC (localhost)" },
                            );
                            if f.kind != ForwardKind::Dynamic {
                                ui.label("➡");
                                ui.add(egui::TextEdit::singleline(&mut f.target_host).hint_text("Ziel-Host").desired_width(110.0));
                                ui.add(egui::DragValue::new(&mut f.target_port).range(0..=65535)).on_hover_text("Ziel-Port");
                            }
                            if ui.small_button("🗑").clicked() {
                                remove = Some(i);
                            }
                        });
                    });
                }
                if let Some(i) = remove {
                    c.forwards.remove(i);
                }
                if ui.small_button("➕ Weiterleitung").on_hover_text("z. B. lokal 13389 ➡ intern-srv:3389, dann RDP auf localhost:13389").clicked() {
                    c.forwards.push(Forward { listen_port: 0, target_port: 3389, ..Default::default() });
                }
            });
            ui.end_row();
            text(ui, "Zusätzliche Argumente", &mut c.ssh_extra_args, "z. B. -o StrictHostKeyChecking=accept-new");
            return;
        }
        yes_no(ui, "Verwende Konsole (/admin)", &mut c.admin_session);
        ui.label("Serverauthentifizierung");
        combo(ui, "auth", &mut c.auth_level, &[AuthLevel::AlwaysConnect, AuthLevel::Warn, AuthLevel::DontConnect], |a| {
            a.label().to_string()
        });
        ui.end_row();
        ui.label("Minuten zum Leerlauf");
        ui.add(egui::DragValue::new(&mut c.idle_minutes).range(0..=240)).on_hover_text("0 = kein Timeout");
        ui.end_row();
        text(ui, "Lastausgleichsinfo", &mut c.load_balance_info, "optional");
        yes_no(ui, "Verwende CredSSP (NLA)", &mut c.use_credssp);
    });

    match c.protocol {
        Protocol::Ssh => {
            ui.add_space(4.0);
            ui.weak("SSH nutzt den Windows-OpenSSH-Client: ~/.ssh/config, known_hosts und ssh-agent gelten wie gewohnt. \
                     Ein gespeichertes Passwort wird beim Prompt „password:“ automatisch eingegeben.");
            return;
        }
        Protocol::Vnc => {
            ui.add_space(4.0);
            ui.weak("VNC läuft im eingebauten Viewer (ohne Passwort oder mit VNC-Passwort; der Benutzername wird nicht verwendet). \
                     Verschlüsselte Verfahren wie RealVNC-Verschlüsselung/VeNCrypt werden nicht unterstützt.");
            return;
        }
        Protocol::Http | Protocol::Https => {
            ui.add_space(4.0);
            ui.weak("Web-Verbindungen öffnen sich im eingebauten Edge-WebView2 – praktisch für iDRAC/iLO, Drucker, Switches. \
                     Anmeldedaten gibst du auf der Webseite ein.");
            return;
        }
        Protocol::Rdp => {}
    }

    section(ui, "Gateway", |ui| {
        ui.label("Verwende Gateway");
        combo(ui, "gw", &mut c.gateway_usage, &[GatewayUsage::Never, GatewayUsage::Always, GatewayUsage::Detect], |g| {
            g.label().to_string()
        });
        ui.end_row();
        if c.gateway_usage != GatewayUsage::Never {
            text(ui, "Gateway-Hostname", &mut c.gateway, "gateway.domain.de");
        }
    });

    section(ui, "Aussehen", |ui| {
        ui.label("Auflösung");
        combo(
            ui,
            "res",
            &mut c.resolution,
            &[Resolution::FitToPanel, Resolution::SmartSize, Resolution::Fullscreen, Resolution::Fixed],
            |r| r.label().to_string(),
        );
        ui.end_row();
        if c.resolution == Resolution::Fixed {
            ui.label("Breite × Höhe");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut c.fixed_width).range(640..=7680));
                ui.label("×");
                ui.add(egui::DragValue::new(&mut c.fixed_height).range(480..=4320));
            });
            ui.end_row();
        }
        if c.resolution == Resolution::FitToPanel {
            yes_no(ui, "Automatische Größenanpassung", &mut c.auto_resize);
        }
        ui.label("Alle Monitore verwenden");
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.radio_value(&mut c.multimon, true, "Ja");
                ui.radio_value(&mut c.multimon, false, "Nein");
            });
            if c.multimon {
                ui.weak("Öffnet in mstsc im Vollbild über alle Monitore statt im Tab.");
            }
        });
        ui.end_row();
        if c.multimon {
            text(ui, "Nur diese Monitore", &mut c.monitors, "z. B. 0,1 – leer = alle (Nummern: mstsc /l)");
        }
        ui.label("Farben");
        combo(ui, "colors", &mut c.colors, &[15, 16, 24, 32], |b| match b {
            15 => "32768 Farben (15 Bit)".into(),
            16 => "65536 Farben (16 Bit)".into(),
            24 => "16 Mio. Farben (24 Bit)".into(),
            _ => "Höchste Qualität (32 Bit)".into(),
        });
        ui.end_row();
        yes_no(ui, "Bitmaps zwischenspeichern", &mut c.bitmap_cache);
        yes_no(ui, "Hintergrundbild anzeigen", &mut c.wallpaper);
        yes_no(ui, "Themen anzeigen", &mut c.themes);
        yes_no(ui, "Schriftglättung", &mut c.font_smoothing);
        yes_no(ui, "Desktopgestaltung", &mut c.desktop_composition);
    });

    section(ui, "Umleitung", |ui| {
        yes_no(ui, "Tastenkombinationen", &mut c.redirect_keys);
        yes_no(ui, "Festplatten", &mut c.redirect_drives);
        yes_no(ui, "Drucker", &mut c.redirect_printers);
        yes_no(ui, "Ports", &mut c.redirect_ports);
        yes_no(ui, "Smartcards", &mut c.redirect_smartcards);
        yes_no(ui, "Zwischenablage", &mut c.redirect_clipboard);
        ui.label("Töne");
        combo(
            ui,
            "sound",
            &mut c.sound,
            &[SoundMode::BringToThisComputer, SoundMode::LeaveAtRemote, SoundMode::DoNotPlay],
            |s| s.label().to_string(),
        );
        ui.end_row();
    });
}

// ---------------------------------------------------------------------------

fn apply_theme(ctx: &egui::Context, mode: ThemeMode) {
    let (pref, sys) = match mode {
        ThemeMode::System => (egui::ThemePreference::System, egui::SystemTheme::SystemDefault),
        ThemeMode::Light => (egui::ThemePreference::Light, egui::SystemTheme::Light),
        ThemeMode::Dark => (egui::ThemePreference::Dark, egui::SystemTheme::Dark),
    };
    ctx.set_theme(pref);
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(sys));
}

/// Sucht eine gespeicherte Verbindung mit gleichem Host (Name oder Hostname) und Port.
fn find_by_host(f: &Folder, host: &str, port: u16) -> Option<u64> {
    f.connections
        .iter()
        .find(|c| {
            c.port == port && (c.host.eq_ignore_ascii_case(host) || c.name.eq_ignore_ascii_case(host))
        })
        .map(|c| c.id)
        .or_else(|| f.folders.iter().find_map(|sub| find_by_host(sub, host, port)))
}

fn for_each_connection(f: &Folder, cb: &mut dyn FnMut(&Connection)) {
    f.connections.iter().for_each(&mut *cb);
    for sub in &f.folders {
        for_each_connection(sub, cb);
    }
}

/// Verbindung sichtbar? Suchbegriffe (inkl. Ordnerpfad) und alle gewählten Tags müssen passen.
fn conn_visible(c: &Connection, terms: &[String], tags: &BTreeSet<String>, path: &str) -> bool {
    c.matches_terms(terms, path) && tags.iter().all(|t| c.has_tag(t))
}

fn folder_has_match(f: &Folder, terms: &[String], tags: &BTreeSet<String>, path: &str) -> bool {
    f.connections.iter().any(|c| conn_visible(c, terms, tags, path))
        || f.folders.iter().any(|sub| folder_has_match(sub, terms, tags, &format!("{path} / {}", sub.name)))
}

/// Alle verwendeten Tags (eigene und gemeinsame Verbindungen)
fn all_tags(roots: &[&Folder]) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for r in roots {
        for_each_connection(r, &mut |c| set.extend(c.tag_list()));
    }
    set
}

/// Zeichnet den Verbindungsbaum inkl. Drag & Drop und „Verschieben nach“.
struct Tree<'a> {
    root: &'a Folder,
    /// Alle Ordner als (ID, Pfad), inkl. oberster Ebene
    paths: &'a [(u64, String)],
    /// Suchbegriffe (klein geschrieben)
    terms: &'a [String],
    /// gewählte Tags (alle müssen passen)
    tags: &'a BTreeSet<String>,
    /// Erreichbarkeitsprüfung (None = ausgeschaltet)
    reach: Option<&'a reach::Reach>,
    /// Gemeinsame Liste: nicht bearbeiten, verschieben oder löschen
    read_only: bool,
    tools: &'a [model::ExtTool],
    /// Favoriten (für Markierung und Kontextmenü)
    favorites: &'a [u64],
}

/// Verbindung in einer der Listen finden: (Wurzel, Ordner, Verbindung)
fn locate_connection<'a>(roots: &[&'a Folder], id: u64) -> Option<(&'a Folder, &'a Folder, &'a Connection)> {
    roots.iter().find_map(|r| {
        let c = r.find_connection(id)?;
        let parent = r.parent_of_connection(id).and_then(|p| r.find_folder(p)).unwrap_or(r);
        Some((*r, parent, c))
    })
}

impl Tree<'_> {
    /// Macht `r` zur Drag-Quelle für `item` und zeigt beim Ziehen den Namen am Mauszeiger.
    fn drag_source(&self, ui: &egui::Ui, r: &egui::Response, item: Item, label: &str) {
        r.dnd_set_drag_payload(item);
        if r.dragged()
            && let Some(pos) = ui.ctx().pointer_interact_pos()
        {
            let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, ui.scope_id().with("dnd_ghost")));
            let galley = painter.layout_no_wrap(label.to_owned(), egui::FontId::proportional(14.0), ui.visuals().text_color());
            let rect = egui::Rect::from_min_size(pos + egui::vec2(14.0, 6.0), galley.size()).expand(4.0);
            painter.rect(
                rect,
                3.0,
                ui.visuals().window_fill,
                ui.visuals().window_stroke,
                egui::StrokeKind::Outside,
            );
            painter.galley(rect.min + egui::vec2(4.0, 4.0), galley, ui.visuals().text_color());
        }
    }

    /// Hebt `r` als Ablageziel für Ordner `target` hervor und verschiebt beim Loslassen.
    fn drop_target(&self, ui: &egui::Ui, r: &egui::Response, target: u64, actions: &mut Vec<Action>) {
        if let Some(item) = r.dnd_hover_payload::<Item>() {
            let ok = self.root.can_move(*item, target);
            let color = if ok { ui.visuals().selection.stroke.color } else { ui.visuals().error_fg_color };
            ui.painter().rect_stroke(r.rect, 3.0, egui::Stroke::new(1.5, color), egui::StrokeKind::Inside);
            if !ok {
                ui.ctx().set_cursor_icon(egui::CursorIcon::NoDrop);
            }
        }
        if let Some(item) = r.dnd_release_payload::<Item>()
            && self.root.can_move(*item, target)
        {
            actions.push(Action::Move(*item, target));
        }
    }

    /// Untermenü „Verschieben nach“ mit allen erlaubten Zielordnern.
    fn move_menu(&self, ui: &mut egui::Ui, item: Item, actions: &mut Vec<Action>) {
        ui.menu_button("📂 Verschieben nach", |ui| {
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                for (id, path) in self.paths {
                    if self.root.can_move(item, *id) && ui.button(path).clicked() {
                        actions.push(Action::Move(item, *id));
                    }
                }
            });
        });
    }

    fn filtering(&self) -> bool {
        !self.terms.is_empty() || !self.tags.is_empty()
    }

    fn contents(&self, ui: &mut egui::Ui, folder: &Folder, path: &str, selected: &mut Option<u64>, actions: &mut Vec<Action>) {
        for sub in &folder.folders {
            let sub_path = if path.is_empty() { sub.name.clone() } else { format!("{path} / {}", sub.name) };
            if self.filtering() && !folder_has_match(sub, self.terms, self.tags, &sub_path) {
                continue;
            }
            let key = if sub.has_credentials() { " 🔑" } else { "" };
            let mut header = CollapsingHeader::new(RichText::new(format!("📁 {}{key}", sub.name)).strong())
                .id_salt(("folder", sub.id))
                .default_open(true);
            if self.filtering() {
                header = header.open(Some(true));
            }
            let resp = header.show(ui, |ui| self.contents(ui, sub, &sub_path, selected, actions));
            let head = if self.read_only {
                resp.header_response
            } else {
                let head = resp.header_response.interact(egui::Sense::drag());
                self.drag_source(ui, &head, Item::Folder(sub.id), &format!("📁 {}", sub.name));
                self.drop_target(ui, &head, sub.id, actions);
                head
            };
            self.folder_menu(&head, sub, actions);
        }
        for c in &folder.connections {
            if !conn_visible(c, self.terms, self.tags, path) {
                continue;
            }
            self.connection_row(ui, folder, c, selected, actions);
        }
    }

    fn folder_menu(&self, head: &egui::Response, sub: &Folder, actions: &mut Vec<Action>) {
        head.context_menu(|ui| {
            let direct = count_connections(sub, false);
            let all = count_connections(sub, true);
            if ui.add_enabled(direct > 0, egui::Button::new(format!("▶ Alle verbinden ({direct})"))).clicked() {
                actions.push(Action::ConnectFolder(sub.id, false));
            }
            if all > direct && ui.button(format!("▶ Alle verbinden inkl. Unterordner ({all})")).clicked() {
                actions.push(Action::ConnectFolder(sub.id, true));
            }
            if self.read_only {
                return;
            }
            ui.separator();
            if ui.button("➕ Neue Verbindung hier").clicked() {
                actions.push(Action::NewConnection(sub.id));
            }
            if ui.button("📁 Unterordner").clicked() {
                actions.push(Action::NewFolder(sub.id));
            }
            if ui.button("✏ Bearbeiten / 🔑 Anmeldedaten").clicked() {
                actions.push(Action::RenameFolder(sub.id));
            }
            self.move_menu(ui, Item::Folder(sub.id), actions);
            ui.separator();
            if ui.button("🗑 Ordner löschen").clicked() {
                actions.push(Action::DeleteFolder(sub.id));
            }
        });
    }

    fn connection_row(
        &self,
        ui: &mut egui::Ui,
        folder: &Folder,
        c: &Connection,
        selected: &mut Option<u64>,
        actions: &mut Vec<Action>,
    ) {
        let note = if c.memo.trim().is_empty() { "" } else { "  📝" };
        let is_fav = self.favorites.contains(&c.id);
        let star = if is_fav { " ⭐" } else { "" };
        let label = format!("{} {}{star}{note}", c.icon.glyph(), c.display_name());
        let (dot, r) = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                (dot, ui.selectable_label(*selected == Some(c.id), &label).interact(egui::Sense::drag()))
            })
            .inner;
        if let Some((cr, cg, cb)) = c.color.rgb() {
            // Farbmarkierung links neben dem Eintrag
            let bar = egui::Rect::from_min_size(egui::pos2(dot.min.x - 5.0, r.rect.min.y + 2.0), egui::vec2(3.0, r.rect.height() - 4.0));
            ui.painter().rect_filled(bar, 1.0, Color32::from_rgb(cr, cg, cb));
        }
        let status = self.status_dot(ui, c, dot, &r);
        if !self.read_only {
            self.drag_source(ui, &r, Item::Connection(c.id), &label);
            // Ablegen auf einer Verbindung → in deren Ordner
            self.drop_target(ui, &r, folder.id, actions);
        }
        if r.clicked() {
            *selected = Some(c.id);
        }
        if r.double_clicked() {
            actions.push(Action::Connect(c.id));
        }
        r.context_menu(|ui| {
            *selected = Some(c.id);
            if ui.button("▶ Verbinden (Tab)").clicked() {
                actions.push(Action::Connect(c.id));
            }
            let external = match c.protocol {
                Protocol::Rdp => Some("↗ Extern verbinden (mstsc)"),
                Protocol::Ssh => Some("↗ Extern verbinden (Konsole)"),
                Protocol::Http | Protocol::Https => Some("↗ Im Standardbrowser öffnen"),
                Protocol::Vnc => None,
            };
            if let Some(label) = external {
                if ui.button(label).clicked() {
                    actions.push(Action::ConnectExternal(c.id));
                }
            }
            if c.protocol == Protocol::Rdp
                && !c.multimon
                && ui.button("🖥 Auf allen Monitoren (mstsc)").on_hover_text("Vollbild über alle Monitore").clicked()
            {
                actions.push(Action::ConnectAllMonitors(c.id));
            }
            if !c.mac.trim().is_empty() {
                if ui.button("⏰ Aufwecken (Wake-on-LAN)").clicked() {
                    actions.push(Action::Wake(c.id, false));
                }
                if ui.button("⏰ Aufwecken und verbinden").on_hover_text("Weckt den Rechner und verbindet, sobald er antwortet").clicked() {
                    actions.push(Action::Wake(c.id, true));
                }
            }
            tools_menu(ui, self.tools, c.id, actions);
            if ui.button(if is_fav { "⭐ Aus Favoriten entfernen" } else { "⭐ Zu Favoriten" }).clicked() {
                actions.push(Action::ToggleFavorite(c.id));
            }
            if ui.button(if c.memo.trim().is_empty() { "📝 Notizen…" } else { "📝 Notizen anzeigen…" }).clicked() {
                actions.push(Action::EditNotes(c.id));
            }
            ui.separator();
            if self.read_only {
                if ui.button("📋 In eigene Verbindungen kopieren…").clicked() {
                    actions.push(Action::DuplicateConnection(c.id));
                }
                return;
            }
            if ui.button("✏ Bearbeiten").clicked() {
                actions.push(Action::EditConnection(c.id));
            }
            if ui.button("📋 Duplizieren").clicked() {
                actions.push(Action::DuplicateConnection(c.id));
            }
            self.move_menu(ui, Item::Connection(c.id), actions);
            ui.separator();
            if ui.button("🗑 Löschen").clicked() {
                actions.push(Action::DeleteConnection(c.id));
            }
        });
        let (eff, cred_id) = self.root.resolve_credentials(c);
        let user = eff.full_username();
        let mut tip = format!("{} · {}:{}", c.protocol.label(), c.host, c.port);
        if !user.is_empty() {
            tip.push_str(&format!("\n{user}{}", if cred_id != c.id { " (vom Ordner)" } else { "" }));
        }
        if c.via_ssh != 0 {
            let via = self.root.find_connection(c.via_ssh).map(|j| j.display_name().to_string()).unwrap_or_else(|| "?".into());
            tip.push_str(&format!("\n🔀 über SSH: {via}"));
        }
        if !c.tags.trim().is_empty() {
            tip.push_str(&format!("\n#{}", c.tag_list().join("  #")));
        }
        if !c.description.is_empty() {
            tip.push_str(&format!("\n{}", c.description));
        }
        if !c.memo.trim().is_empty() {
            tip.push_str(&format!("\n\n📝 {}", c.memo.trim()));
        }
        if let Some(s) = status {
            tip.push_str(&format!("\n\n{s}"));
        }
        r.on_hover_text(tip);
    }

    /// Zeichnet den Status-Punkt vor einer Verbindung und fragt die Erreichbarkeit an –
    /// aber nur, wenn die Zeile tatsächlich sichtbar ist. Liefert den Text für den Tooltip.
    fn status_dot(&self, ui: &egui::Ui, c: &Connection, dot: egui::Rect, row: &egui::Response) -> Option<String> {
        let reach = self.reach?;
        if !ui.is_rect_visible(row.rect) {
            return None;
        }
        let center = egui::pos2(dot.center().x, row.rect.center().y);
        let gray = ui.visuals().weak_text_color();
        if c.protocol == Protocol::Rdp && c.gateway_usage == GatewayUsage::Always {
            ui.painter().circle_stroke(center, 3.5, egui::Stroke::new(1.0, gray));
            return Some("Status: über Gateway – nicht direkt prüfbar".into());
        }
        if c.via_ssh != 0 {
            ui.painter().circle_stroke(center, 3.5, egui::Stroke::new(1.0, gray));
            return Some("Status: über SSH-Tunnel – nicht direkt prüfbar".into());
        }
        let (status, busy) = reach.check(&c.host, c.port);
        let (color, text) = match &status {
            Some(reach::Status::Up(ms)) => (Color32::from_rgb(60, 180, 90), format!("Erreichbar ({ms} ms)")),
            Some(reach::Status::Down(why)) => (Color32::from_rgb(220, 70, 70), format!("Nicht erreichbar: {why}")),
            None => (gray, "Wird geprüft …".into()),
        };
        if status.is_some() {
            ui.painter().circle_filled(center, 4.0, color);
        } else {
            ui.painter().circle_stroke(center, 3.5, egui::Stroke::new(1.0, gray));
        }
        if busy && status.is_some() {
            // Neuprüfung läuft: dünner Ring
            ui.painter().circle_stroke(center, 5.5, egui::Stroke::new(1.0, gray));
        }
        Some(format!("Status: {text}"))
    }
}

/// Setzt in `out` (Kopie von `root`) bei allen Verbindungen die wirksamen Anmeldedaten ein
/// und merkt sich, unter welcher ID das Passwort liegt.
fn resolve_tree(root: &Folder, out: &mut Folder, cred_of: &mut std::collections::HashMap<u64, u64>) {
    for c in &mut out.connections {
        if c.inherit_credentials {
            let (eff, cred) = root.resolve_credentials(c);
            c.username = eff.username;
            c.domain = eff.domain;
            c.inherit_credentials = false;
            cred_of.insert(c.id, cred);
        }
    }
    for f in &mut out.folders {
        resolve_tree(root, f, cred_of);
    }
}

/// Untermenü „Werkzeuge“ für eine Verbindung
fn tools_menu(ui: &mut egui::Ui, tools: &[model::ExtTool], conn_id: u64, actions: &mut Vec<Action>) {
    ui.menu_button("🔧 Werkzeuge", |ui| {
        for (i, t) in tools.iter().enumerate() {
            let name = if t.name.trim().is_empty() { t.program.as_str() } else { t.name.as_str() };
            if ui.button(name).on_hover_text(format!("{} {}", t.program, t.args)).clicked() {
                actions.push(Action::RunTool(conn_id, i));
            }
        }
        if !tools.is_empty() {
            ui.separator();
        }
        if ui.button("Werkzeuge verwalten…").clicked() {
            actions.push(Action::ManageTools);
        }
    });
}

fn detached_viewport(id: u64) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("detached_session", id))
}

/// Sichtbare Popups/Menüs/Tooltips des Fensters in physischen Pixeln (mit Rand für den Schatten)
fn popup_rects(ctx: &egui::Context, ppp: f32) -> Vec<egui::Rect> {
    ctx.memory(|m| {
        m.areas()
            .visible_layer_ids()
            .into_iter()
            .filter(|l| l.order != egui::Order::Background)
            .filter_map(|l| m.area_rect(l.id))
            .map(|r| egui::Rect::from_min_max(r.min * ppp, r.max * ppp).expand(8.0 * ppp))
            .collect()
    })
}

/// Ausschnitte relativ zum eingebetteten Fenster an Position `p` (x, y, w, h)
fn cutouts_for(popups: &[egui::Rect], p: (i32, i32, i32, i32)) -> Vec<(i32, i32, i32, i32)> {
    let area = egui::Rect::from_min_size(egui::pos2(p.0 as f32, p.1 as f32), egui::vec2(p.2 as f32, p.3 as f32));
    popups
        .iter()
        .filter(|r| r.intersects(area))
        .map(|r| {
            let r = r.intersect(area);
            (
                (r.min.x - area.min.x).floor() as i32,
                (r.min.y - area.min.y).floor() as i32,
                r.width().ceil() as i32 + 1,
                r.height().ceil() as i32 + 1,
            )
        })
        .collect()
}

/// Alle Verbindungen mit ihrem Ordnerpfad
fn collect_with_path<'a>(f: &'a Folder, prefix: &str, out: &mut Vec<(&'a Connection, String)>) {
    for c in &f.connections {
        out.push((c, prefix.to_string()));
    }
    for sub in &f.folders {
        let p = if prefix.is_empty() { sub.name.clone() } else { format!("{prefix} / {}", sub.name) };
        collect_with_path(sub, &p, out);
    }
}

/// Host und Port aus einer URL ("https://idrac01:8443/x" → ("idrac01", 8443))
fn url_host_port(url: &str, default_port: u16) -> (String, u16) {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split('/').next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(v6) = authority.strip_prefix('[') {
        let (h, after) = v6.split_once(']').unwrap_or((v6, ""));
        let port = after.strip_prefix(':').and_then(|p| p.parse().ok()).unwrap_or(default_port);
        return (h.to_string(), port);
    }
    match authority.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.parse().unwrap()),
        _ => (authority.to_string(), default_port),
    }
}

/// Verbindung so umschreiben, dass sie über den lokalen Tunnel-Port läuft
fn via_local_port(mut conn: Connection, port: u16) -> Connection {
    if conn.name.trim().is_empty() {
        conn.name = conn.display_name().to_string();
    }
    match conn.host.split_once("://") {
        Some((scheme, rest)) => {
            let path = rest.find('/').map(|i| &rest[i..]).unwrap_or("");
            conn.host = format!("{scheme}://127.0.0.1:{port}{path}");
        }
        None => conn.host = "127.0.0.1".into(),
    }
    conn.port = port;
    conn
}

fn count_connections(f: &Folder, recursive: bool) -> usize {
    f.connections.len() + if recursive { f.folders.iter().map(|s| count_connections(s, true)).sum() } else { 0 }
}

fn collect_connections(f: &Folder, recursive: bool, out: &mut Vec<u64>) {
    out.extend(f.connections.iter().map(|c| c.id));
    if recursive {
        for s in &f.folders {
            collect_connections(s, true, out);
        }
    }
}

