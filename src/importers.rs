//! Import aus anderen Programmen: Remote Desktop Connection Manager (.rdg), PuTTY-Sitzungen
//! (Registrierung), OpenSSH-Konfiguration (~/.ssh/config) und CSV (z. B. Royal TS).

use crate::model::{Connection, Folder, IconKind, Protocol};
use std::collections::HashMap;

pub struct Imported {
    pub root: Folder,
    /// (Verbindungs- oder Ordner-ID, Passwort)
    pub passwords: Vec<(u64, String)>,
    pub connections: usize,
    pub skipped: usize,
    /// Hinweise für den Benutzer
    pub notes: Vec<String>,
}

impl Imported {
    fn new(name: &str, next_id: &mut dyn FnMut() -> u64) -> Self {
        Self {
            root: Folder { id: next_id(), name: name.into(), ..Default::default() },
            passwords: Vec::new(),
            connections: 0,
            skipped: 0,
            notes: Vec::new(),
        }
    }
}

fn icon_for(p: Protocol) -> IconKind {
    match p {
        Protocol::Ssh => IconKind::Linux,
        Protocol::Http | Protocol::Https => IconKind::Web,
        _ => IconKind::Server,
    }
}

fn new_conn(id: u64, name: &str, host: &str, protocol: Protocol) -> Connection {
    Connection {
        id,
        name: name.trim().to_string(),
        host: host.trim().to_string(),
        protocol,
        port: protocol.default_port(),
        icon: icon_for(protocol),
        ignore_cert_errors: protocol.is_web(),
        ..Default::default()
    }
}

/// "DOMÄNE\benutzer" → (domäne, benutzer)
fn split_user(user: &str) -> (String, String) {
    match user.trim().split_once('\\') {
        Some((d, u)) => (d.to_string(), u.to_string()),
        None => (String::new(), user.trim().to_string()),
    }
}

// ---------------------------------------------------------------------------
// RDCMan (.rdg)
// ---------------------------------------------------------------------------

/// Passwort aus einer RDCMan-Datei (DPAPI, nur für denselben Windows-Benutzer lesbar)
fn dpapi_decrypt(b64: &str) -> Option<String> {
    use base64::Engine as _;
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData};
    let mut data = base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok()?;
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_mut_ptr() };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).ok()?;
        let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize);
        let wide: Vec<u16> = bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut std::ffi::c_void)));
        Some(String::from_utf16_lossy(&wide).trim_end_matches('\0').to_string())
    }
}

pub fn rdcman(xml: &str, next_id: &mut dyn FnMut() -> u64) -> Result<Imported, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("Ungültige RDCMan-Datei: {e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "RDCMan" {
        return Err("Keine RDCMan-Datei (Wurzelelement 'RDCMan' fehlt).".into());
    }
    let file = child(root, "file").unwrap_or(root);
    let name = prop(file, "name").unwrap_or_else(|| "RDCMan".into());
    let mut imp = Imported::new(&name, next_id);
    let mut failed_pw = 0;
    let mut folder = std::mem::take(&mut imp.root);
    rdg_group(file, &mut folder, &mut imp, next_id, &mut failed_pw);
    imp.root = folder;
    if failed_pw > 0 {
        imp.notes.push(format!(
            "{failed_pw} Passwort/Passwörter konnten nicht entschlüsselt werden (nur der Windows-Benutzer, der die Datei gespeichert hat, kann sie lesen)"
        ));
    }
    Ok(imp)
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

fn text_of(n: roxmltree::Node, name: &str) -> Option<String> {
    child(n, name).and_then(|c| c.text()).map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// Wert aus <properties> (Schema 3) oder direkt unter dem Knoten (ältere Schemas)
fn prop(n: roxmltree::Node, name: &str) -> Option<String> {
    child(n, "properties").and_then(|p| text_of(p, name)).or_else(|| text_of(n, name))
}

/// Anmeldedaten (Benutzer, Domäne, Passwort) eines Knotens, falls nicht geerbt
fn rdg_creds(n: roxmltree::Node, failed: &mut usize) -> Option<(String, String, Option<String>)> {
    let c = child(n, "logonCredentials")?;
    if c.attribute("inherit").is_some_and(|v| v != "None") {
        return None;
    }
    let user = text_of(c, "userName").unwrap_or_default();
    let domain = text_of(c, "domain").unwrap_or_default();
    let pw = text_of(c, "password").and_then(|p| {
        let plain = dpapi_decrypt(&p);
        if plain.is_none() {
            *failed += 1;
        }
        plain
    });
    (!user.is_empty() || pw.is_some()).then_some((user, domain, pw))
}

fn rdg_group(n: roxmltree::Node, into: &mut Folder, imp: &mut Imported, next_id: &mut dyn FnMut() -> u64, failed: &mut usize) {
    for c in n.children().filter(|c| c.is_element()) {
        match c.tag_name().name() {
            "group" => {
                let mut f = Folder { id: next_id(), name: prop(c, "name").unwrap_or_else(|| "Gruppe".into()), ..Default::default() };
                if let Some((user, domain, pw)) = rdg_creds(c, failed) {
                    f.username = user;
                    f.domain = domain;
                    if let Some(pw) = pw {
                        imp.passwords.push((f.id, pw));
                    }
                }
                rdg_group(c, &mut f, imp, next_id, failed);
                into.folders.push(f);
            }
            "server" => {
                let Some(host) = prop(c, "name") else { continue };
                let display = prop(c, "displayName").unwrap_or_else(|| host.clone());
                let mut conn = new_conn(next_id(), &display, &host, Protocol::Rdp);
                conn.description = prop(c, "comment").unwrap_or_default();
                match rdg_creds(c, failed) {
                    Some((user, domain, pw)) => {
                        conn.username = user;
                        conn.domain = domain;
                        if let Some(pw) = pw {
                            imp.passwords.push((conn.id, pw));
                        }
                    }
                    None => conn.inherit_credentials = child(c, "logonCredentials").is_some() || !into.username.is_empty(),
                }
                if let Some(s) = child(c, "connectionSettings").filter(|s| s.attribute("inherit") == Some("None")) {
                    if let Some(p) = text_of(s, "port").and_then(|p| p.parse().ok()) {
                        conn.port = p;
                    }
                    conn.admin_session = text_of(s, "connectToConsole").is_some_and(|v| v.eq_ignore_ascii_case("true"));
                    if let Some(lb) = text_of(s, "loadBalanceInfo") {
                        conn.load_balance_info = lb;
                    }
                }
                if let Some(g) = child(c, "gatewaySettings").filter(|s| s.attribute("inherit") == Some("None"))
                    && text_of(g, "enabled").is_some_and(|v| v.eq_ignore_ascii_case("true"))
                    && let Some(h) = text_of(g, "hostName")
                {
                    conn.gateway = h;
                    conn.gateway_usage = crate::model::GatewayUsage::Always;
                }
                imp.connections += 1;
                into.connections.push(conn);
            }
            "smartGroup" => imp.skipped += 1,
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// PuTTY
// ---------------------------------------------------------------------------

/// Eine PuTTY-Sitzung aus der Registrierung
#[derive(Default, Debug)]
pub struct PuttySession {
    pub name: String,
    pub host: String,
    pub port: u32,
    pub protocol: String,
    pub user: String,
    pub key_file: String,
}

/// PuTTY speichert Sitzungsnamen URL-kodiert ("Mein%20Server")
fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn read_putty_sessions() -> Result<Vec<PuttySession>, String> {
    use windows::Win32::System::Registry::*;
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    let base = HSTRING::from(r"Software\SimonTatham\PuTTY\Sessions");
    let mut key = HKEY::default();
    let r = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(base.as_ptr()), None, KEY_READ, &mut key) };
    if r.is_err() {
        return Err("Keine PuTTY-Sitzungen gefunden (PuTTY ist für diesen Benutzer nicht eingerichtet).".into());
    }
    let mut out = Vec::new();
    let mut index = 0u32;
    loop {
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let r = unsafe { RegEnumKeyExW(key, index, Some(PWSTR(buf.as_mut_ptr())), &mut len, None, None, None, None) };
        if r.is_err() {
            break;
        }
        index += 1;
        let sub = String::from_utf16_lossy(&buf[..len as usize]);
        let subkey = HSTRING::from(sub.as_str());
        let read_sz = |name: &str| -> String {
            let n = HSTRING::from(name);
            let mut data = [0u16; 1024];
            let mut size = (data.len() * 2) as u32;
            let r = unsafe {
                RegGetValueW(
                    key,
                    PCWSTR(subkey.as_ptr()),
                    PCWSTR(n.as_ptr()),
                    RRF_RT_REG_SZ,
                    None,
                    Some(data.as_mut_ptr() as *mut std::ffi::c_void),
                    Some(&mut size),
                )
            };
            if r.is_err() {
                return String::new();
            }
            let chars = (size as usize / 2).saturating_sub(1).min(data.len());
            String::from_utf16_lossy(&data[..chars])
        };
        let read_dword = |name: &str| -> u32 {
            let n = HSTRING::from(name);
            let mut v = 0u32;
            let mut size = 4u32;
            let r = unsafe {
                RegGetValueW(
                    key,
                    PCWSTR(subkey.as_ptr()),
                    PCWSTR(n.as_ptr()),
                    RRF_RT_REG_DWORD,
                    None,
                    Some(&mut v as *mut u32 as *mut std::ffi::c_void),
                    Some(&mut size),
                )
            };
            if r.is_err() { 0 } else { v }
        };
        out.push(PuttySession {
            name: url_decode(&sub),
            host: read_sz("HostName"),
            port: read_dword("PortNumber"),
            protocol: read_sz("Protocol"),
            user: read_sz("UserName"),
            key_file: read_sz("PublicKeyFile"),
        });
    }
    unsafe {
        let _ = RegCloseKey(key);
    }
    Ok(out)
}

pub fn putty(sessions: &[PuttySession], next_id: &mut dyn FnMut() -> u64) -> Imported {
    let mut imp = Imported::new("PuTTY", next_id);
    let mut ppk = 0;
    for s in sessions {
        if s.name == "Default Settings" || s.host.trim().is_empty() {
            continue;
        }
        if !s.protocol.is_empty() && !s.protocol.eq_ignore_ascii_case("ssh") {
            imp.skipped += 1; // Telnet, seriell, raw …
            continue;
        }
        // "benutzer@host" im Hostfeld
        let (user, host) = match s.host.trim().rsplit_once('@') {
            Some((u, h)) => (u.to_string(), h.to_string()),
            None => (s.user.trim().to_string(), s.host.trim().to_string()),
        };
        let mut c = new_conn(next_id(), &s.name, &host, Protocol::Ssh);
        c.username = user;
        if s.port != 0 {
            c.port = s.port as u16;
        }
        let key = s.key_file.trim();
        if key.to_ascii_lowercase().ends_with(".ppk") {
            ppk += 1;
            c.memo = format!("PuTTY-Schlüssel: {key}\n(.ppk muss für OpenSSH umgewandelt werden: PuTTYgen > Conversions > Export OpenSSH key)");
        } else if !key.is_empty() {
            c.ssh_key_file = key.to_string();
        }
        imp.connections += 1;
        imp.root.connections.push(c);
    }
    if ppk > 0 {
        imp.notes.push(format!("{ppk} Sitzung(en) nutzen .ppk-Schlüssel – diese müssen für OpenSSH umgewandelt werden (Hinweis in den Notizen)"));
    }
    imp
}

// ---------------------------------------------------------------------------
// OpenSSH (~/.ssh/config)
// ---------------------------------------------------------------------------

pub fn ssh_config_path() -> std::path::PathBuf {
    std::env::var_os("USERPROFILE").map(std::path::PathBuf::from).unwrap_or_default().join(".ssh").join("config")
}

pub fn ssh_config(text: &str, next_id: &mut dyn FnMut() -> u64) -> Imported {
    let mut imp = Imported::new("SSH-Konfiguration", next_id);
    // (Aliasse, Optionen)
    let mut blocks: Vec<(Vec<String>, HashMap<String, String>)> = Vec::new();
    let mut in_match = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (k.to_ascii_lowercase(), v.trim().trim_start_matches('=').trim().trim_matches('"').to_string()),
            None => continue,
        };
        match key.as_str() {
            "host" => {
                in_match = false;
                blocks.push((value.split_whitespace().map(String::from).collect(), HashMap::new()));
            }
            "match" => in_match = true,
            "include" => imp.notes.push(format!("„Include {value}“ wurde nicht ausgewertet")),
            _ if in_match => {}
            _ => {
                if let Some((_, opts)) = blocks.last_mut() {
                    // erster Wert gilt (wie bei ssh)
                    opts.entry(key).or_insert(value);
                }
            }
        }
    }
    for (aliases, opts) in blocks {
        for alias in aliases {
            if alias.contains(['*', '?', '!']) {
                continue; // Muster, kein Host
            }
            let host = opts.get("hostname").cloned().unwrap_or_else(|| alias.clone());
            let mut c = new_conn(next_id(), &alias, &host, Protocol::Ssh);
            if let Some(u) = opts.get("user") {
                c.username = u.clone();
            }
            if let Some(p) = opts.get("port").and_then(|p| p.parse().ok()) {
                c.port = p;
            }
            if let Some(k) = opts.get("identityfile") {
                c.ssh_key_file = k.clone();
            }
            if let Some(j) = opts.get("proxyjump").filter(|j| !j.eq_ignore_ascii_case("none")) {
                c.ssh_extra_args = format!("-J {j}");
            }
            if opts.get("forwardagent").is_some_and(|v| v.eq_ignore_ascii_case("yes")) {
                c.ssh_agent_forward = true;
            }
            imp.connections += 1;
            imp.root.connections.push(c);
        }
    }
    imp
}

// ---------------------------------------------------------------------------
// CSV (Royal TS und andere)
// ---------------------------------------------------------------------------

/// Eine CSV-Zeile mit Anführungszeichen ("a ""b""", c) zerlegen
fn csv_fields(line: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == sep && !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|f| f.trim().to_string()).collect()
}

/// Zeilen zusammensetzen, die Zeilenumbrüche in Anführungszeichen enthalten
fn csv_records(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in text.lines() {
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(line);
        if cur.matches('"').count() % 2 == 0 {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[derive(Clone, Copy, PartialEq)]
enum Col {
    Name,
    Host,
    Port,
    Protocol,
    User,
    Domain,
    Password,
    Description,
    Folder,
}

fn column(header: &str) -> Option<Col> {
    let h: String = header.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
    Some(match h.as_str() {
        "name" | "displayname" | "title" | "anzeigename" | "titel" => Col::Name,
        "host" | "hostname" | "uri" | "url" | "computername" | "computer" | "address" | "adresse" | "ip" | "ipaddress"
        | "server" | "servername" => Col::Host,
        "port" => Col::Port,
        "protocol" | "protokoll" | "type" | "typ" | "connectiontype" | "objecttype" => Col::Protocol,
        "user" | "username" | "benutzer" | "benutzername" | "login" | "credentialusername" | "credentialuser" => Col::User,
        "domain" | "domäne" | "domane" | "credentialdomain" => Col::Domain,
        "password" | "passwort" | "kennwort" | "credentialpassword" => Col::Password,
        "description" | "beschreibung" | "comment" | "kommentar" | "notes" | "notizen" => Col::Description,
        "folder" | "ordner" | "path" | "pfad" | "group" | "gruppe" | "parent" | "folderpath" => Col::Folder,
        _ => return None,
    })
}

fn protocol_of(kind: &str, host: &str) -> Option<Protocol> {
    let k = kind.to_lowercase();
    let h = host.to_lowercase();
    let has = |s: &str| k.contains(s);
    if h.starts_with("https://") || has("https") {
        Some(Protocol::Https)
    } else if h.starts_with("http://") || has("http") || has("web") {
        Some(Protocol::Http)
    } else if h.starts_with("ssh://") || has("ssh") || has("terminal") || has("putty") {
        Some(Protocol::Ssh)
    } else if h.starts_with("vnc://") || has("vnc") {
        Some(Protocol::Vnc)
    } else if h.starts_with("rdp://") || k.is_empty() || has("rdp") || has("rds") || has("remotedesktop") || has("remote desktop") {
        Some(Protocol::Rdp)
    } else {
        None
    }
}

fn folder_path<'a>(root: &'a mut Folder, path: &str, next_id: &mut dyn FnMut() -> u64) -> &'a mut Folder {
    let mut cur = root;
    for part in path.split(['/', '\\']).map(str::trim).filter(|p| !p.is_empty()) {
        let pos = match cur.folders.iter().position(|f| f.name == part) {
            Some(p) => p,
            None => {
                cur.folders.push(Folder { id: next_id(), name: part.to_string(), ..Default::default() });
                cur.folders.len() - 1
            }
        };
        cur = &mut cur.folders[pos];
    }
    cur
}

pub fn csv(text: &str, next_id: &mut dyn FnMut() -> u64) -> Result<Imported, String> {
    let text = text.trim_start_matches('\u{feff}');
    let records = csv_records(text);
    let header_line = records.first().ok_or("Die Datei ist leer.")?;
    // Trennzeichen: das häufigste von ; , Tab in der Kopfzeile
    let sep = [';', ',', '\t'].into_iter().max_by_key(|s| header_line.matches(*s).count()).unwrap_or(',');
    let header: Vec<Option<Col>> = csv_fields(header_line, sep).iter().map(|h| column(h)).collect();
    if !header.contains(&Some(Col::Host)) {
        return Err("Keine Spalte für den Host gefunden (erwartet z. B. „Host“, „URI“, „ComputerName“ oder „Address“).".into());
    }
    let mut imp = Imported::new("CSV-Import", next_id);
    for rec in records.iter().skip(1) {
        if rec.trim().is_empty() {
            continue;
        }
        let fields = csv_fields(rec, sep);
        let get = |col: Col| -> String {
            header.iter().position(|h| *h == Some(col)).and_then(|i| fields.get(i)).cloned().unwrap_or_default()
        };
        let raw_host = get(Col::Host);
        if raw_host.is_empty() {
            imp.skipped += 1;
            continue;
        }
        let Some(protocol) = protocol_of(&get(Col::Protocol), &raw_host) else {
            imp.skipped += 1;
            continue;
        };
        // "rdp://host:port", "host:port"
        let mut host = raw_host.clone();
        let mut port = None;
        if !protocol.is_web() {
            if let Some((_, rest)) = host.split_once("://") {
                host = rest.trim_end_matches('/').to_string();
            }
            if let Some((h, p)) = host.rsplit_once(':')
                && let Ok(p) = p.parse::<u16>()
                && !h.contains(':')
            {
                port = Some(p);
                host = h.to_string();
            }
        }
        let name = get(Col::Name);
        let mut c = new_conn(next_id(), if name.is_empty() { &host } else { &name }, &host, protocol);
        if let Some(p) = get(Col::Port).parse().ok().or(port) {
            c.port = p;
        }
        let (d, u) = split_user(&get(Col::User));
        c.username = u;
        c.domain = if get(Col::Domain).is_empty() { d } else { get(Col::Domain) };
        c.description = get(Col::Description);
        let pw = get(Col::Password);
        if !pw.is_empty() {
            imp.passwords.push((c.id, pw));
        }
        let path = get(Col::Folder);
        let folder = folder_path(&mut imp.root, &path, next_id);
        folder.connections.push(c);
        imp.connections += 1;
    }
    Ok(imp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> impl FnMut() -> u64 {
        let mut n = 100;
        move || {
            n += 1;
            n
        }
    }

    #[test]
    fn rdg() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<RDCMan programVersion="2.7" schemaVersion="3">
  <file>
    <properties><expanded>True</expanded><name>Firma</name></properties>
    <group>
      <properties><name>DCs</name></properties>
      <logonCredentials inherit="None"><profileName scope="Local">Custom</profileName><userName>admin</userName><password></password><domain>CORP</domain></logonCredentials>
      <server>
        <properties><displayName>DC 1</displayName><name>dc01.corp.local</name><comment>primär</comment></properties>
      </server>
      <server>
        <properties><name>dc02</name></properties>
        <connectionSettings inherit="None"><connectToConsole>True</connectToConsole><port>3390</port></connectionSettings>
        <logonCredentials inherit="None"><userName>jdoe</userName><domain>X</domain></logonCredentials>
      </server>
    </group>
    <server><name>alt-schema</name><displayName>Alt</displayName></server>
  </file>
</RDCMan>"#;
        let mut next = ids();
        let imp = rdcman(xml, &mut next).unwrap();
        assert_eq!(imp.root.name, "Firma");
        assert_eq!(imp.connections, 3);
        let g = &imp.root.folders[0];
        assert_eq!((g.name.as_str(), g.username.as_str(), g.domain.as_str()), ("DCs", "admin", "CORP"));
        let dc1 = &g.connections[0];
        assert_eq!((dc1.name.as_str(), dc1.host.as_str(), dc1.description.as_str()), ("DC 1", "dc01.corp.local", "primär"));
        assert!(dc1.inherit_credentials);
        let dc2 = &g.connections[1];
        assert_eq!((dc2.port, dc2.admin_session, dc2.username.as_str(), dc2.inherit_credentials), (3390, true, "jdoe", false));
        assert_eq!(imp.root.connections[0].host, "alt-schema");
    }

    #[test]
    fn putty_sessions() {
        let s = vec![
            PuttySession { name: "Default Settings".into(), ..Default::default() },
            PuttySession { name: "Mein Server".into(), host: "root@linux01".into(), port: 2222, protocol: "ssh".into(), ..Default::default() },
            PuttySession { name: "Switch".into(), host: "sw1".into(), port: 23, protocol: "telnet".into(), ..Default::default() },
            PuttySession { name: "K".into(), host: "k".into(), port: 22, protocol: "ssh".into(), key_file: r"C:\k.ppk".into(), ..Default::default() },
        ];
        let mut next = ids();
        let imp = putty(&s, &mut next);
        assert_eq!((imp.connections, imp.skipped), (2, 1));
        let c = &imp.root.connections[0];
        assert_eq!((c.name.as_str(), c.host.as_str(), c.username.as_str(), c.port), ("Mein Server", "linux01", "root", 2222));
        assert!(imp.root.connections[1].ssh_key_file.is_empty());
        assert_eq!(url_decode("Mein%20Server%2Fx"), "Mein Server/x");
    }

    #[test]
    fn ssh_cfg() {
        let text = "Host *\n  ServerAliveInterval 30\n\nHost web web-alias\n  HostName 10.0.0.5\n  User deploy\n  Port 2200\n  IdentityFile ~/.ssh/id_web\n  ProxyJump bastion\n\nHost bastion\n  HostName=bastion.example.org\n  ForwardAgent yes\nMatch host foo\n  User x\n";
        let mut next = ids();
        let imp = ssh_config(text, &mut next);
        assert_eq!(imp.connections, 3);
        let web = &imp.root.connections[0];
        assert_eq!((web.name.as_str(), web.host.as_str(), web.username.as_str(), web.port), ("web", "10.0.0.5", "deploy", 2200));
        assert_eq!((web.ssh_key_file.as_str(), web.ssh_extra_args.as_str()), ("~/.ssh/id_web", "-J bastion"));
        let b = &imp.root.connections[2];
        assert_eq!((b.host.as_str(), b.ssh_agent_forward, b.username.as_str()), ("bastion.example.org", true, ""));
    }

    #[test]
    fn csv_royal() {
        let text = "\u{feff}\"Name\";\"URI\";\"Type\";\"CredentialUsername\";\"Description\";\"Folder\"\n\
            \"DC 1\";\"dc01:3390\";\"RoyalRDSConnection\";\"CORP\\admin\";\"mit; Semikolon\";\"Firma/DCs\"\n\
            \"Linux\";\"ssh://linux01\";\"RoyalSSHConnection\";\"root\";\"\";\"Firma\"\n\
            \"iLO\";\"https://ilo01/\";\"RoyalWebConnection\";\"\";\"zwei\nZeilen\";\"\"\n\
            \"Datei\";\"x\";\"RoyalFileTransferConnection\";\"\";\"\";\"\"\n";
        let mut next = ids();
        let imp = csv(text, &mut next).unwrap();
        assert_eq!((imp.connections, imp.skipped), (3, 1));
        let firma = &imp.root.folders[0];
        let dc = &firma.folders[0].connections[0];
        assert_eq!((dc.host.as_str(), dc.port, dc.domain.as_str(), dc.username.as_str()), ("dc01", 3390, "CORP", "admin"));
        assert_eq!(dc.description, "mit; Semikolon");
        let lin = &firma.connections[0];
        assert_eq!((lin.protocol, lin.host.as_str(), lin.port), (Protocol::Ssh, "linux01", 22));
        let ilo = &imp.root.connections[0];
        assert_eq!((ilo.protocol, ilo.host.as_str()), (Protocol::Https, "https://ilo01/"));
        assert_eq!(ilo.description, "zwei\nZeilen");
        assert!(csv("Name;Foo\n", &mut next).is_err());
    }
}
