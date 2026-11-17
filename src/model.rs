//! Datenmodell (Ordner + Verbindungen) und Speicherung als JSON.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

/// Symbol im Verbindungsbaum (wie "Symbol" in mRemoteNG).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum IconKind {
    #[default]
    Server,
    DomainController,
    Workstation,
    Database,
    Web,
    Linux,
    Other,
}

impl IconKind {
    pub const ALL: [IconKind; 7] = [
        IconKind::Server,
        IconKind::DomainController,
        IconKind::Workstation,
        IconKind::Database,
        IconKind::Web,
        IconKind::Linux,
        IconKind::Other,
    ];
    pub fn glyph(self) -> &'static str {
        match self {
            IconKind::Server => "🖥",
            IconKind::DomainController => "🏢",
            IconKind::Workstation => "💻",
            IconKind::Database => "🗄",
            IconKind::Web => "🌐",
            IconKind::Linux => "🐧",
            IconKind::Other => "🔌",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            IconKind::Server => "Server",
            IconKind::DomainController => "Domain Controller",
            IconKind::Workstation => "Arbeitsstation",
            IconKind::Database => "Datenbank",
            IconKind::Web => "Webserver",
            IconKind::Linux => "Linux",
            IconKind::Other => "Sonstiges",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Protocol {
    #[default]
    Rdp,
    Ssh,
    Vnc,
    Http,
    Https,
}

impl Protocol {
    pub const ALL: [Protocol; 5] = [Protocol::Rdp, Protocol::Ssh, Protocol::Vnc, Protocol::Http, Protocol::Https];

    pub fn label(self) -> &'static str {
        match self {
            Protocol::Rdp => "RDP",
            Protocol::Ssh => "SSH",
            Protocol::Vnc => "VNC",
            Protocol::Http => "HTTP",
            Protocol::Https => "HTTPS",
        }
    }
    pub fn default_port(self) -> u16 {
        match self {
            Protocol::Rdp => 3389,
            Protocol::Ssh => 22,
            Protocol::Vnc => 5900,
            Protocol::Http => 80,
            Protocol::Https => 443,
        }
    }
    pub fn is_web(self) -> bool {
        matches!(self, Protocol::Http | Protocol::Https)
    }
}

/// Farbe einer Verbindung (Tab und Baum), z. B. Rot für Produktivsysteme
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TabColor {
    #[default]
    None,
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Purple,
    Gray,
}

impl TabColor {
    pub const ALL: [TabColor; 8] = [
        TabColor::None,
        TabColor::Red,
        TabColor::Orange,
        TabColor::Yellow,
        TabColor::Green,
        TabColor::Blue,
        TabColor::Purple,
        TabColor::Gray,
    ];
    pub fn label(self) -> &'static str {
        match self {
            TabColor::None => "Keine",
            TabColor::Red => "Rot",
            TabColor::Orange => "Orange",
            TabColor::Yellow => "Gelb",
            TabColor::Green => "Grün",
            TabColor::Blue => "Blau",
            TabColor::Purple => "Lila",
            TabColor::Gray => "Grau",
        }
    }
    pub fn rgb(self) -> Option<(u8, u8, u8)> {
        match self {
            TabColor::None => None,
            TabColor::Red => Some((220, 60, 60)),
            TabColor::Orange => Some((235, 135, 30)),
            TabColor::Yellow => Some((215, 185, 20)),
            TabColor::Green => Some((50, 170, 80)),
            TabColor::Blue => Some((50, 120, 220)),
            TabColor::Purple => Some((150, 80, 200)),
            TabColor::Gray => Some((130, 130, 130)),
        }
    }
}

/// Art einer SSH-Portweiterleitung
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForwardKind {
    /// -L: lokaler Port → Ziel (vom Server aus gesehen)
    #[default]
    Local,
    /// -R: Port auf dem Server → Ziel (von hier aus gesehen)
    Remote,
    /// -D: SOCKS-Proxy auf einem lokalen Port
    Dynamic,
}

impl ForwardKind {
    pub const ALL: [ForwardKind; 3] = [ForwardKind::Local, ForwardKind::Remote, ForwardKind::Dynamic];
    pub fn label(self) -> &'static str {
        match self {
            ForwardKind::Local => "Lokal (-L)",
            ForwardKind::Remote => "Entfernt (-R)",
            ForwardKind::Dynamic => "SOCKS (-D)",
        }
    }
}

/// SSH-Portweiterleitung einer Verbindung
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Forward {
    pub kind: ForwardKind,
    /// Lokal/SOCKS: Port auf diesem PC; Entfernt: Port auf dem Server
    pub listen_port: u16,
    /// Ziel (nicht bei SOCKS)
    pub target_host: String,
    pub target_port: u16,
}

impl Forward {
    /// ssh-Argumente, z. B. ["-L", "127.0.0.1:13389:intern:3389"]
    pub fn args(&self) -> Option<[String; 2]> {
        if self.listen_port == 0 {
            return None;
        }
        let host = self.target_host.trim();
        let host = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
        match self.kind {
            ForwardKind::Dynamic => Some(["-D".into(), format!("127.0.0.1:{}", self.listen_port)]),
            _ if host.is_empty() || self.target_port == 0 => None,
            ForwardKind::Local => {
                Some(["-L".into(), format!("127.0.0.1:{}:{host}:{}", self.listen_port, self.target_port)])
            }
            ForwardKind::Remote => Some(["-R".into(), format!("{}:{host}:{}", self.listen_port, self.target_port)]),
        }
    }

    pub fn describe(&self) -> String {
        match self.kind {
            ForwardKind::Local => {
                format!("localhost:{} ➡ {}:{}", self.listen_port, self.target_host.trim(), self.target_port)
            }
            ForwardKind::Remote => {
                format!("Server:{} ➡ {}:{}", self.listen_port, self.target_host.trim(), self.target_port)
            }
            ForwardKind::Dynamic => format!("SOCKS-Proxy localhost:{}", self.listen_port),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthLevel {
    /// Immer verbinden, auch wenn die Serverauthentifizierung fehlschlägt
    AlwaysConnect,
    /// Warnen
    #[default]
    Warn,
    /// Nicht verbinden
    DontConnect,
}

impl AuthLevel {
    pub fn label(self) -> &'static str {
        match self {
            AuthLevel::AlwaysConnect => "Immer verbinden, auch wenn Authentifizierung fehlschlägt",
            AuthLevel::Warn => "Warnen, wenn Authentifizierung fehlschlägt",
            AuthLevel::DontConnect => "Nicht verbinden, wenn Authentifizierung fehlschlägt",
        }
    }
    /// Wert für `AuthenticationLevel` des RDP-Controls
    pub fn value(self) -> u32 {
        match self {
            AuthLevel::AlwaysConnect => 0,
            AuthLevel::DontConnect => 1,
            AuthLevel::Warn => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GatewayUsage {
    #[default]
    Never,
    Always,
    Detect,
}

impl GatewayUsage {
    pub fn label(self) -> &'static str {
        match self {
            GatewayUsage::Never => "Niemals",
            GatewayUsage::Always => "Immer",
            GatewayUsage::Detect => "Erkennen",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resolution {
    #[default]
    FitToPanel,
    SmartSize,
    Fullscreen,
    Fixed,
}

impl Resolution {
    pub fn label(self) -> &'static str {
        match self {
            Resolution::FitToPanel => "An Panel anpassen",
            Resolution::SmartSize => "Smart Size (skalieren)",
            Resolution::Fullscreen => "Vollbild",
            Resolution::Fixed => "Feste Auflösung",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SoundMode {
    /// Auf diesem Computer wiedergeben
    BringToThisComputer,
    /// Auf dem Remotecomputer belassen
    LeaveAtRemote,
    #[default]
    DoNotPlay,
}

impl SoundMode {
    pub fn label(self) -> &'static str {
        match self {
            SoundMode::BringToThisComputer => "Auf diesem Computer wiedergeben",
            SoundMode::LeaveAtRemote => "Auf Remotecomputer belassen",
            SoundMode::DoNotPlay => "Nicht wiedergeben",
        }
    }
    pub fn value(self) -> i32 {
        match self {
            SoundMode::BringToThisComputer => 0,
            SoundMode::LeaveAtRemote => 1,
            SoundMode::DoNotPlay => 2,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Connection {
    pub id: u64,
    // --- Anzeige
    pub name: String,
    #[serde(alias = "notes")]
    pub description: String,
    pub icon: IconKind,
    /// Tags, durch Komma getrennt (z. B. "prod, linux, dc")
    pub tags: String,
    /// Farbe für Tab und Baum
    pub color: TabColor,
    /// Freitext-Notizen (Wartungsfenster, Ansprechpartner …)
    pub memo: String,
    // --- Verbindung
    pub host: String,
    pub username: String,
    pub domain: String,
    /// Benutzer, Domäne und Passwort vom nächsten übergeordneten Ordner mit Anmeldedaten übernehmen
    pub inherit_credentials: bool,
    // --- Protokoll
    pub protocol: Protocol,
    pub port: u16,
    // --- SSH
    /// Private-Key-Datei (ssh -i)
    pub ssh_key_file: String,
    /// Zusätzliche Kommandozeilenargumente für ssh.exe
    pub ssh_extra_args: String,
    /// Agent-Weiterleitung (ssh -A)
    pub ssh_agent_forward: bool,
    /// Portweiterleitungen (-L/-R/-D)
    pub forwards: Vec<Forward>,
    /// Über diese SSH-Verbindung (ID) als Sprunghost/Tunnel verbinden, 0 = direkt
    pub via_ssh: u64,
    // --- Web (HTTP/HTTPS)
    /// Pfad hinter Host:Port, z. B. "/login.html"
    pub web_path: String,
    /// Zertifikatsfehler ignorieren (selbstsignierte Zertifikate von iDRAC/iLO, Druckern …)
    pub ignore_cert_errors: bool,
    /// "Verwende Konsole" (/admin)
    pub admin_session: bool,
    pub auth_level: AuthLevel,
    /// Minuten bis zum Leerlauf-Timeout (0 = aus)
    pub idle_minutes: u32,
    pub load_balance_info: String,
    /// "Verwende Sicherheitssupportprovider" (CredSSP / NLA)
    pub use_credssp: bool,
    // --- Gateway
    pub gateway_usage: GatewayUsage,
    pub gateway: String,
    // --- Aussehen
    pub resolution: Resolution,
    pub fixed_width: u32,
    pub fixed_height: u32,
    pub auto_resize: bool,
    /// Farbtiefe in Bit: 15, 16, 24 oder 32
    pub colors: u32,
    pub bitmap_cache: bool,
    pub wallpaper: bool,
    pub themes: bool,
    pub font_smoothing: bool,
    pub desktop_composition: bool,
    // --- Umleitung
    pub redirect_keys: bool,
    pub redirect_drives: bool,
    pub redirect_printers: bool,
    pub redirect_ports: bool,
    pub redirect_smartcards: bool,
    pub redirect_clipboard: bool,
    pub sound: SoundMode,
    // --- Mehrere Monitore
    /// Über alle Monitore verbinden (öffnet mstsc mit „use multimon“)
    pub multimon: bool,
    /// Monitor-Nummern für mstsc, z. B. "0,1" (leer = alle); siehe `mstsc /l`
    pub monitors: String,
    // --- Passwort-Tresor
    /// Eintrag in KeePass/Bitwarden, aus dem Benutzer und Passwort kommen (leer = Windows-Tresor)
    pub vault_entry: String,
    /// MAC-Adresse für Wake-on-LAN
    pub mac: String,
}

impl Default for Connection {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            description: String::new(),
            icon: IconKind::Server,
            tags: String::new(),
            color: TabColor::None,
            memo: String::new(),
            host: String::new(),
            username: String::new(),
            domain: String::new(),
            inherit_credentials: false,
            protocol: Protocol::Rdp,
            port: 3389,
            ssh_key_file: String::new(),
            ssh_extra_args: String::new(),
            ssh_agent_forward: false,
            forwards: Vec::new(),
            via_ssh: 0,
            web_path: String::new(),
            ignore_cert_errors: false,
            admin_session: false,
            auth_level: AuthLevel::Warn,
            idle_minutes: 0,
            load_balance_info: String::new(),
            use_credssp: true,
            gateway_usage: GatewayUsage::Never,
            gateway: String::new(),
            resolution: Resolution::FitToPanel,
            fixed_width: 1920,
            fixed_height: 1080,
            auto_resize: true,
            colors: 32,
            bitmap_cache: false,
            wallpaper: false,
            themes: false,
            font_smoothing: false,
            desktop_composition: false,
            redirect_keys: true,
            redirect_drives: false,
            redirect_printers: false,
            redirect_ports: false,
            redirect_smartcards: false,
            redirect_clipboard: true,
            sound: SoundMode::DoNotPlay,
            multimon: false,
            monitors: String::new(),
            vault_entry: String::new(),
            mac: String::new(),
        }
    }
}

impl Connection {
    /// `PerformanceFlags` des RDP-Controls (TS_PERF_*)
    pub fn performance_flags(&self) -> i32 {
        let mut f = 0;
        if !self.wallpaper {
            f |= 0x1; // DISABLE_WALLPAPER
        }
        if !self.themes {
            f |= 0x8; // DISABLE_THEMING
        }
        if self.font_smoothing {
            f |= 0x80; // ENABLE_FONT_SMOOTHING
        }
        if self.desktop_composition {
            f |= 0x100; // ENABLE_DESKTOP_COMPOSITION
        }
        f
    }

    /// Adresse für Web-Verbindungen. Ein Host mit "://" wird unverändert als URL verwendet.
    pub fn web_url(&self) -> String {
        let host = self.host.trim();
        if host.contains("://") {
            return host.to_string();
        }
        let scheme = if self.protocol == Protocol::Http { "http" } else { "https" };
        let port = if self.port == self.protocol.default_port() || self.port == 0 { String::new() } else { format!(":{}", self.port) };
        let path = self.web_path.trim();
        let path = if path.is_empty() || path.starts_with('/') { path.to_string() } else { format!("/{path}") };
        format!("{scheme}://{host}{port}{path}")
    }

    /// Tags als Liste (klein geschrieben, ohne Leerzeichen und führendes #)
    pub fn tag_list(&self) -> Vec<String> {
        parse_tags(&self.tags)
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tag_list().iter().any(|t| t == tag)
    }

    /// Passt die Verbindung zu allen Suchbegriffen? `#tag` sucht nur in den Tags,
    /// andere Begriffe in Name, Host, Benutzer, Beschreibung, Notizen, Tags und `path` (Ordner).
    pub fn matches_terms(&self, terms: &[String], path: &str) -> bool {
        if terms.is_empty() {
            return true;
        }
        let tags = self.tag_list();
        let hay = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}",
            self.display_name(),
            self.host,
            self.username,
            self.description,
            self.memo,
            self.tags,
            path
        )
        .to_lowercase();
        terms.iter().all(|t| match t.strip_prefix('#') {
            Some(tag) => tags.iter().any(|x| x.starts_with(tag)),
            None => hay.contains(t.as_str()),
        })
    }

    pub fn display_name(&self) -> &str {
        if self.name.trim().is_empty() { &self.host } else { &self.name }
    }

    /// Benutzername inkl. Domäne, so wie mstsc ihn erwartet.
    pub fn full_username(&self) -> String {
        join_user(&self.domain, &self.username)
    }
}

/// "prod, #Linux  dc" → ["prod", "linux", "dc"]
pub fn parse_tags(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in s.split([',', ';', ' ']) {
        let t = t.trim().trim_start_matches('#').to_lowercase();
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// Suchtext in Begriffe zerlegen (klein geschrieben)
pub fn search_terms(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_lowercase).collect()
}

/// "DOMÄNE\benutzer" – bzw. unverändert, wenn der Benutzer schon eine Domäne enthält
pub fn join_user(domain: &str, user: &str) -> String {
    match (domain.trim(), user.trim()) {
        (_, "") => String::new(),
        // enthält bereits eine Domäne ("corp\jdoe" oder "jdoe@corp")
        (_, u) if u.contains('\\') || u.contains('@') => u.to_string(),
        ("", u) => u.to_string(),
        (d, u) => format!("{d}\\{u}"),
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Folder {
    pub id: u64,
    pub name: String,
    /// Gemeinsame Anmeldedaten für Verbindungen, die sie übernehmen (Passwort im Windows-Tresor)
    pub username: String,
    pub domain: String,
    pub folders: Vec<Folder>,
    pub connections: Vec<Connection>,
}

impl Folder {
    pub fn has_credentials(&self) -> bool {
        !self.username.trim().is_empty()
    }

    pub fn full_username(&self) -> String {
        join_user(&self.domain, &self.username)
    }

    /// Ordnerkette von hier bis zum Ordner `id` (inklusive beider Enden)
    pub fn folder_chain(&self, id: u64) -> Option<Vec<&Folder>> {
        if self.id == id {
            return Some(vec![self]);
        }
        self.folders.iter().find_map(|f| {
            let mut chain = f.folder_chain(id)?;
            chain.insert(0, self);
            Some(chain)
        })
    }

    /// Nächster Ordner mit Anmeldedaten, vom Ordner `folder_id` aus nach außen gesucht.
    pub fn credential_folder(&self, folder_id: u64) -> Option<&Folder> {
        self.folder_chain(folder_id)?.into_iter().rev().find(|f| f.has_credentials())
    }

    /// Wirksame Anmeldedaten einer Verbindung: (Verbindung mit Benutzer/Domäne, ID des Passwort-Eintrags).
    /// Übernimmt die Verbindung die Anmeldedaten vom Ordner, stammen Benutzer, Domäne und Passwort von dort.
    pub fn resolve_credentials(&self, conn: &Connection) -> (Connection, u64) {
        if conn.inherit_credentials
            && let Some(f) = self.parent_of_connection(conn.id).and_then(|p| self.credential_folder(p))
        {
            let mut c = conn.clone();
            c.username = f.username.clone();
            c.domain = f.domain.clone();
            return (c, f.id);
        }
        (conn.clone(), conn.id)
    }

    pub fn for_each_folder(&self, cb: &mut dyn FnMut(&Folder)) {
        cb(self);
        for f in &self.folders {
            f.for_each_folder(cb);
        }
    }
    pub fn find_folder_mut(&mut self, id: u64) -> Option<&mut Folder> {
        if self.id == id {
            return Some(self);
        }
        self.folders.iter_mut().find_map(|f| f.find_folder_mut(id))
    }

    pub fn find_connection(&self, id: u64) -> Option<&Connection> {
        self.connections
            .iter()
            .find(|c| c.id == id)
            .or_else(|| self.folders.iter().find_map(|f| f.find_connection(id)))
    }

    /// Liefert die ID des Ordners, in dem die Verbindung liegt.
    pub fn parent_of_connection(&self, id: u64) -> Option<u64> {
        if self.connections.iter().any(|c| c.id == id) {
            return Some(self.id);
        }
        self.folders.iter().find_map(|f| f.parent_of_connection(id))
    }

    pub fn remove_connection(&mut self, id: u64) -> Option<Connection> {
        if let Some(pos) = self.connections.iter().position(|c| c.id == id) {
            return Some(self.connections.remove(pos));
        }
        self.folders.iter_mut().find_map(|f| f.remove_connection(id))
    }

    pub fn remove_folder(&mut self, id: u64) -> Option<Folder> {
        if let Some(pos) = self.folders.iter().position(|f| f.id == id) {
            return Some(self.folders.remove(pos));
        }
        self.folders.iter_mut().find_map(|f| f.remove_folder(id))
    }

    pub fn find_folder(&self, id: u64) -> Option<&Folder> {
        if self.id == id {
            return Some(self);
        }
        self.folders.iter().find_map(|f| f.find_folder(id))
    }

    /// Liefert die ID des Ordners, in dem der Ordner `id` liegt.
    pub fn parent_of_folder(&self, id: u64) -> Option<u64> {
        if self.folders.iter().any(|f| f.id == id) {
            return Some(self.id);
        }
        self.folders.iter().find_map(|f| f.parent_of_folder(id))
    }

    /// Darf `item` in den Ordner `target` verschoben werden?
    /// (nicht in den eigenen Ordner, kein Ordner in sich selbst oder einen Unterordner)
    pub fn can_move(&self, item: Item, target: u64) -> bool {
        if self.find_folder(target).is_none() {
            return false;
        }
        match item {
            Item::Connection(id) => self.parent_of_connection(id).is_some_and(|p| p != target),
            Item::Folder(id) => {
                id != ROOT_ID
                    && self.parent_of_folder(id).is_some_and(|p| p != target)
                    && self.find_folder(id).is_some_and(|f| f.find_folder(target).is_none())
            }
        }
    }

    /// Verschiebt eine Verbindung oder einen Ordner in den Ordner `target`.
    pub fn move_item(&mut self, item: Item, target: u64) -> bool {
        if !self.can_move(item, target) {
            return false;
        }
        match item {
            Item::Connection(id) => {
                let Some(c) = self.remove_connection(id) else { return false };
                self.find_folder_mut(target).expect("geprüft").connections.push(c);
            }
            Item::Folder(id) => {
                let Some(f) = self.remove_folder(id) else { return false };
                self.find_folder_mut(target).expect("geprüft").folders.push(f);
            }
        }
        true
    }

    /// Alle Ordner als (ID, Pfad) für Auswahllisten.
    pub fn folder_paths(&self, prefix: &str, out: &mut Vec<(u64, String)>) {
        for f in &self.folders {
            let path = if prefix.is_empty() { f.name.clone() } else { format!("{prefix} / {}", f.name) };
            out.push((f.id, path.clone()));
            f.folder_paths(&path, out);
        }
    }

    pub fn sort(&mut self) {
        self.folders.sort_by_key(|f| f.name.to_lowercase());
        self.connections.sort_by_key(|c| c.display_name().to_lowercase());
        self.folders.iter_mut().for_each(Folder::sort);
    }
}

pub const ROOT_ID: u64 = 0;

/// Externes Werkzeug (Menüeintrag) mit Platzhaltern wie %HOST%
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtTool {
    pub name: String,
    /// Programm, Datei oder URL (Platzhalter erlaubt)
    pub program: String,
    pub args: String,
}

/// Gemeinsame Verbindungsliste (z. B. auf einem Netzlaufwerk), schreibgeschützt eingebunden
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SharedSource {
    pub name: String,
    /// connections.json (RDP Manager) oder confCons.xml (mRemoteNG)
    pub path: String,
}

/// Gespeicherter Befehl, der per Klick an SSH-Sitzungen gesendet wird
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Snippet {
    pub name: String,
    /// Befehl, darf mehrzeilig sein
    pub command: String,
    /// Am Ende Enter senden (sonst nur eintippen)
    pub enter: bool,
}

/// Ein Element im Verbindungsbaum (für Verschieben / Drag & Drop)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Connection(u64),
    Folder(u64),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Folder {
        // root(0) ─ A(1) ─ B(2) ─ conn 10
        //         └ C(3)
        let b = Folder { id: 2, name: "B".into(), connections: vec![Connection { id: 10, ..Default::default() }], ..Default::default() };
        let a = Folder { id: 1, name: "A".into(), folders: vec![b], ..Default::default() };
        let c = Folder { id: 3, name: "C".into(), ..Default::default() };
        Folder { id: ROOT_ID, folders: vec![a, c], ..Default::default() }
    }

    #[test]
    fn move_rules() {
        let mut r = tree();
        assert!(!r.can_move(Item::Folder(1), 2), "Ordner nicht in eigenen Unterordner");
        assert!(!r.can_move(Item::Folder(1), 1), "Ordner nicht in sich selbst");
        assert!(!r.can_move(Item::Folder(1), ROOT_ID), "liegt schon dort");
        assert!(!r.can_move(Item::Connection(10), 2), "liegt schon dort");
        assert!(!r.can_move(Item::Folder(ROOT_ID), 3));
        assert!(r.move_item(Item::Folder(2), 3));
        assert_eq!(r.parent_of_folder(2), Some(3));
        assert!(r.move_item(Item::Connection(10), ROOT_ID));
        assert_eq!(r.parent_of_connection(10), Some(ROOT_ID));
        assert!(r.find_folder(3).unwrap().find_folder(2).unwrap().connections.is_empty());
    }

    #[test]
    fn tags_and_search() {
        assert_eq!(parse_tags("prod, #Linux  dc;prod"), vec!["prod", "linux", "dc"]);
        let c = Connection {
            name: "Web01".into(),
            host: "web01.corp.local".into(),
            tags: "prod, linux".into(),
            memo: "Wartung So 02:00".into(),
            ..Default::default()
        };
        let t = |s: &str| search_terms(s);
        assert!(c.matches_terms(&t("web #prod"), ""));
        assert!(c.matches_terms(&t("#lin"), ""));
        assert!(!c.matches_terms(&t("#dc"), ""));
        assert!(c.matches_terms(&t("wartung"), ""));
        assert!(c.matches_terms(&t("rechenzentrum"), "Rechenzentrum / Linux"));
        assert!(!c.matches_terms(&t("web02"), ""));
        // alte Dateien ohne neue Felder bleiben lesbar
        let old: Connection = serde_json::from_str(r#"{"id":5,"host":"x","notes":"alt"}"#).unwrap();
        assert_eq!((old.description.as_str(), old.color, old.forwards.len()), ("alt", TabColor::None, 0));
    }

    #[test]
    fn forward_args() {
        let f = Forward { kind: ForwardKind::Local, listen_port: 13389, target_host: "intern".into(), target_port: 3389 };
        assert_eq!(f.args().unwrap(), ["-L".to_string(), "127.0.0.1:13389:intern:3389".to_string()]);
        let f = Forward { kind: ForwardKind::Remote, listen_port: 8080, target_host: "fe80::1".into(), target_port: 80 };
        assert_eq!(f.args().unwrap()[1], "8080:[fe80::1]:80");
        let f = Forward { kind: ForwardKind::Dynamic, listen_port: 1080, ..Default::default() };
        assert_eq!(f.args().unwrap()[1], "127.0.0.1:1080");
        assert!(Forward { kind: ForwardKind::Local, listen_port: 1, ..Default::default() }.args().is_none());
    }

    #[test]
    fn broken_config_restored_from_backup() {
        let d = std::env::temp_dir().join(format!("rdpm_load_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let bdir = d.join("backups");
        std::fs::create_dir_all(&bdir).unwrap();
        let file = d.join("connections.json");
        // fehlt → Standard, kein Hinweis
        assert!(Store::load_from(&file, &bdir, "x").1.is_none());
        // mit BOM → wird normal gelesen
        std::fs::write(&file, "\u{feff}{\"next_id\":7}").unwrap();
        let (s, msg) = Store::load_from(&file, &bdir, "x");
        assert_eq!((s.next_id, msg.is_none()), (7, true));
        // beschädigt → Sicherung laden, defekte Datei aufheben
        std::fs::write(bdir.join("connections_2026-11-01_09-00-00.json"), "{\"next_id\":42}").unwrap();
        std::fs::write(&file, "{ kaputt").unwrap();
        let (s, msg) = Store::load_from(&file, &bdir, "2026-11-16_10-00-00");
        assert_eq!(s.next_id, 42);
        assert!(msg.unwrap().contains("Wiederhergestellt"));
        assert!(bdir.join("connections_defekt_2026-11-16_10-00-00.json").exists());
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn inherited_credentials() {
        let mut r = tree();
        // A(1) hat Anmeldedaten, B(2) liegt darin; Verbindung 10 liegt in B
        let a = r.find_folder_mut(1).unwrap();
        a.username = "jdoe".into();
        a.domain = "CORP".into();
        let c = r.find_connection(10).unwrap().clone();
        // ohne Übernahme: eigene Daten
        assert_eq!(r.resolve_credentials(&c).1, 10);
        let b = r.find_folder_mut(2).unwrap();
        b.connections[0].inherit_credentials = true;
        let c = r.find_connection(10).unwrap().clone();
        let (eff, cred) = r.resolve_credentials(&c);
        assert_eq!((eff.full_username().as_str(), cred), ("CORP\\jdoe", 1));
        // näherer Ordner gewinnt
        r.find_folder_mut(2).unwrap().username = "corp\\admin".into();
        let (eff, cred) = r.resolve_credentials(&c);
        assert_eq!((eff.full_username().as_str(), cred), ("corp\\admin", 2));
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    pub theme: ThemeMode,
    /// Zuletzt per Quick Connect verwendete Ziele
    pub quick_history: Vec<String>,
    /// Schriftgröße im SSH-Terminal
    pub terminal_font_size: f32,
    /// Erreichbarkeit der Verbindungen im Baum anzeigen
    pub check_reachability: bool,
    /// Befehls-Schnipsel für SSH-Sitzungen
    pub snippets: Vec<Snippet>,
    /// SSH-Sitzungen automatisch in eine Logdatei mitschreiben
    pub ssh_auto_log: bool,
    /// Externe Werkzeuge (Kontextmenü)
    pub tools: Vec<ExtTool>,
    /// Gemeinsame, schreibgeschützte Verbindungslisten
    pub shared_sources: Vec<SharedSource>,
    /// Automatische Sicherungen der connections.json
    pub backup_enabled: bool,
    /// Anzahl der aufbewahrten Sicherungen (zusätzlich je eine pro Tag der letzten 14 Tage)
    pub backup_keep: usize,
    /// Optionaler zweiter Sicherungsordner (z. B. Netzlaufwerk oder OneDrive)
    pub backup_extra_dir: String,
    /// Zuletzt gestartete Version (für „Was ist neu“ nach einem Update)
    pub last_seen_version: String,
    /// Programmsperre (Hauptkennwort / Windows Hello)
    pub lock: LockSettings,
    /// Externer Passwort-Tresor (KeePass / Bitwarden)
    pub vault: VaultSettings,
    /// Verbindungsprotokoll führen (wer hat wann was wie lange geöffnet)
    pub conn_log: bool,
    /// Favoriten (Verbindungs-IDs, auch aus gemeinsamen Listen)
    pub favorites: Vec<u64>,
    /// Zuletzt geöffnete Verbindungen, neueste zuerst
    pub recent: Vec<u64>,
    /// Beim Beenden offene Tabs beim nächsten Start wieder öffnen
    pub restore_tabs: bool,
    /// Beim letzten Beenden offene Verbindungen (aktive zuletzt)
    pub open_tabs: Vec<u64>,
    /// Ordner oder URL mit neuen Versionen (RdpManager-x.y.z-x64.msi)
    pub update_source: String,
    /// Beim Start nach Updates suchen
    pub update_check: bool,
    pub next_id: u64,
    pub root: Folder,
}

/// Programmsperre beim Start und nach Leerlauf
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LockSettings {
    /// "salt$hash" (Base64, PBKDF2-HMAC-SHA1); leer = kein Hauptkennwort
    pub password_hash: String,
    /// Entsperren mit Windows Hello (PIN, Fingerabdruck, Gesicht)
    pub hello: bool,
    /// Beim Programmstart sperren
    pub at_start: bool,
    /// Nach so vielen Minuten ohne Eingabe sperren (0 = nie)
    pub idle_minutes: u32,
}

impl Default for LockSettings {
    fn default() -> Self {
        Self { password_hash: String::new(), hello: false, at_start: true, idle_minutes: 0 }
    }
}

impl LockSettings {
    pub fn enabled(&self) -> bool {
        !self.password_hash.is_empty() || self.hello
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VaultKind {
    #[default]
    None,
    /// KeePassXC (keepassxc-cli.exe)
    KeePassXc,
    /// KeePass 2 mit KPScript.exe
    KpScript,
    /// Bitwarden / Vaultwarden (bw.exe)
    Bitwarden,
}

impl VaultKind {
    pub const ALL: [VaultKind; 4] = [VaultKind::None, VaultKind::KeePassXc, VaultKind::KpScript, VaultKind::Bitwarden];
    pub fn label(self) -> &'static str {
        match self {
            VaultKind::None => "Keiner (nur Windows-Tresor)",
            VaultKind::KeePassXc => "KeePassXC (keepassxc-cli)",
            VaultKind::KpScript => "KeePass 2 (KPScript)",
            VaultKind::Bitwarden => "Bitwarden / Vaultwarden (bw)",
        }
    }
    /// Standardname des Kommandozeilenprogramms
    pub fn default_program(self) -> &'static str {
        match self {
            VaultKind::None => "",
            VaultKind::KeePassXc => "keepassxc-cli.exe",
            VaultKind::KpScript => "KPScript.exe",
            VaultKind::Bitwarden => "bw.exe",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VaultSettings {
    pub kind: VaultKind,
    /// KeePass-Datenbank (.kdbx)
    pub database: String,
    /// Optionale Schlüsseldatei für KeePass
    pub key_file: String,
    /// Pfad zum Kommandozeilenprogramm (leer = Standardname bzw. Standard-Installationsort)
    pub program: String,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            quick_history: Vec::new(),
            terminal_font_size: 14.0,
            check_reachability: true,
            snippets: Vec::new(),
            ssh_auto_log: false,
            tools: crate::tools::defaults(),
            shared_sources: Vec::new(),
            backup_enabled: true,
            backup_keep: 20,
            backup_extra_dir: String::new(),
            last_seen_version: String::new(),
            lock: LockSettings::default(),
            vault: VaultSettings::default(),
            conn_log: true,
            favorites: Vec::new(),
            recent: Vec::new(),
            restore_tabs: true,
            open_tabs: Vec::new(),
            update_source: String::new(),
            update_check: true,
            next_id: 1,
            root: Folder { id: ROOT_ID, name: "Verbindungen".into(), ..Default::default() },
        }
    }
}

impl Store {
    pub fn new_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn data_dir() -> PathBuf {
        let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        base.join("RdpManager")
    }

    pub fn file() -> PathBuf {
        Self::data_dir().join("connections.json")
    }

    /// Lädt die Konfiguration. Ist die Datei beschädigt, wird sie beiseitegelegt und die neueste
    /// lesbare Sicherung geladen. Rückgabe: (Konfiguration, Hinweis für den Benutzer)
    pub fn load_checked() -> (Self, Option<String>) {
        Self::load_from(&Self::file(), &crate::backup::backup_dir(), &crate::ssh::local_timestamp())
    }

    pub fn load_from(file: &std::path::Path, backup_dir: &std::path::Path, stamp: &str) -> (Self, Option<String>) {
        let problem = match std::fs::read_to_string(file) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Self::default(), None),
            Err(e) => format!("nicht lesbar ({e})"),
            Ok(text) if text.trim().is_empty() => "leer".to_string(),
            Ok(text) => match serde_json::from_str::<Store>(text.trim_start_matches('\u{feff}')) {
                Ok(s) => return (s, None),
                Err(e) => format!("beschädigt ({e})"),
            },
        };
        // beschädigte Datei aufheben, nicht überschreiben
        let _ = std::fs::create_dir_all(backup_dir);
        let broken = backup_dir.join(format!("connections_defekt_{stamp}.json"));
        let _ = std::fs::rename(file, &broken);
        match crate::backup::newest_readable(backup_dir) {
            Some((p, s)) => (
                s,
                Some(format!(
                    "Die Verbindungsdatei war {problem}. Wiederhergestellt aus der Sicherung {} – die defekte Datei liegt unter {}.",
                    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                    broken.display()
                )),
            ),
            None => (
                Self::default(),
                Some(format!("Die Verbindungsdatei war {problem} und es gibt keine Sicherung. Sie wurde nach {} verschoben.", broken.display())),
            ),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let dir = Self::data_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        // Erst in temporäre Datei schreiben, dann ersetzen -> keine halbe Datei bei Absturz.
        let tmp = dir.join("connections.json.tmp");
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, Self::file()).map_err(|e| e.to_string())
    }
}
