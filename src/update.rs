//! Update-Prüfung: sucht in einem Ordner (z. B. Netzlaufwerk) oder unter einer URL nach einer
//! neueren „RdpManager-x.y.z-x64.msi“ und startet auf Wunsch den Installer.

use std::os::windows::process::CommandExt as _;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub type Version = (u32, u32, u32);

pub fn parse_version(s: &str) -> Option<Version> {
    let mut it = s.trim().split('.').map(|p| p.parse::<u32>().ok());
    let v = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some(v)
}

pub fn current() -> Version {
    parse_version(env!("CARGO_PKG_VERSION")).unwrap_or((0, 0, 0))
}

pub fn version_text(v: Version) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

/// Alle „RdpManager-x.y.z-x64.msi“ in einem Text (Ordnerliste, HTML-Seite …): (Version, Dateiname)
pub fn find_installers(text: &str) -> Vec<(Version, String)> {
    const PREFIX: &str = "RdpManager-";
    const SUFFIX: &str = "-x64.msi";
    let mut out: Vec<(Version, String)> = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(PREFIX) {
        let after = &rest[i + PREFIX.len()..];
        if let Some(end) = after.find(SUFFIX) {
            let ver = &after[..end];
            if ver.len() <= 16
                && let Some(v) = parse_version(ver)
            {
                let name = format!("{PREFIX}{ver}{SUFFIX}");
                if !out.iter().any(|(_, n)| *n == name) {
                    out.push((v, name));
                }
            }
        }
        rest = after;
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub version: Version,
    /// Pfad (Ordner) oder URL der MSI-Datei
    pub location: String,
}

fn is_url(s: &str) -> bool {
    let l = s.trim().to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

/// Aufruf von curl.exe (seit Windows 10 enthalten)
fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("curl.exe")
        .args(["-fsSL", "--max-time", "120"])
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("curl.exe konnte nicht gestartet werden: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() { format!("Download fehlgeschlagen (Code {:?})", out.status.code()) } else { err });
    }
    Ok(out.stdout)
}

/// Offizielle Quelle, wenn nichts eingetragen ist
pub const DEFAULT_SOURCE: &str = "https://github.com/Yuji2k/rdp-manager";

/// Eingetragene Quelle oder – wenn leer – die offizielle GitHub-Seite
pub fn effective_source(s: &str) -> &str {
    let s = s.trim().trim_matches('"');
    if s.is_empty() { DEFAULT_SOURCE } else { s }
}

/// "https://github.com/besitzer/repo[/releases…]" oder "github.com/besitzer/repo" → (besitzer, repo)
pub fn github_repo(source: &str) -> Option<(String, String)> {
    let s = source.trim().trim_end_matches('/');
    let rest = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://")).unwrap_or(s);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let path = rest.strip_prefix("github.com/")?;
    let mut parts = path.split('/');
    let owner = parts.next().filter(|p| !p.is_empty())?;
    let repo = parts.next().filter(|p| !p.is_empty())?.trim_end_matches(".git");
    Some((owner.to_string(), repo.to_string()))
}

/// Antwort von api.github.com/repos/…/releases auswerten: höchste Version mit MSI (ohne Entwürfe/Vorabversionen)
fn from_github_json(json: &str) -> Result<Option<Found>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("GitHub: unerwartete Antwort ({e})"))?;
    if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
        return Err(format!("GitHub: {msg}"));
    }
    let releases: Vec<&serde_json::Value> = match v.as_array() {
        Some(a) => a.iter().collect(),
        None => vec![&v],
    };
    let mut best: Option<Found> = None;
    for r in releases {
        if r.get("draft").and_then(|d| d.as_bool()) == Some(true) || r.get("prerelease").and_then(|d| d.as_bool()) == Some(true) {
            continue;
        }
        for a in r.get("assets").and_then(|a| a.as_array()).into_iter().flatten() {
            let name = a.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let url = a.get("browser_download_url").and_then(|n| n.as_str()).unwrap_or("");
            if let Some((ver, _)) = find_installers(name).into_iter().next()
                && !url.is_empty()
                && best.as_ref().is_none_or(|b| ver > b.version)
            {
                best = Some(Found { version: ver, location: url.to_string() });
            }
        }
    }
    Ok(best)
}

/// Neueste Version an der Quelle suchen; `None` = keine neuere als die laufende
pub fn check(source: &str) -> Result<Option<Found>, String> {
    let source = effective_source(source);
    let best = if let Some((owner, repo)) = github_repo(source) {
        let api = format!("https://api.github.com/repos/{owner}/{repo}/releases?per_page=10");
        let body = curl(&["-H", "Accept: application/vnd.github+json", &api]).map_err(|e| {
            if e.contains("404") { format!("GitHub-Repository {owner}/{repo} nicht gefunden.") } else { format!("GitHub: {e}") }
        })?;
        from_github_json(&String::from_utf8_lossy(&body))?
    } else if is_url(source) {
        // direkte MSI-Adresse oder Seite/Ordnerliste mit Links
        let (list, base) = if source.to_ascii_lowercase().ends_with(".msi") {
            let name = source.rsplit('/').next().unwrap_or("").to_string();
            (find_installers(&name), source[..source.len() - name.len()].to_string())
        } else {
            let text = String::from_utf8_lossy(&curl(&[source])?).into_owned();
            let base = if source.ends_with('/') { source.to_string() } else { format!("{source}/") };
            (find_installers(&text), base)
        };
        list.into_iter().max_by_key(|(v, _)| *v).map(|(v, name)| Found { version: v, location: format!("{base}{name}") })
    } else {
        let dir = PathBuf::from(source);
        let names: Vec<String> = std::fs::read_dir(&dir)
            .map_err(|e| format!("Update-Ordner {} nicht lesbar: {e}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        find_installers(&names.join("\n"))
            .into_iter()
            .max_by_key(|(v, _)| *v)
            .map(|(v, name)| Found { version: v, location: dir.join(name).display().to_string() })
    };
    Ok(best.filter(|f| f.version > current()))
}

/// Prüfung im Hintergrund
pub fn check_async(source: &str, ctx: &eframe::egui::Context) -> mpsc::Receiver<Result<Option<Found>, String>> {
    let (tx, rx) = mpsc::channel();
    let (source, ctx) = (source.to_string(), ctx.clone());
    std::thread::spawn(move || {
        let _ = tx.send(check(&source));
        ctx.request_repaint();
    });
    rx
}

/// MSI holen (URL → %TEMP%, Netzwerkordner → lokale Kopie), damit die Installation nicht vom
/// Netzwerk abhängt
pub fn fetch_async(found: &Found, ctx: &eframe::egui::Context) -> mpsc::Receiver<Result<PathBuf, String>> {
    let (tx, rx) = mpsc::channel();
    let (loc, ctx) = (found.location.clone(), ctx.clone());
    let name = format!("RdpManager-{}-x64.msi", version_text(found.version));
    std::thread::spawn(move || {
        let target = std::env::temp_dir().join(name);
        let r = if is_url(&loc) {
            curl(&["-o", &target.display().to_string(), &loc]).map(|_| target)
        } else {
            std::fs::copy(&loc, &target).map(|_| target).map_err(|e| format!("Kopieren fehlgeschlagen: {e}"))
        };
        let _ = tx.send(r);
        ctx.request_repaint();
    });
    rx
}

/// Installer starten (das Programm sollte sich danach beenden, damit die Dateien ersetzt werden können)
pub fn run_installer(msi: &std::path::Path) -> Result<(), String> {
    Command::new("msiexec.exe")
        .arg("/i")
        .arg(msi)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Installer konnte nicht gestartet werden: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("0.1.12"), Some((0, 1, 12)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert!((0, 1, 13) > (0, 1, 12) && (0, 2, 0) > (0, 1, 99));
        let html = r#"<a href="RdpManager-0.1.9-x64.msi">x</a> <a href="RdpManager-0.1.10-x64.msi">RdpManager-0.1.10-x64.msi</a>
            RdpManager-0.1.10.exe RdpManager-abc-x64.msi"#;
        let f = find_installers(html);
        assert_eq!(f, vec![((0, 1, 9), "RdpManager-0.1.9-x64.msi".into()), ((0, 1, 10), "RdpManager-0.1.10-x64.msi".into())]);
    }

    #[test]
    fn github() {
        assert_eq!(github_repo("https://github.com/Yuji2k/rdp-manager"), Some(("Yuji2k".into(), "rdp-manager".into())));
        assert_eq!(github_repo("github.com/a/b/releases/latest"), Some(("a".into(), "b".into())));
        assert_eq!(github_repo("https://github.com/a/b.git/"), Some(("a".into(), "b".into())));
        assert_eq!(github_repo(r"\\server\share"), None);
        assert_eq!(github_repo("https://github.com/nur-besitzer"), None);
        assert_eq!(effective_source("  "), DEFAULT_SOURCE);
        let json = r#"[
          {"tag_name":"v99.1.0","draft":false,"prerelease":true,"assets":[{"name":"RdpManager-99.1.0-x64.msi","browser_download_url":"https://x/pre.msi"}]},
          {"tag_name":"v0.3.0","draft":false,"prerelease":false,"assets":[
             {"name":"RdpManager-0.3.0.exe","browser_download_url":"https://x/a.exe"},
             {"name":"RdpManager-0.3.0-x64.msi","browser_download_url":"https://x/0.3.0.msi"}]},
          {"tag_name":"v0.2.0","draft":false,"prerelease":false,"assets":[{"name":"RdpManager-0.2.0-x64.msi","browser_download_url":"https://x/0.2.0.msi"}]}
        ]"#;
        assert_eq!(from_github_json(json).unwrap(), Some(Found { version: (0, 3, 0), location: "https://x/0.3.0.msi".into() }));
        assert_eq!(from_github_json("[]").unwrap(), None);
        assert!(from_github_json(r#"{"message":"API rate limit exceeded"}"#).is_err());
    }

    /// Fragt die echte GitHub-Seite ab (nur manuell: cargo test -- --ignored)
    #[test]
    #[ignore]
    fn github_live() {
        let body = curl(&["-H", "Accept: application/vnd.github+json", "https://api.github.com/repos/Yuji2k/rdp-manager/releases?per_page=10"]).unwrap();
        let f = from_github_json(&String::from_utf8_lossy(&body)).unwrap().expect("ein Release mit MSI");
        println!("gefunden: {} {}", version_text(f.version), f.location);
        assert!(f.version >= (0, 2, 0) && f.location.ends_with("-x64.msi"));
    }

    #[test]
    fn folder_check() {
        let dir = std::env::temp_dir().join(format!("rdpm_upd_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("RdpManager-0.0.1-x64.msi"), b"").unwrap();
        assert_eq!(check(&dir.display().to_string()).unwrap(), None);
        std::fs::write(dir.join("RdpManager-99.0.0-x64.msi"), b"").unwrap();
        let f = check(&dir.display().to_string()).unwrap().unwrap();
        assert_eq!(f.version, (99, 0, 0));
        assert!(f.location.ends_with("RdpManager-99.0.0-x64.msi"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
