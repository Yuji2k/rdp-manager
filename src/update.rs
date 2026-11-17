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

/// Neueste Version an der Quelle suchen; `None` = keine neuere als die laufende
pub fn check(source: &str) -> Result<Option<Found>, String> {
    let source = source.trim().trim_matches('"');
    if source.is_empty() {
        return Err("Keine Update-Quelle eingetragen.".into());
    }
    let best = if is_url(source) {
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
