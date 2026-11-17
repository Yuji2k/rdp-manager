//! Externe Werkzeuge: eigene Menüeinträge (Ping, Computerverwaltung, C$ …) mit Platzhaltern
//! für die gewählte Verbindung. Gestartet wird über die Windows-Shell, damit auch .msc-Dateien,
//! UNC-Pfade und URLs funktionieren.

use crate::model::{Connection, ExtTool};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

pub const PLACEHOLDERS: &str =
    "%HOST%  %PORT%  %USER%  %DOMAIN%  %NAME%  %PROTOCOL%  %URL%";

pub fn defaults() -> Vec<ExtTool> {
    let t = |name: &str, program: &str, args: &str| ExtTool { name: name.into(), program: program.into(), args: args.into() };
    vec![
        t("Ping (dauerhaft)", "cmd.exe", "/k ping -t %HOST%"),
        t("Traceroute", "cmd.exe", "/k tracert %HOST%"),
        t("Port testen", "powershell.exe", "-NoExit -Command Test-NetConnection %HOST% -Port %PORT%"),
        t("Freigabe C$ öffnen", "explorer.exe", r"\\%HOST%\c$"),
        t("Computerverwaltung", "compmgmt.msc", r"/computer=\\%HOST%"),
        t("Dienste", "services.msc", "/computer=%HOST%"),
        t("Ereignisanzeige", "eventvwr.msc", "/computer=%HOST%"),
        t("PowerShell-Remoting", "powershell.exe", "-NoExit -Command Enter-PSSession -ComputerName %HOST%"),
        t("Im Browser öffnen (https)", "https://%HOST%", ""),
    ]
}

/// Platzhalter durch die Werte der Verbindung ersetzen
pub fn expand(s: &str, c: &Connection) -> String {
    let url = if c.protocol.is_web() { c.web_url() } else { format!("https://{}", c.host.trim()) };
    s.replace("%HOST%", c.host.trim())
        .replace("%PORT%", &c.port.to_string())
        .replace("%USER%", c.username.trim())
        .replace("%DOMAIN%", c.domain.trim())
        .replace("%NAME%", c.display_name())
        .replace("%PROTOCOL%", c.protocol.label())
        .replace("%URL%", &url)
}

/// Programm, Datei oder URL über die Windows-Shell öffnen
pub fn shell_open(file: &str, params: &str) -> Result<(), String> {
    shell_exec(w!("open"), file, params)
}

/// Programm mit Administratorrechten starten (Windows fragt per Benutzerkontensteuerung nach)
pub fn shell_runas(file: &str, params: &str) -> Result<(), String> {
    shell_exec(w!("runas"), file, params)
}

fn shell_exec(verb: PCWSTR, file: &str, params: &str) -> Result<(), String> {
    let (f, p) = (HSTRING::from(file), HSTRING::from(params));
    let r = unsafe {
        ShellExecuteW(
            None,
            verb,
            PCWSTR(f.as_ptr()),
            if params.is_empty() { PCWSTR::null() } else { PCWSTR(p.as_ptr()) },
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // Rückgabewerte > 32 bedeuten Erfolg
    if r.0 as usize > 32 { Ok(()) } else { Err(format!("„{file}“ konnte nicht gestartet werden (Fehler {}).", r.0 as usize)) }
}

pub fn run(tool: &ExtTool, c: &Connection) -> Result<(), String> {
    let program = expand(tool.program.trim(), c);
    if program.is_empty() {
        return Err(format!("Für „{}“ ist kein Programm eingetragen.", tool.name));
    }
    shell_open(&program, &expand(&tool.args, c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders() {
        let c = Connection { name: "Srv".into(), host: " srv01 ".into(), port: 3389, username: "jdoe".into(), domain: "CORP".into(), ..Default::default() };
        assert_eq!(expand(r"/computer=\\%HOST% %USER%@%DOMAIN% %NAME%:%PORT% %PROTOCOL%", &c), r"/computer=\\srv01 jdoe@CORP Srv:3389 RDP");
        assert_eq!(expand("%URL%", &c), "https://srv01");
    }
}
