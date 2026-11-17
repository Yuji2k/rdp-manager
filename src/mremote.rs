//! Import/Export im mRemoteNG-Format (confCons.xml, ConfVersion 2.6).
//!
//! Passwörter werden wie in mRemoteNG verschlüsselt:
//! AES-256-GCM, 128-Bit-Nonce, 128-Bit-Tag, Schlüssel = PBKDF2-HMAC-SHA1(Passwort, Salt, KdfIterations),
//! Ausgabe = Base64(Salt[16] | Nonce[16] | Ciphertext | Tag[16]), Salt dient zusätzlich als AAD.
//! Ohne eigenes Master-Passwort verwendet mRemoteNG den Standardschlüssel "mR3m".

use crate::model::{AuthLevel, Connection, Folder, GatewayUsage, IconKind, Protocol, Resolution, SoundMode};
use aes_gcm::aes::Aes256;
use aes_gcm::aes::cipher::consts::U16;
use aes_gcm::{AeadInOut, AesGcm, KeyInit, Nonce};
use base64::Engine as _;
use std::collections::HashMap;
use std::fmt::Write as _;

type Cipher = AesGcm<Aes256, U16>;

pub const DEFAULT_PASSWORD: &str = "mR3m";
const KDF_ITERATIONS: u32 = 1000;
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

// ---------------------------------------------------------------------------
// Kryptografie
// ---------------------------------------------------------------------------

fn derive_key(password: &str, salt: &[u8], iterations: u32) -> [u8; 32] {
    // BouncyCastle Pkcs5PasswordToBytes: jedes Zeichen -> 1 Byte
    let pw: Vec<u8> = password.chars().map(|c| c as u32 as u8).collect();
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(&pw, salt, iterations, &mut key);
    key
}

pub fn encrypt(plain: &str, password: &str, iterations: u32) -> String {
    if plain.is_empty() {
        return String::new();
    }
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut salt).expect("Zufallszahlen");
    getrandom::fill(&mut nonce).expect("Zufallszahlen");
    let key = derive_key(password, &salt, iterations);
    let cipher = Cipher::new_from_slice(&key).expect("Schlüssellänge");
    let mut buf = plain.as_bytes().to_vec();
    let n = Nonce::<U16>::try_from(&nonce[..]).expect("Nonce-Länge");
    cipher.encrypt_in_place(&n, &salt, &mut buf).expect("Verschlüsselung");
    let mut out = Vec::with_capacity(32 + buf.len());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&buf);
    B64.encode(out)
}

pub fn decrypt(cipher_b64: &str, password: &str, iterations: u32) -> Option<String> {
    let data = B64.decode(cipher_b64.trim()).ok()?;
    if data.len() < 48 {
        return None;
    }
    let (salt, rest) = data.split_at(16);
    let (nonce, ct) = rest.split_at(16);
    let key = derive_key(password, salt, iterations);
    let cipher = Cipher::new_from_slice(&key).ok()?;
    let mut buf = ct.to_vec();
    let n = Nonce::<U16>::try_from(nonce).ok()?;
    cipher.decrypt_in_place(&n, salt, &mut buf).ok()?;
    String::from_utf8(buf).ok()
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\r' => o.push_str("&#xD;"),
            '\n' => o.push_str("&#xA;"),
            '\t' => o.push_str("&#x9;"),
            c => o.push(c),
        }
    }
    o
}

fn b(v: bool) -> String {
    if v { "true".into() } else { "false".into() }
}

fn icon_name(i: IconKind) -> &'static str {
    match i {
        IconKind::Server => "mRemoteNG",
        IconKind::DomainController => "Domain Controller",
        IconKind::Workstation => "Workstation",
        IconKind::Database => "Database",
        IconKind::Web => "Web Server",
        IconKind::Linux => "Linux",
        IconKind::Other => "Windows",
    }
}

fn icon_from(name: &str) -> IconKind {
    match name {
        "Domain Controller" => IconKind::DomainController,
        "Workstation" => IconKind::Workstation,
        "Database" => IconKind::Database,
        "Web Server" => IconKind::Web,
        "Linux" => IconKind::Linux,
        "Windows" => IconKind::Other,
        _ => IconKind::Server,
    }
}

const INHERIT_KEYS: &[&str] = &[
    "CacheBitmaps", "Colors", "Description", "DisplayThemes", "DisplayWallpaper", "EnableFontSmoothing",
    "EnableDesktopComposition", "Domain", "Icon", "Panel", "Password", "Port", "Protocol", "PuttySession",
    "RedirectDiskDrives", "RedirectKeys", "RedirectPorts", "RedirectPrinters", "RedirectSmartCards", "RedirectSound",
    "SoundQuality", "Resolution", "AutomaticResize", "UseConsoleSession", "UseCredSsp", "RenderingEngine", "Username",
    "ICAEncryptionStrength", "RDPAuthenticationLevel", "RDPMinutesToIdleTimeout", "RDPAlertIdleTimeout",
    "LoadBalanceInfo", "PreExtApp", "PostExtApp", "MacAddress", "UserField", "ExtApp", "VNCCompression",
    "VNCEncoding", "VNCAuthMode", "VNCProxyType", "VNCProxyIP", "VNCProxyPort", "VNCProxyUsername",
    "VNCProxyPassword", "VNCColors", "VNCSmartSizeMode", "VNCViewOnly", "RDGatewayUsageMethod",
    "RDGatewayHostname", "RDGatewayUseConnectionCredentials", "RDGatewayUsername", "RDGatewayPassword",
    "RDGatewayDomain",
];

/// Attribute eines Knotens in derselben Reihenfolge wie mRemoteNG.
fn node_attributes(c: &Connection, kind: &str, password: &str) -> Vec<(&'static str, String)> {
    let colors = match c.colors {
        15 => "Colors15Bit",
        16 => "Colors16Bit",
        24 => "Colors24Bit",
        _ => "Colors32Bit",
    };
    let resolution = match c.resolution {
        Resolution::FitToPanel => "FitToWindow".to_string(),
        Resolution::SmartSize => "SmartSize".to_string(),
        Resolution::Fullscreen => "Fullscreen".to_string(),
        Resolution::Fixed => format!("Res{}x{}", c.fixed_width, c.fixed_height),
    };
    let auth = match c.auth_level {
        AuthLevel::AlwaysConnect => "NoAuth",
        AuthLevel::Warn => "WarnOnFailedAuth",
        AuthLevel::DontConnect => "AuthRequired",
    };
    let sound = match c.sound {
        SoundMode::BringToThisComputer => "BringToThisComputer",
        SoundMode::LeaveAtRemote => "LeaveAtRemoteComputer",
        SoundMode::DoNotPlay => "DoNotPlay",
    };
    let gw = match c.gateway_usage {
        GatewayUsage::Never => "Never",
        GatewayUsage::Always => "Always",
        GatewayUsage::Detect => "Detect",
    };
    let mut a: Vec<(&'static str, String)> = vec![
        ("Name", c.name.clone()),
        ("Type", kind.into()),
    ];
    if kind == "Container" {
        a.push(("Expanded", "true".into()));
    }
    a.extend([
        ("Descr", c.description.clone()),
        ("Icon", icon_name(c.icon).into()),
        ("Panel", "General".into()),
        ("Id", uuid::Uuid::new_v4().to_string()),
        ("Username", c.username.clone()),
        ("Domain", c.domain.clone()),
        ("Password", password.into()),
        ("Hostname", c.host.clone()),
        (
            "Protocol",
            match c.protocol {
                Protocol::Rdp => "RDP",
                Protocol::Ssh => "SSH2",
                Protocol::Vnc => "VNC",
                Protocol::Http => "HTTP",
                Protocol::Https => "HTTPS",
            }
            .into(),
        ),
        ("PuttySession", "Default Settings".into()),
        ("Port", c.port.to_string()),
        ("ConnectToConsole", b(c.admin_session)),
        ("UseCredSsp", b(c.use_credssp)),
        ("RenderingEngine", "IE".into()),
        ("ICAEncryptionStrength", "EncrBasic".into()),
        ("RDPAuthenticationLevel", auth.into()),
        ("RDPMinutesToIdleTimeout", c.idle_minutes.to_string()),
        ("RDPAlertIdleTimeout", "false".into()),
        ("LoadBalanceInfo", c.load_balance_info.clone()),
        ("Colors", colors.into()),
        ("Resolution", resolution),
        ("AutomaticResize", b(c.auto_resize)),
        ("DisplayWallpaper", b(c.wallpaper)),
        ("DisplayThemes", b(c.themes)),
        ("EnableFontSmoothing", b(c.font_smoothing)),
        ("EnableDesktopComposition", b(c.desktop_composition)),
        ("CacheBitmaps", b(c.bitmap_cache)),
        ("RedirectDiskDrives", b(c.redirect_drives)),
        ("RedirectPorts", b(c.redirect_ports)),
        ("RedirectPrinters", b(c.redirect_printers)),
        ("RedirectSmartCards", b(c.redirect_smartcards)),
        ("RedirectSound", sound.into()),
        ("SoundQuality", "Dynamic".into()),
        ("RedirectKeys", b(c.redirect_keys)),
        ("Connected", "false".into()),
        ("PreExtApp", String::new()),
        ("PostExtApp", String::new()),
        ("MacAddress", String::new()),
        ("UserField", String::new()),
        ("ExtApp", String::new()),
        ("VNCCompression", "CompNone".into()),
        ("VNCEncoding", "EncHextile".into()),
        ("VNCAuthMode", "AuthVNC".into()),
        ("VNCProxyType", "ProxyNone".into()),
        ("VNCProxyIP", String::new()),
        ("VNCProxyPort", "0".into()),
        ("VNCProxyUsername", String::new()),
        ("VNCProxyPassword", String::new()),
        ("VNCColors", "ColNormal".into()),
        ("VNCSmartSizeMode", "SmartSAspect".into()),
        ("VNCViewOnly", "false".into()),
        ("RDGatewayUsageMethod", gw.into()),
        ("RDGatewayHostname", c.gateway.clone()),
        ("RDGatewayUseConnectionCredentials", "Yes".into()),
        ("RDGatewayUsername", String::new()),
        ("RDGatewayPassword", String::new()),
        ("RDGatewayDomain", String::new()),
    ]);
    for k in INHERIT_KEYS {
        a.push((leak_inherit(k), "false".into()));
    }
    a
}

fn leak_inherit(k: &str) -> &'static str {
    // kleine, feste Menge an Schlüsseln -> einmalig erzeugen
    use std::sync::OnceLock;
    static NAMES: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    NAMES.get_or_init(|| {
        INHERIT_KEYS.iter().map(|k| (*k, &*Box::leak(format!("Inherit{k}").into_boxed_str()))).collect()
    })[k]
}

fn write_node(out: &mut String, depth: usize, attrs: &[(&'static str, String)], children: Option<&str>) {
    let indent = "    ".repeat(depth);
    let _ = write!(out, "{indent}<Node");
    for (k, v) in attrs {
        let _ = write!(out, " {k}=\"{}\"", esc(v));
    }
    match children {
        Some(ch) if !ch.is_empty() => {
            let _ = writeln!(out, ">");
            out.push_str(ch);
            let _ = writeln!(out, "{indent}</Node>");
        }
        _ => {
            let _ = writeln!(out, " />");
        }
    }
}

fn export_folder(
    out: &mut String,
    folder: &Folder,
    depth: usize,
    password_of: &dyn Fn(u64) -> Option<String>,
    key: &str,
) {
    for sub in &folder.folders {
        let mut inner = String::new();
        export_folder(&mut inner, sub, depth + 1, password_of, key);
        let c = Connection { name: sub.name.clone(), ..Default::default() };
        write_node(out, depth, &node_attributes(&c, "Container", ""), Some(&inner));
    }
    for c in &folder.connections {
        let pw = password_of(c.id).map(|p| encrypt(&p, key, KDF_ITERATIONS)).unwrap_or_default();
        write_node(out, depth, &node_attributes(c, "Connection", &pw), None);
    }
}

/// Erzeugt eine mRemoteNG-kompatible confCons.xml.
/// `master_password` = None -> Standardschlüssel (in mRemoteNG ohne Passwortabfrage zu öffnen).
pub fn export(root: &Folder, master_password: Option<&str>, password_of: &dyn Fn(u64) -> Option<String>) -> String {
    let key = master_password.filter(|p| !p.is_empty()).unwrap_or(DEFAULT_PASSWORD);
    let protected = encrypt(
        if key == DEFAULT_PASSWORD { "ThisIsNotProtected" } else { "ThisIsProtected" },
        key,
        KDF_ITERATIONS,
    );
    let mut nodes = String::new();
    export_folder(&mut nodes, root, 1, password_of, key);
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    let _ = writeln!(
        out,
        "<mrng:Connections xmlns:mrng=\"http://mremoteng.org\" Name=\"{}\" Export=\"false\" EncryptionEngine=\"AES\" BlockCipherMode=\"GCM\" KdfIterations=\"{KDF_ITERATIONS}\" FullFileEncryption=\"false\" Protected=\"{protected}\" ConfVersion=\"2.6\">",
        esc(if root.name.is_empty() { "Verbindungen" } else { &root.name })
    );
    out.push_str(&nodes);
    out.push_str("</mrng:Connections>\n");
    out
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

pub enum ImportError {
    /// Datei ist mit eigenem Master-Passwort geschützt (oder das angegebene ist falsch)
    NeedsPassword,
    Other(String),
}

pub struct Imported {
    pub root: Folder,
    pub passwords: Vec<(u64, String)>,
    pub connections: usize,
    pub skipped: usize,
}

struct Ctx<'a> {
    key: String,
    iterations: u32,
    next_id: &'a mut dyn FnMut() -> u64,
    passwords: Vec<(u64, String)>,
    connections: usize,
    skipped: usize,
}

/// Prüft, ob die Datei ein Master-Passwort braucht (ohne zu importieren).
pub fn needs_password(xml: &str) -> Result<bool, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("Ungültige XML-Datei: {e}"))?;
    let root = doc.root_element();
    let iterations = root.attribute("KdfIterations").and_then(|v| v.parse().ok()).unwrap_or(KDF_ITERATIONS);
    Ok(match root.attribute("Protected") {
        Some(p) if !p.is_empty() => decrypt(p, DEFAULT_PASSWORD, iterations).is_none(),
        _ => false,
    })
}

pub fn import(xml: &str, master_password: Option<&str>, next_id: &mut dyn FnMut() -> u64) -> Result<Imported, ImportError> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| ImportError::Other(format!("Ungültige XML-Datei: {e}")))?;
    let root = doc.root_element();
    if root.tag_name().name() != "Connections" {
        return Err(ImportError::Other("Keine mRemoteNG-Verbindungsdatei (Wurzelelement 'Connections' fehlt).".into()));
    }
    let version: f32 = root.attribute("ConfVersion").and_then(|v| v.parse().ok()).unwrap_or(0.0);
    if version < 2.6 {
        return Err(ImportError::Other(format!(
            "ConfVersion {version} wird nicht unterstützt. Bitte die Datei mit einem aktuellen mRemoteNG (≥ 1.76) einmal speichern."
        )));
    }
    if let Some(mode) = root.attribute("BlockCipherMode") {
        if !mode.eq_ignore_ascii_case("GCM") {
            return Err(ImportError::Other(format!("Verschlüsselungsmodus {mode} wird nicht unterstützt (nur GCM).")));
        }
    }
    let iterations = root.attribute("KdfIterations").and_then(|v| v.parse().ok()).unwrap_or(KDF_ITERATIONS);

    // Schlüssel bestimmen
    let protected = root.attribute("Protected").unwrap_or("");
    let key = if protected.is_empty() {
        master_password.unwrap_or(DEFAULT_PASSWORD).to_string()
    } else if decrypt(protected, DEFAULT_PASSWORD, iterations).is_some() {
        DEFAULT_PASSWORD.to_string()
    } else {
        match master_password {
            Some(pw) if decrypt(protected, pw, iterations).is_some() => pw.to_string(),
            _ => return Err(ImportError::NeedsPassword),
        }
    };

    let mut ctx = Ctx { key, iterations, next_id, passwords: Vec::new(), connections: 0, skipped: 0 };
    let mut folder = Folder { id: (ctx.next_id)(), name: root.attribute("Name").unwrap_or("Verbindungen").to_string(), ..Default::default() };

    let full_file = root.attribute("FullFileEncryption").is_some_and(|v| v.eq_ignore_ascii_case("true"));
    if full_file {
        let text = root.text().unwrap_or("").trim().to_string();
        let inner = decrypt(&text, &ctx.key, ctx.iterations).ok_or(ImportError::NeedsPassword)?;
        let wrapped = format!("<root xmlns:mrng=\"http://mremoteng.org\">{inner}</root>");
        let doc2 = roxmltree::Document::parse(&wrapped)
            .map_err(|e| ImportError::Other(format!("Entschlüsselter Inhalt ungültig: {e}")))?;
        read_children(doc2.root_element(), &HashMap::new(), &mut folder, &mut ctx);
    } else {
        read_children(root, &HashMap::new(), &mut folder, &mut ctx);
    }

    Ok(Imported { root: folder, passwords: ctx.passwords, connections: ctx.connections, skipped: ctx.skipped })
}

type Attrs = HashMap<String, String>;

/// Effektive Attribute eines Knotens inkl. Vererbung ("Inherit…=true") vom Elternordner.
fn effective_attrs(node: roxmltree::Node, parent: &Attrs) -> Attrs {
    let mut a: Attrs = node.attributes().map(|at| (at.name().to_string(), at.value().to_string())).collect();
    let inherited: Vec<String> = a
        .iter()
        .filter(|(k, v)| k.starts_with("Inherit") && v.eq_ignore_ascii_case("true"))
        .map(|(k, _)| k["Inherit".len()..].to_string())
        .collect();
    for key in inherited {
        let target = match key.as_str() {
            "Description" => "Descr",
            "UseConsoleSession" => "ConnectToConsole",
            k => k,
        };
        if let Some(v) = parent.get(target) {
            a.insert(target.to_string(), v.clone());
        }
    }
    a
}

fn read_children(node: roxmltree::Node, parent_attrs: &Attrs, into: &mut Folder, ctx: &mut Ctx) {
    for child in node.children().filter(|n| n.is_element() && n.tag_name().name() == "Node") {
        let attrs = effective_attrs(child, parent_attrs);
        let get = |k: &str| attrs.get(k).map(String::as_str).unwrap_or("");
        match get("Type") {
            "Container" => {
                let mut f = Folder { id: (ctx.next_id)(), name: get("Name").to_string(), ..Default::default() };
                read_children(child, &attrs, &mut f, ctx);
                into.folders.push(f);
            }
            "Connection" => {
                let protocol = match get("Protocol").to_ascii_uppercase().as_str() {
                    "" | "RDP" => Protocol::Rdp,
                    "SSH1" | "SSH2" => Protocol::Ssh,
                    "VNC" => Protocol::Vnc,
                    "HTTP" => Protocol::Http,
                    "HTTPS" => Protocol::Https,
                    _ => {
                        ctx.skipped += 1;
                        continue;
                    }
                };
                let mut c = connection_from(&attrs, (ctx.next_id)());
                c.protocol = protocol;
                if protocol != Protocol::Rdp && attrs.get("Port").is_none() {
                    c.port = protocol.default_port();
                }
                c.icon = match protocol {
                    Protocol::Ssh if c.icon == IconKind::Server => IconKind::Linux,
                    Protocol::Http | Protocol::Https if c.icon == IconKind::Server => IconKind::Web,
                    _ => c.icon,
                };
                let pw = get("Password");
                if !pw.is_empty() {
                    if let Some(plain) = decrypt(pw, &ctx.key, ctx.iterations).filter(|p| !p.is_empty()) {
                        ctx.passwords.push((c.id, plain));
                    }
                }
                ctx.connections += 1;
                into.connections.push(c);
            }
            _ => {}
        }
    }
}

fn connection_from(a: &Attrs, id: u64) -> Connection {
    let get = |k: &str| a.get(k).map(String::as_str).unwrap_or("");
    let flag = |k: &str, default: bool| match a.get(k) {
        Some(v) => v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("all") || v.eq_ignore_ascii_case("yes"),
        None => default,
    };
    let d = Connection::default();
    let (resolution, fw, fh) = match get("Resolution") {
        "FitToWindow" | "" => (Resolution::FitToPanel, d.fixed_width, d.fixed_height),
        "SmartSize" => (Resolution::SmartSize, d.fixed_width, d.fixed_height),
        "Fullscreen" => (Resolution::Fullscreen, d.fixed_width, d.fixed_height),
        r => {
            let dims = r.trim_start_matches("Res").split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
            match dims {
                Some((w, h)) => (Resolution::Fixed, w, h),
                None => (Resolution::FitToPanel, d.fixed_width, d.fixed_height),
            }
        }
    };
    Connection {
        id,
        name: get("Name").to_string(),
        description: get("Descr").to_string(),
        icon: icon_from(get("Icon")),
        host: get("Hostname").to_string(),
        username: get("Username").to_string(),
        domain: get("Domain").to_string(),
        inherit_credentials: false,
        protocol: Protocol::Rdp,
        ssh_key_file: String::new(),
        ssh_extra_args: String::new(),
        ssh_agent_forward: false,
        forwards: Vec::new(),
        via_ssh: 0,
        multimon: false,
        monitors: String::new(),
        vault_entry: String::new(),
        mac: String::new(),
        tags: String::new(),
        color: crate::model::TabColor::None,
        memo: String::new(),
        web_path: String::new(),
        ignore_cert_errors: false,
        port: get("Port").parse().unwrap_or(3389),
        admin_session: flag("ConnectToConsole", false),
        auth_level: match get("RDPAuthenticationLevel") {
            "NoAuth" => AuthLevel::AlwaysConnect,
            "AuthRequired" => AuthLevel::DontConnect,
            _ => AuthLevel::Warn,
        },
        idle_minutes: get("RDPMinutesToIdleTimeout").parse().unwrap_or(0),
        load_balance_info: get("LoadBalanceInfo").to_string(),
        use_credssp: flag("UseCredSsp", true),
        gateway_usage: match get("RDGatewayUsageMethod") {
            "Always" => GatewayUsage::Always,
            "Detect" => GatewayUsage::Detect,
            _ => GatewayUsage::Never,
        },
        gateway: get("RDGatewayHostname").to_string(),
        resolution,
        fixed_width: fw,
        fixed_height: fh,
        auto_resize: flag("AutomaticResize", true),
        colors: match get("Colors") {
            "Colors15Bit" => 15,
            "Colors16Bit" | "Colors256" => 16,
            "Colors24Bit" => 24,
            _ => 32,
        },
        bitmap_cache: flag("CacheBitmaps", false),
        wallpaper: flag("DisplayWallpaper", false),
        themes: flag("DisplayThemes", false),
        font_smoothing: flag("EnableFontSmoothing", false),
        desktop_composition: flag("EnableDesktopComposition", false),
        redirect_keys: flag("RedirectKeys", false),
        redirect_drives: flag("RedirectDiskDrives", false),
        redirect_printers: flag("RedirectPrinters", false),
        redirect_ports: flag("RedirectPorts", false),
        redirect_smartcards: flag("RedirectSmartCards", false),
        redirect_clipboard: flag("RedirectClipboard", true),
        sound: match get("RedirectSound") {
            "BringToThisComputer" => SoundMode::BringToThisComputer,
            "LeaveAtRemoteComputer" => SoundMode::LeaveAtRemote,
            _ => SoundMode::DoNotPlay,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Protected`-Wert aus einer echten mRemoteNG-Datei
    #[test]
    fn decrypt_real_mremoteng_value() {
        let p = "B04CwFQGeCYgYrZTUPkz77PKGVrtxR2th/QrwaV9NklQduNg3ei1aAKRZfzeDCLnWMPYW5nhrFVppOrtW05k0SXn";
        let plain = decrypt(p, DEFAULT_PASSWORD, 1000);
        assert_eq!(plain.as_deref(), Some("ThisIsNotProtected"));
    }

    #[test]
    fn roundtrip_crypto() {
        let c = encrypt("geheim€", DEFAULT_PASSWORD, 1000);
        assert_eq!(decrypt(&c, DEFAULT_PASSWORD, 1000).as_deref(), Some("geheim€"));
        assert!(decrypt(&c, "falsch", 1000).is_none());
    }

    #[test]
    fn roundtrip_export_import() {
        let mut root = Folder { id: 0, name: "Verbindungen".into(), ..Default::default() };
        let mut sub = Folder { id: 1, name: "Prod & Test".into(), ..Default::default() };
        sub.connections.push(Connection {
            id: 2,
            name: "AUFDC3".into(),
            host: "aufdc3".into(),
            username: "corp\\jdoe".into(),
            domain: "corp.example.com".into(),
            colors: 16,
            resolution: Resolution::Fixed,
            fixed_width: 1280,
            fixed_height: 720,
            auth_level: AuthLevel::AlwaysConnect,
            ..Default::default()
        });
        root.folders.push(sub);
        let xml = export(&root, None, &|id| (id == 2).then(|| "pa\"ss".to_string()));
        assert!(!needs_password(&xml).unwrap());
        let mut n = 100;
        let mut next = || {
            n += 1;
            n
        };
        let Ok(imp) = import(&xml, None, &mut next) else { panic!("import") };
        assert_eq!(imp.connections, 1);
        let c = &imp.root.folders[0].connections[0];
        assert_eq!(imp.root.folders[0].name, "Prod & Test");
        assert_eq!(c.username, "corp\\jdoe");
        assert_eq!(c.colors, 16);
        assert_eq!((c.resolution, c.fixed_width, c.fixed_height), (Resolution::Fixed, 1280, 720));
        assert_eq!(c.auth_level, AuthLevel::AlwaysConnect);
        assert_eq!(imp.passwords, vec![(c.id, "pa\"ss".to_string())]);

        // mit Master-Passwort
        let xml = export(&root, Some("Master1"), &|_| Some("x".into()));
        assert!(needs_password(&xml).unwrap());
        assert!(matches!(import(&xml, None, &mut next), Err(ImportError::NeedsPassword)));
        assert!(import(&xml, Some("Master1"), &mut next).is_ok());
    }

    #[test]
    fn inheritance() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<mrng:Connections xmlns:mrng="http://mremoteng.org" Name="Verbindungen" Export="false" EncryptionEngine="AES" BlockCipherMode="GCM" KdfIterations="1000" FullFileEncryption="false" Protected="" ConfVersion="2.6">
  <Node Name="Ordner" Type="Container" Username="corp\jdoe" Domain="corp" Colors="Colors16Bit">
    <Node Name="A" Type="Connection" Hostname="a" Protocol="RDP" Username="" InheritUsername="true" InheritDomain="true" InheritColors="true" />
    <Node Name="S" Type="Connection" Hostname="s" Protocol="SSH2" Port="2222" />
    <Node Name="V" Type="Connection" Hostname="v" Protocol="VNC" />
    <Node Name="T" Type="Connection" Hostname="t" Protocol="Telnet" />
  </Node>
</mrng:Connections>"#;
        let mut n = 0;
        let mut next = || {
            n += 1;
            n
        };
        let Ok(imp) = import(xml, None, &mut next) else { panic!("import") };
        let c = &imp.root.folders[0].connections[0];
        assert_eq!((c.username.as_str(), c.domain.as_str(), c.colors), ("corp\\jdoe", "corp", 16));
        let s = &imp.root.folders[0].connections[1];
        assert_eq!((s.protocol, s.port), (Protocol::Ssh, 2222));
        let v = &imp.root.folders[0].connections[2];
        assert_eq!((v.protocol, v.port), (Protocol::Vnc, 5900));
        assert_eq!(imp.skipped, 1, "Telnet wird übersprungen");
    }
}
