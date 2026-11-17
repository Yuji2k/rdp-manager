# RDP Manager

Verbindungsmanager für Windows im Stil von mRemoteNG, geschrieben in Rust. RDP-, SSH-, VNC- und Web-Sitzungen laufen in Tabs eines Fensters.

![Plattform](https://img.shields.io/badge/Plattform-Windows%2010%2F11-blue) ![Lizenz](https://img.shields.io/badge/Lizenz-MIT-green)

## Funktionen

- **RDP** direkt im Tab, über das Microsoft-RDP-Control wie bei mRemoteNG:
  - Gateway, Umleitungen, Smart Sizing, Tastenkombinationen
  - Auflösung passt sich im laufenden Betrieb an
  - mehrere Monitore über mstsc
- **SSH** im eingebauten Terminal (Windows-OpenSSH):
  - SFTP-Dateibrowser und scp-Übertragung
  - Portweiterleitungen, Sprunghost, SSH-Agent
  - Befehls-Schnipsel, gleichzeitige Eingabe in mehrere Sitzungen, Mitschnitt
- **VNC**-Viewer im Tab und **Web-Tabs** über Edge WebView2, z. B. für iDRAC/iLO, Drucker und Switches.
- **Verbindungsbaum**:
  - Ordner, Tags, Farben, Notizen, Favoriten, „Zuletzt verwendet“
  - Erreichbarkeit als farbiger Punkt
  - Schnellsuche mit Strg+P
  - Wake-on-LAN
- **Anmeldedaten**:
  - Passwörter verschlüsselt in der Windows-Anmeldeinformationsverwaltung, Anmeldedaten auch am Ordner
  - optional aus KeePassXC, KeePass 2 (KPScript) oder Bitwarden
- **Sicherheit**:
  - Sperre mit Hauptkennwort oder Windows Hello
  - Verbindungsprotokoll mit CSV-Export
- **Ansichten**: Tabs abdocken, geteilte Ansicht, Übersicht aller Sitzungen mit Vorschaubildern. Offene Tabs werden beim nächsten Start wiederhergestellt.
- **Import und Export**:
  - mRemoteNG (confCons.xml, auch verschlüsselt)
  - Import aus RDCMan (.rdg), PuTTY, `~/.ssh/config` und CSV (z. B. Royal TS)
  - gemeinsame Verbindungslisten (schreibgeschützt), z. B. auf einem Netzlaufwerk
- **Pflege**: automatische Sicherungen, externe Werkzeuge (Ping, Computerverwaltung, C$ …), Update-Prüfung über einen Ordner oder eine URL.

Die Oberfläche ist auf Deutsch.

## Installation

Unter [Releases](../../releases) gibt es zwei Varianten:

- `RdpManager-x.y.z-x64.msi` installiert das Programm mit Startmenü- und Desktop-Verknüpfung.
- `RdpManager-x.y.z.exe` läuft portabel ohne Installation.

Daten (Verbindungen, Sicherungen, Logs) liegen unter `%APPDATA%\RdpManager`. Passwörter liegen in der Windows-Anmeldeinformationsverwaltung, nicht in der Konfigurationsdatei.

Voraussetzungen: Windows 10 oder 11 (x64). Für SSH wird der Windows-OpenSSH-Client gebraucht, für Web-Tabs die Edge-WebView2-Runtime. Beides ist bei aktuellen Windows-Versionen vorhanden.

## Selbst bauen

Benötigt wird:

- [Rust](https://rustup.rs) mit der GNU-Toolchain (`rustup default stable-x86_64-pc-windows-gnu`)
- MinGW-w64 von WinLibs (`winget install BrechtSanders.WinLibs.POSIX.UCRT`), wird für `windres` und `dlltool` gebraucht
- für den MSI-Installer zusätzlich [WiX Toolset 3.14](https://github.com/wixtoolset/wix3/releases)

```powershell
.\build.ps1        # exe nach target\release\
.\build-msi.ps1    # exe + MSI nach dist\
cargo test --release
```

`build.ps1` setzt den PATH für Rust und MinGW automatisch.

## Hinweise

- `assets/WebView2Loader.dll` stammt aus dem Microsoft WebView2 SDK. Microsoft erlaubt, sie mit Anwendungen weiterzugeben.
- RDP nutzt das in Windows enthaltene RDP-ActiveX-Control (`mstscax.dll`).

## Lizenz

[MIT](LICENSE)
