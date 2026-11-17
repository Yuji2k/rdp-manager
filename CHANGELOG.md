# Versionshinweise

## 0.2.0
### Neu
- **Erste öffentliche Version** – Quellcode auf GitHub unter der MIT-Lizenz.
- **Updates direkt von GitHub**: Ohne eingetragene Quelle sucht RDP Manager beim Start unter den offiziellen Releases (github.com/Yuji2k/rdp-manager) nach einer neueren Version. Auch eigene GitHub-Repositories lassen sich als Quelle eintragen. Abschaltbar unter *Hilfe > Nach Updates suchen*; Netzwerkordner und Webadressen funktionieren weiterhin.
- Enthält alle Funktionen der bisherigen Versionen (siehe unten).

## 0.1.13
### Neu
- **Favoriten und „Zuletzt verwendet“** oben im Baum: Rechtsklick auf eine Verbindung > *Zu Favoriten* (auch für gemeinsame Listen). Die letzten 10 geöffneten Verbindungen erscheinen automatisch; Rechtsklick auf die Überschrift leert die Liste.
- **Tabs wiederherstellen**: Beim Beenden offene Verbindungen werden beim nächsten Start nacheinander wieder geöffnet (nach dem Entsperren). Abschaltbar unter *Ansicht > Offene Tabs beim nächsten Start wieder öffnen*.
- **Wake-on-LAN**: MAC-Adresse an der Verbindung eintragen (oder per *Ermitteln* abfragen, solange der Rechner läuft), dann Rechtsklick > *Aufwecken* oder *Aufwecken und verbinden* – verbindet automatisch, sobald der Rechner antwortet.
- **Update-Prüfung** (*Hilfe > Nach Updates suchen*): Ordner (z. B. Netzlaufwerk) oder Webadresse mit den MSI-Dateien eintragen. Beim Start wird nach einer neueren Version gesucht; unten rechts erscheint dann „⬆ Version … verfügbar“, ein Klick installiert sie.

## 0.1.12
### Neu
- **Übersicht aller Sitzungen** (▦ rechts in der Tableiste, *Ansicht* oder **Strg+Umschalt+A**): alle Tabs als Kacheln mit Vorschaubild, SSH mit den letzten Terminalzeilen. Klick öffnet die Sitzung, Mittelklick trennt, Esc schließt die Übersicht.
- **Auflösung im laufenden Betrieb**: *Sitzung > Auflösung jetzt anpassen* und *Auflösung automatisch anpassen* (an/aus) – ohne neu zu verbinden, auch nach dem Verschieben auf einen anderen Monitor.
- **Alle Monitore**: RDP-Verbindungen können über alle (oder ausgewählte) Monitore laufen – Einstellung *Aussehen > Alle Monitore verwenden*, Rechtsklick > *Auf allen Monitoren* oder *Sitzung > Auf alle Monitore verschieben*. Dafür wird mstsc verwendet; ein gespeichertes Passwort wird übergeben.
- **Sperre mit Hauptkennwort oder Windows Hello** (*Datei > Sperre & Hauptkennwort*): beim Start, nach Leerlauf oder per 🔒 oben rechts. Offene Sitzungen bleiben verbunden, werden aber ausgeblendet. Der Export mit Passwörtern verlangt ebenfalls das Entsperren.
- **Passwort-Tresor** (*Datei > Passwort-Tresor*): Benutzer und Passwort aus **KeePassXC**, **KeePass 2 (KPScript)** oder **Bitwarden/Vaultwarden** holen – je Verbindung über das Feld *Tresor-Eintrag*. Das Masterkennwort wird einmal pro Programmstart abgefragt und nur im Arbeitsspeicher gehalten.
- **Import aus anderen Programmen** (*Datei > Importieren*): Remote Desktop Connection Manager (.rdg, inkl. Gruppen, Anmeldedaten und – für denselben Windows-Benutzer – Passwörtern), PuTTY-Sitzungen, OpenSSH-Konfiguration (~/.ssh/config) und CSV (z. B. aus Royal TS).
- **Verbindungsprotokoll** (*Datei > Verbindungsprotokoll*): wer wann welche Verbindung wie lange genutzt hat, mit Filter und CSV-Export. Abschaltbar im selben Fenster.

## 0.1.11
### Neu
- **Schnellsuche (Strg+P oder „🔍 Suchen“)**: tippen, mit Pfeiltasten wählen, Enter verbindet. Durchsucht eigene und gemeinsame Listen und offene Tabs nach Name, Host, Ordner, Beschreibung, Notizen und Tags (#tag).
- **Tags und Farben**: Verbindungen bekommen Tags (z. B. prod, linux) und eine Farbe. Der Baum lässt sich über die Tag-Schaltflächen oder „#tag“ im Filterfeld filtern, Tab und Baumeintrag werden eingefärbt.
- **Notizen pro Verbindung**: Wartungsfenster, Ansprechpartner usw. – im Tooltip, über Rechtsklick > Notizen und während der Sitzung über *Sitzung > Notizen* als eigenes Fenster.
- **Tabs abdocken**: *Sitzung > In eigenem Fenster öffnen* (z. B. für den zweiten Monitor), zurück über „Andocken“ oder Schließen des Fensters.
- **SSH-Tunnel und Sprunghost**: RDP, VNC und Web können „Über SSH-Sprunghost“ verbunden werden – der Tunnel wird automatisch auf- und abgebaut. SSH-Verbindungen nutzen dafür ssh -J.
- **Portweiterleitungen** (-L, -R, SOCKS -D) je SSH-Verbindung.
- **SSH-Schlüssel und Agent**: *Datei > SSH-Agent & Schlüssel* zeigt den Agent-Dienst und die geladenen Schlüssel, Schlüssel hinzufügen oder erzeugen. Agent-Weiterleitung (-A) je Verbindung. *Sitzung > Öffentlichen Schlüssel eintragen* richtet die Anmeldung per Schlüssel auf dem Server ein.
- **Gleichzeitige Eingabe**: Was du tippst, geht an alle markierten SSH-Tabs (📡). Markieren über das Sitzungsmenü, beenden in der Statusleiste.
- **SFTP-Dateibrowser** als Seitenleiste im SSH-Tab: Ordner durchblättern, herunterladen (Doppelklick), hochladen (auch per Ziehen), umbenennen, löschen, Ordner anlegen, „cd“ im Terminal.

## 0.1.10
### Neu
- **Passwort senden in SSH-Sitzungen** (z. B. für sudo): *Sitzung > Passwort senden*, Rechtsklick auf den Tab oder **Strg+Umschalt+P** im Terminal.
- Fragt der Server nach einem Passwort („[sudo] Passwort für …:“), erscheint unten rechts im Terminal die Schaltfläche „🔑 Passwort senden“. Gesendet wird nur auf Klick – nie automatisch.
- Ist für die Verbindung kein Passwort gespeichert (z. B. Anmeldung per Schlüssel), wird es einmal abgefragt und kann gespeichert werden.

## 0.1.9
### Neu
- Eingebaute Versionshinweise: *Hilfe > Versionshinweise* oder Klick auf die Versionsnummer unten rechts.
- Nach einem Update werden die Neuerungen seit der zuletzt verwendeten Version einmalig angezeigt.

## 0.1.8
### Neu
- Automatische Sicherung der Verbindungen: beim ersten Speichern pro Programmstart, danach höchstens stündlich und immer vor Import (ersetzen), Ordner löschen und Wiederherstellen.
- Dialog *Datei > Sicherungen*: Stand wiederherstellen oder als Ordner zurückholen, „Jetzt sichern“, Einstellungen (Anzahl, zusätzlicher Sicherungsordner z. B. auf einem Netzlaufwerk).
- Aufbewahrt werden die letzten 20 Sicherungen und je eine pro Tag der letzten 14 Tage.
### Verbessert
- Ist die Verbindungsdatei beschädigt oder leer, wird sie beiseitegelegt und automatisch die neueste lesbare Sicherung geladen – statt mit einer leeren Liste zu starten.
- Dateien mit Byte-Order-Mark (z. B. nach Bearbeitung im Editor) werden gelesen.
### Geändert
- Bei eingeschalteter Sicherung bleiben Passwörter gelöschter Verbindungen im Windows-Tresor, damit eine Wiederherstellung sie wiederfindet. Aufräumen über *Gespeicherte Passwörter > verwaiste Einträge*.

## 0.1.7
### Neu
- **Web-Tabs (HTTP/HTTPS)** mit Edge WebView2 – für iDRAC/iLO, Drucker, Switches. Adressleiste, Zurück/Vor/Neu laden, „Im Standardbrowser öffnen“, optional Zertifikatsfehler ignorieren.
- **VNC-Viewer** direkt im Tab (ohne Passwort oder mit VNC-Passwort; Raw, CopyRect, ZRLE), Strg+Alt+Entf, Zwischenablage eintippen.
- **Externe Werkzeuge** im Rechtsklick-Menü (Ping, Traceroute, Port testen, C$, Computerverwaltung, Dienste, Ereignisanzeige, PowerShell-Remoting …) mit Platzhaltern wie %HOST% – frei erweiterbar.
- **Gemeinsame Verbindungslisten**: connections.json oder mRemoteNG-confCons.xml, z. B. vom Netzlaufwerk, schreibgeschützt im Baum; wird bei Änderungen automatisch neu eingelesen.
- *Datei > Eigene Verbindungen als gemeinsame Liste speichern* (ohne Passwörter).
- Quick Connect versteht vnc://, http:// und https://.
- mRemoteNG-Import/-Export für VNC, HTTP und HTTPS.

## 0.1.6
### Neu
- **Gemeinsame Anmeldedaten am Ordner**: Benutzer, Domäne und Passwort einmal am Ordner hinterlegen, Verbindungen übernehmen sie („Vom Ordner übernehmen“).
- **Passwortübersicht** (*Datei > Gespeicherte Passwörter*): ändern, löschen, verwaiste Einträge entfernen.
- **Dateiübertragung per scp**: Dateien auf ein SSH-Terminal ziehen oder über das Sitzungsmenü hoch- und herunterladen, mit Fortschrittsfenster.
- **Befehls-Schnipsel** für SSH, auch an alle SSH-Sitzungen gleichzeitig.
- **Suche im Terminal-Verlauf** (Strg+Umschalt+F) und **Mitschnitt** von SSH-Sitzungen in eine Logdatei.

## 0.1.5
### Neu
- **Erreichbarkeit im Baum**: farbiger Punkt vor jeder Verbindung, geprüft nach und nach und nur für sichtbare Einträge.
- **Alle Verbindungen eines Ordners öffnen** (optional inkl. Unterordner).
- **Geteilte Ansicht**: zwei Sitzungen nebeneinander, Trennlinie verschiebbar.
- Portable exe zum Testen ohne Installation.
### Behoben
- Programm reagierte nach der Erreichbarkeitsprüfung nicht mehr (gegenseitige Blockade zweier Sperren).

## 0.1.0
### Neu
- Eingebettete RDP-Sitzungen in Tabs (Microsoft-RDP-Control wie bei mRemoteNG).
- Verbindungen in auf- und zuklappbaren Ordnern, Verschieben per Drag & Drop oder Rechtsklick.
- Farbmodus Hell, Dunkel oder wie Windows.
- Alle wichtigen Verbindungseinstellungen aus mRemoteNG (Anzeige, Protokoll, Gateway, Aussehen, Umleitung).
- Import und Export im mRemoteNG-Format (confCons.xml, inkl. verschlüsselter Passwörter).
- Quick Connect mit Verlauf (Strg+K) und Tastenkombinationen wie Strg+Alt+Entf.
- SSH-Verbindungen im eingebauten Terminal (Windows-OpenSSH).
- Passwörter verschlüsselt in der Windows-Anmeldeinformationsverwaltung.
- MSI-Installer mit Programmsymbol.
