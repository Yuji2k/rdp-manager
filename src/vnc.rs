//! Eingebauter VNC-Client (RFB-Protokoll 3.3/3.7/3.8), direkt im Tab mit egui gezeichnet.
//!
//! Unterstützt: Sicherheitstyp „keine“ und „VNC-Passwort“, Kodierungen Raw, CopyRect, ZRLE
//! und DesktopSize. Verschlüsselte Verfahren (VeNCrypt, RealVNC-RA2) werden nicht unterstützt.

use crate::model::Connection;
use crate::rdp::State;
use eframe::egui::{self, Color32, Pos2, Rect, Sense};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ENC_RAW: i32 = 0;
const ENC_COPYRECT: i32 = 1;
const ENC_ZRLE: i32 = 16;
const ENC_DESKTOP_SIZE: i32 = -223;

/// Bildspeicher (RGBA)
#[derive(Default)]
struct Framebuffer {
    w: usize,
    h: usize,
    rgba: Vec<u8>,
    dirty: bool,
}

impl Framebuffer {
    fn resize(&mut self, w: usize, h: usize) {
        self.w = w;
        self.h = h;
        self.rgba = vec![0; w * h * 4];
        self.dirty = true;
    }
    fn set(&mut self, x: usize, y: usize, px: [u8; 4]) {
        if x < self.w && y < self.h {
            let i = (y * self.w + x) * 4;
            self.rgba[i..i + 4].copy_from_slice(&px);
        }
    }
    fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, px: [u8; 4]) {
        for yy in y..(y + h).min(self.h) {
            for xx in x..(x + w).min(self.w) {
                let i = (yy * self.w + xx) * 4;
                self.rgba[i..i + 4].copy_from_slice(&px);
            }
        }
    }
    fn copy_rect(&mut self, sx: usize, sy: usize, dx: usize, dy: usize, w: usize, h: usize) {
        let mut tmp = Vec::with_capacity(w * h * 4);
        for yy in 0..h {
            for xx in 0..w {
                let (x, y) = (sx + xx, sy + yy);
                if x < self.w && y < self.h {
                    let i = (y * self.w + x) * 4;
                    tmp.extend_from_slice(&self.rgba[i..i + 4]);
                } else {
                    tmp.extend_from_slice(&[0, 0, 0, 255]);
                }
            }
        }
        for yy in 0..h {
            for xx in 0..w {
                let j = (yy * w + xx) * 4;
                self.set(dx + xx, dy + yy, [tmp[j], tmp[j + 1], tmp[j + 2], tmp[j + 3]]);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum VState {
    Connecting,
    Connected,
    NeedsPassword,
    Failed(String),
    Closed,
}

struct Shared {
    fb: Mutex<Framebuffer>,
    state: Mutex<VState>,
    name: Mutex<String>,
    /// Text aus der Zwischenablage des Servers
    clipboard: Mutex<Option<String>>,
}

pub struct VncSession {
    pub id: u64,
    pub title: String,
    pub host: String,
    pub icon: &'static str,
    pub conn: Connection,
    shared: Arc<Shared>,
    writer: Arc<Mutex<Option<TcpStream>>>,
    state: State,
    texture: Option<egui::TextureHandle>,
    buttons: u8,
    last_pointer: Option<(u16, u16)>,
    modifiers: egui::Modifiers,
    password: Option<String>,
    /// Eingabefeld, wenn der Server ein Passwort verlangt
    pub password_input: String,
    pub remember_password: bool,
    grab_focus: bool,
    started: Instant,
    ctx: egui::Context,
}

impl VncSession {
    /// Letztes Bild des Remote-Bildschirms (für Miniaturansichten)
    pub fn texture(&self) -> Option<&egui::TextureHandle> {
        self.texture.as_ref()
    }

    pub fn start(id: u64, conn: &Connection, password: Option<String>, ctx: &egui::Context) -> Self {
        let mut s = Self {
            id,
            title: conn.display_name().to_string(),
            host: conn.host.trim().to_string(),
            icon: conn.icon.glyph(),
            conn: conn.clone(),
            shared: Arc::new(Shared {
                fb: Mutex::new(Framebuffer::default()),
                state: Mutex::new(VState::Connecting),
                name: Mutex::new(String::new()),
                clipboard: Mutex::new(None),
            }),
            writer: Arc::new(Mutex::new(None)),
            state: State::Connecting,
            texture: None,
            buttons: 0,
            last_pointer: None,
            modifiers: egui::Modifiers::default(),
            password,
            password_input: String::new(),
            remember_password: true,
            grab_focus: true,
            started: Instant::now(),
            ctx: ctx.clone(),
        };
        s.spawn();
        s
    }

    fn spawn(&mut self) {
        self.shared = Arc::new(Shared {
            fb: Mutex::new(Framebuffer::default()),
            state: Mutex::new(VState::Connecting),
            name: Mutex::new(String::new()),
            clipboard: Mutex::new(None),
        });
        self.writer = Arc::new(Mutex::new(None));
        self.state = State::Connecting;
        self.started = Instant::now();
        let (host, port) = (self.conn.host.trim().to_string(), self.conn.port);
        let (shared, writer, ctx, pw) = (self.shared.clone(), self.writer.clone(), self.ctx.clone(), self.password.clone());
        std::thread::spawn(move || {
            let result = run(&host, port, pw.as_deref(), &shared, &writer, &ctx);
            let mut st = shared.state.lock().unwrap();
            if *st != VState::NeedsPassword {
                *st = match result {
                    Ok(()) => VState::Closed,
                    Err(e) => VState::Failed(e),
                };
            }
            ctx.request_repaint();
        });
    }

    /// Mit dem eingegebenen Passwort erneut verbinden. Gibt das Passwort zurück, wenn es gespeichert werden soll.
    pub fn retry_with_password(&mut self) -> Option<String> {
        let pw = std::mem::take(&mut self.password_input);
        self.password = Some(pw.clone());
        self.spawn();
        self.remember_password.then_some(pw)
    }

    pub fn needs_password(&self) -> bool {
        *self.shared.state.lock().unwrap() == VState::NeedsPassword
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn seconds_running(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    pub fn update(&mut self) {
        self.state = match &*self.shared.state.lock().unwrap() {
            VState::Connecting => State::Connecting,
            VState::Connected => State::Connected,
            VState::NeedsPassword => State::Failed("Der VNC-Server verlangt ein Passwort.".into()),
            VState::Failed(e) => State::Failed(e.clone()),
            VState::Closed => State::Closed,
        };
        if let Some(text) = self.shared.clipboard.lock().unwrap().take() {
            self.ctx.copy_text(text);
        }
    }

    pub fn close(&mut self) {
        if let Some(s) = self.writer.lock().unwrap().take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }

    fn send(&self, bytes: &[u8]) {
        let mut w = self.writer.lock().unwrap();
        if let Some(s) = w.as_mut() {
            if s.write_all(bytes).is_err() {
                *w = None;
            }
        }
    }

    fn key(&self, keysym: u32, down: bool) {
        let mut m = vec![4, down as u8, 0, 0];
        m.extend_from_slice(&keysym.to_be_bytes());
        self.send(&m);
    }

    fn tap(&self, keysym: u32) {
        self.key(keysym, true);
        self.key(keysym, false);
    }

    /// Strg+Alt+Entf an den Server senden
    pub fn send_ctrl_alt_del(&self) {
        for k in [XK_CONTROL_L, XK_ALT_L, XK_DELETE] {
            self.key(k, true);
        }
        for k in [XK_DELETE, XK_ALT_L, XK_CONTROL_L] {
            self.key(k, false);
        }
    }

    fn pointer(&mut self, x: u16, y: u16, buttons: u8) {
        let mut m = vec![5, buttons];
        m.extend_from_slice(&x.to_be_bytes());
        m.extend_from_slice(&y.to_be_bytes());
        self.send(&m);
        self.last_pointer = Some((x, y));
    }

    /// Text tippen (z. B. Einfügen aus der lokalen Zwischenablage)
    pub fn type_text(&self, text: &str) {
        for ch in text.chars() {
            match ch {
                '\r' => {}
                '\n' => self.tap(XK_RETURN),
                '\t' => self.tap(XK_TAB),
                c => self.tap(char_keysym(c)),
            }
        }
    }

    /// Zeichnet den Bildschirm skaliert in `rect` und verarbeitet Maus/Tastatur.
    pub fn ui(&mut self, ui: &mut egui::Ui, rect: Rect, grab_focus: bool) -> bool {
        // Textur aktualisieren
        {
            let mut fb = self.shared.fb.lock().unwrap();
            if fb.dirty && fb.w > 0 {
                let img = egui::ColorImage::from_rgba_unmultiplied([fb.w, fb.h], &fb.rgba);
                match &mut self.texture {
                    Some(t) if t.size() == [fb.w, fb.h] => t.set(img, egui::TextureOptions::LINEAR),
                    _ => self.texture = Some(ui.ctx().load_texture(format!("vnc-{}", self.id), img, egui::TextureOptions::LINEAR)),
                }
                fb.dirty = false;
            }
        }
        let Some(tex) = &self.texture else {
            return false;
        };
        let size = tex.size_vec2();
        let scale = (rect.width() / size.x).min(rect.height() / size.y).min(1.0).max(0.05);
        let img_rect = Rect::from_center_size(rect.center(), size * scale);
        ui.painter().image(tex.id(), img_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);

        let id = egui::Id::unique(("vnc", self.id));
        let resp = ui.interact(img_rect, id, Sense::click_and_drag());
        if grab_focus || std::mem::take(&mut self.grab_focus) || resp.clicked() || resp.drag_started() || resp.secondary_clicked() {
            resp.request_focus();
        }
        let focused = resp.has_focus();
        if focused {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    id,
                    egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true },
                )
            });
        }
        if *self.state() != State::Connected {
            return focused;
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }

        // --- Maus
        let events = ui.input(|i| i.events.clone());
        let to_fb = |p: Pos2| -> (u16, u16) {
            let x = ((p.x - img_rect.min.x) / scale).clamp(0.0, size.x - 1.0);
            let y = ((p.y - img_rect.min.y) / scale).clamp(0.0, size.y - 1.0);
            (x as u16, y as u16)
        };
        if resp.hovered() || resp.dragged() {
            for ev in &events {
                match ev {
                    egui::Event::PointerMoved(p) => {
                        let (x, y) = to_fb(*p);
                        if self.last_pointer != Some((x, y)) {
                            self.pointer(x, y, self.buttons);
                        }
                    }
                    egui::Event::PointerButton { pos, button, pressed, .. } => {
                        let bit = match button {
                            egui::PointerButton::Primary => 1,
                            egui::PointerButton::Middle => 2,
                            egui::PointerButton::Secondary => 4,
                            _ => 0,
                        };
                        if *pressed {
                            self.buttons |= bit;
                        } else {
                            self.buttons &= !bit;
                        }
                        let (x, y) = to_fb(*pos);
                        self.pointer(x, y, self.buttons);
                    }
                    egui::Event::MouseWheel { delta, .. } if delta.y != 0.0 => {
                        let (x, y) = self.last_pointer.unwrap_or((0, 0));
                        let bit = if delta.y > 0.0 { 8 } else { 16 };
                        self.pointer(x, y, self.buttons | bit);
                        self.pointer(x, y, self.buttons);
                    }
                    _ => {}
                }
            }
        }

        // --- Tastatur
        if focused {
            for ev in events {
                match ev {
                    egui::Event::Key { key, pressed, modifiers, .. } => {
                        self.sync_modifiers(modifiers);
                        if let Some(sym) = key_keysym(key, modifiers) {
                            self.key(sym, pressed);
                        }
                    }
                    egui::Event::Text(t) => {
                        // AltGr (= Strg+Alt) erzeugt Zeichen wie @ € { } – Modifikatoren dafür kurz lösen
                        let altgr = self.modifiers.ctrl && self.modifiers.alt;
                        if altgr {
                            self.key(XK_CONTROL_L, false);
                            self.key(XK_ALT_L, false);
                        }
                        for c in t.chars() {
                            self.tap(char_keysym(c));
                        }
                        if altgr {
                            self.key(XK_CONTROL_L, true);
                            self.key(XK_ALT_L, true);
                        }
                    }
                    egui::Event::Copy => self.tap('c' as u32),
                    egui::Event::Cut => self.tap('x' as u32),
                    egui::Event::Paste(t) => {
                        // lokale Zwischenablage eintippen (Strg dafür lösen)
                        let ctrl = self.modifiers.ctrl;
                        if ctrl {
                            self.key(XK_CONTROL_L, false);
                        }
                        self.type_text(&t);
                        if ctrl {
                            self.key(XK_CONTROL_L, true);
                        }
                    }
                    _ => {}
                }
            }
        } else if self.modifiers.any() {
            // Fokus verloren → keine Taste hängen lassen
            self.sync_modifiers(egui::Modifiers::default());
        }
        focused
    }

    fn sync_modifiers(&mut self, m: egui::Modifiers) {
        let old = self.modifiers;
        if old.shift != m.shift {
            self.key(XK_SHIFT_L, m.shift);
        }
        if old.ctrl != m.ctrl {
            self.key(XK_CONTROL_L, m.ctrl);
        }
        if old.alt != m.alt {
            self.key(XK_ALT_L, m.alt);
        }
        self.modifiers = m;
    }
}

impl Drop for VncSession {
    fn drop(&mut self) {
        self.close();
    }
}

// ---------------------------------------------------------------------------
// Tasten (X11-Keysyms)
// ---------------------------------------------------------------------------

const XK_RETURN: u32 = 0xff0d;
const XK_TAB: u32 = 0xff09;
const XK_DELETE: u32 = 0xffff;
const XK_SHIFT_L: u32 = 0xffe1;
const XK_CONTROL_L: u32 = 0xffe3;
const XK_ALT_L: u32 = 0xffe9;

fn char_keysym(c: char) -> u32 {
    let cp = c as u32;
    if (0x20..=0x7e).contains(&cp) || (0xa0..=0xff).contains(&cp) { cp } else { 0x0100_0000 | cp }
}

/// Sondertasten → Keysym. Buchstaben/Ziffern nur mit Strg/Alt (sonst kommen sie als Text-Ereignis).
fn key_keysym(key: egui::Key, m: egui::Modifiers) -> Option<u32> {
    use egui::Key::*;
    Some(match key {
        Enter => XK_RETURN,
        Tab => XK_TAB,
        Backspace => 0xff08,
        Escape => 0xff1b,
        Delete => XK_DELETE,
        Insert => 0xff63,
        Home => 0xff50,
        End => 0xff57,
        PageUp => 0xff55,
        PageDown => 0xff56,
        ArrowLeft => 0xff51,
        ArrowUp => 0xff52,
        ArrowRight => 0xff53,
        ArrowDown => 0xff54,
        F1 => 0xffbe,
        F2 => 0xffbf,
        F3 => 0xffc0,
        F4 => 0xffc1,
        F5 => 0xffc2,
        F6 => 0xffc3,
        F7 => 0xffc4,
        F8 => 0xffc5,
        F9 => 0xffc6,
        F10 => 0xffc7,
        F11 => 0xffc8,
        F12 => 0xffc9,
        _ if (m.ctrl || m.alt) && !(m.ctrl && m.alt) => {
            let name = key.name();
            if name.len() == 1 && name.as_bytes()[0].is_ascii_alphanumeric() {
                name.to_ascii_lowercase().as_bytes()[0] as u32
            } else {
                return None;
            }
        }
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Protokoll
// ---------------------------------------------------------------------------

fn read_exact<R: Read>(r: &mut R, n: usize) -> Result<Vec<u8>, String> {
    let mut b = vec![0; n];
    r.read_exact(&mut b).map_err(io_err)?;
    Ok(b)
}
fn u8_<R: Read>(r: &mut R) -> Result<u8, String> {
    Ok(read_exact(r, 1)?[0])
}
fn u16_<R: Read>(r: &mut R) -> Result<u16, String> {
    let b = read_exact(r, 2)?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}
fn u32_<R: Read>(r: &mut R) -> Result<u32, String> {
    let b = read_exact(r, 4)?;
    Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}
fn io_err(e: std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::UnexpectedEof => "Der Server hat die Verbindung beendet.".into(),
        _ => format!("Verbindungsfehler: {e}"),
    }
}
fn reason<R: Read>(r: &mut R) -> String {
    u32_(r)
        .and_then(|n| read_exact(r, n.min(4096) as usize))
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .unwrap_or_default()
}

/// VNC-Passwort-Antwort: DES mit bitweise gespiegeltem Passwort als Schlüssel
pub fn vnc_auth_response(password: &str, challenge: &[u8; 16]) -> [u8; 16] {
    use des::cipher::{BlockCipherEncrypt, KeyInit};
    let mut key = [0u8; 8];
    for (i, b) in password.bytes().take(8).enumerate() {
        key[i] = b.reverse_bits();
    }
    let cipher = des::Des::new_from_slice(&key).expect("DES-Schlüssel");
    let mut out = *challenge;
    for chunk in out.chunks_exact_mut(8) {
        let block: &mut [u8; 8] = chunk.try_into().unwrap();
        cipher.encrypt_block(block.into());
    }
    out
}

/// Handshake, danach Bildaktualisierungen empfangen, bis die Verbindung endet.
fn run(
    host: &str,
    port: u16,
    password: Option<&str>,
    shared: &Shared,
    writer: &Mutex<Option<TcpStream>>,
    ctx: &egui::Context,
) -> Result<(), String> {
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|_| format!("Name „{host}“ nicht auflösbar"))?
        .next()
        .ok_or_else(|| format!("Name „{host}“ nicht auflösbar"))?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(8)).map_err(|e| format!("Keine Verbindung zu {host}:{port}: {e}"))?;
    let _ = s.set_nodelay(true);
    handshake(&mut s, password, shared)?;
    *writer.lock().unwrap() = Some(s.try_clone().map_err(io_err)?);
    *shared.state.lock().unwrap() = VState::Connected;
    ctx.request_repaint();
    receive_loop(s, shared, writer, ctx)
}

fn handshake<S: Read + Write>(s: &mut S, password: Option<&str>, shared: &Shared) -> Result<(), String> {
    let ver = read_exact(s, 12)?;
    let ver = String::from_utf8_lossy(&ver).to_string();
    if !ver.starts_with("RFB ") {
        return Err("Gegenstelle ist kein VNC-Server.".into());
    }
    let minor: u32 = ver.get(8..11).and_then(|m| m.parse().ok()).unwrap_or(3);
    let minor = if minor >= 8 { 8 } else if minor == 7 { 7 } else { 3 };
    s.write_all(format!("RFB 003.00{minor}\n").as_bytes()).map_err(io_err)?;

    // --- Sicherheitstyp
    let chosen = if minor == 3 {
        match u32_(s)? {
            0 => return Err(format!("Server lehnt ab: {}", reason(s))),
            t => t as u8,
        }
    } else {
        let n = u8_(s)?;
        if n == 0 {
            return Err(format!("Server lehnt ab: {}", reason(s)));
        }
        let types = read_exact(s, n as usize)?;
        let t = if types.contains(&2) && password.is_some() {
            2
        } else if types.contains(&1) {
            1
        } else if types.contains(&2) {
            *shared.state.lock().unwrap() = VState::NeedsPassword;
            return Err("Der VNC-Server verlangt ein Passwort.".into());
        } else {
            return Err(format!(
                "Der Server bietet nur Anmeldeverfahren an, die hier nicht unterstützt werden ({}). \
                 Unterstützt: ohne Passwort, VNC-Passwort. Ggf. einen VNC-Viewer über „Werkzeuge“ verwenden.",
                types.iter().map(|t| sec_name(*t)).collect::<Vec<_>>().join(", ")
            ));
        };
        s.write_all(&[t]).map_err(io_err)?;
        t
    };
    match chosen {
        1 => {}
        2 => {
            let Some(pw) = password else {
                *shared.state.lock().unwrap() = VState::NeedsPassword;
                return Err("Der VNC-Server verlangt ein Passwort.".into());
            };
            let ch: [u8; 16] = read_exact(s, 16)?.try_into().unwrap();
            s.write_all(&vnc_auth_response(pw, &ch)).map_err(io_err)?;
        }
        t => return Err(format!("Anmeldeverfahren {} wird nicht unterstützt.", sec_name(t))),
    }
    if chosen == 2 || minor == 8 {
        if u32_(s)? != 0 {
            let why = if minor == 8 { reason(s) } else { String::new() };
            if chosen == 2 {
                *shared.state.lock().unwrap() = VState::NeedsPassword;
            }
            return Err(format!("Anmeldung fehlgeschlagen{}", if why.is_empty() { " – Passwort falsch?".into() } else { format!(": {why}") }));
        }
    }

    // --- Init
    s.write_all(&[1]).map_err(io_err)?; // ClientInit: geteilte Sitzung
    let w = u16_(s)? as usize;
    let h = u16_(s)? as usize;
    let _server_pf = read_exact(s, 16)?;
    let name_len = u32_(s)?.min(4096) as usize;
    let name = String::from_utf8_lossy(&read_exact(s, name_len)?).to_string();
    *shared.name.lock().unwrap() = name;
    shared.fb.lock().unwrap().resize(w, h);

    // 32 Bit, Tiefe 24, Little Endian, Rot<<16 Grün<<8 Blau<<0
    let mut m = vec![0u8, 0, 0, 0];
    m.extend_from_slice(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
    s.write_all(&m).map_err(io_err)?;
    let encs = [ENC_ZRLE, ENC_COPYRECT, ENC_RAW, ENC_DESKTOP_SIZE];
    let mut m = vec![2u8, 0];
    m.extend_from_slice(&(encs.len() as u16).to_be_bytes());
    for e in encs {
        m.extend_from_slice(&e.to_be_bytes());
    }
    s.write_all(&m).map_err(io_err)?;
    request_update(s, false, w, h)?;
    Ok(())
}

fn sec_name(t: u8) -> String {
    match t {
        1 => "ohne Passwort".into(),
        2 => "VNC-Passwort".into(),
        5 | 6 => "RealVNC RA2".into(),
        16 => "Tight".into(),
        18 => "TLS".into(),
        19 => "VeNCrypt".into(),
        30 => "Apple ARD".into(),
        t => format!("Typ {t}"),
    }
}

fn request_update<W: Write>(s: &mut W, incremental: bool, w: usize, h: usize) -> Result<(), String> {
    let mut m = vec![3u8, incremental as u8, 0, 0, 0, 0];
    m.extend_from_slice(&(w as u16).to_be_bytes());
    m.extend_from_slice(&(h as u16).to_be_bytes());
    s.write_all(&m).map_err(io_err)
}

fn receive_loop(
    s: TcpStream,
    shared: &Shared,
    writer: &Mutex<Option<TcpStream>>,
    ctx: &egui::Context,
) -> Result<(), String> {
    let mut r = std::io::BufReader::with_capacity(1 << 16, s);
    let mut zlib = flate2::Decompress::new(true);
    loop {
        if writer.lock().unwrap().is_none() {
            return Ok(()); // Tab geschlossen
        }
        match u8_(&mut r)? {
            0 => {
                read_exact(&mut r, 1)?;
                let n = u16_(&mut r)?;
                for _ in 0..n {
                    read_rect(&mut r, shared, &mut zlib)?;
                }
                shared.fb.lock().unwrap().dirty = true;
                ctx.request_repaint();
                let (w, h) = {
                    let fb = shared.fb.lock().unwrap();
                    (fb.w, fb.h)
                };
                if let Some(wr) = writer.lock().unwrap().as_mut() {
                    request_update(wr, true, w, h)?;
                }
            }
            1 => {
                read_exact(&mut r, 3)?;
                let n = u16_(&mut r)?;
                read_exact(&mut r, n as usize * 6)?;
            }
            2 => {} // Klingel
            3 => {
                read_exact(&mut r, 3)?;
                let n = u32_(&mut r)?;
                let text: String = read_exact(&mut r, n.min(1 << 20) as usize)?.iter().map(|&b| b as char).collect();
                *shared.clipboard.lock().unwrap() = Some(text);
                ctx.request_repaint();
            }
            t => return Err(format!("Unbekannte Nachricht {t} vom Server")),
        }
    }
}

fn read_rect<R: Read>(r: &mut R, shared: &Shared, zlib: &mut flate2::Decompress) -> Result<(), String> {
    let x = u16_(r)? as usize;
    let y = u16_(r)? as usize;
    let w = u16_(r)? as usize;
    let h = u16_(r)? as usize;
    let enc = u32_(r)? as i32;
    match enc {
        ENC_RAW => {
            let data = read_exact(r, w * h * 4)?;
            let mut fb = shared.fb.lock().unwrap();
            for yy in 0..h {
                for xx in 0..w {
                    let i = (yy * w + xx) * 4;
                    fb.set(x + xx, y + yy, [data[i + 2], data[i + 1], data[i], 255]);
                }
            }
        }
        ENC_COPYRECT => {
            let sx = u16_(r)? as usize;
            let sy = u16_(r)? as usize;
            shared.fb.lock().unwrap().copy_rect(sx, sy, x, y, w, h);
        }
        ENC_ZRLE => {
            let len = u32_(r)? as usize;
            let input = read_exact(r, len)?;
            let data = inflate(zlib, &input)?;
            let mut fb = shared.fb.lock().unwrap();
            decode_zrle(&data, x, y, w, h, &mut fb)?;
        }
        ENC_DESKTOP_SIZE => shared.fb.lock().unwrap().resize(w, h),
        e => return Err(format!("Nicht unterstützte Kodierung {e}")),
    }
    Ok(())
}

/// Entpackt einen ZRLE-Block (der zlib-Strom läuft über die ganze Verbindung weiter).
fn inflate(z: &mut flate2::Decompress, input: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(input.len() * 4);
    let mut pos = 0;
    loop {
        out.reserve(64 * 1024);
        let (before_in, before_out) = (z.total_in(), out.len());
        z.decompress_vec(&input[pos..], &mut out, flate2::FlushDecompress::Sync)
            .map_err(|e| format!("ZRLE-Daten fehlerhaft: {e}"))?;
        pos += (z.total_in() - before_in) as usize;
        if pos >= input.len() && out.len() < out.capacity() {
            return Ok(out);
        }
        if z.total_in() == before_in && out.len() == before_out {
            return if pos >= input.len() { Ok(out) } else { Err("ZRLE-Daten fehlerhaft".into()) };
        }
    }
}

/// ZRLE-Kacheln (64×64) dekodieren. CPIXEL = 3 Bytes (B, G, R) bei unserem Pixelformat.
fn decode_zrle(d: &[u8], x: usize, y: usize, w: usize, h: usize, fb: &mut Framebuffer) -> Result<(), String> {
    let mut p = 0usize;
    let err = || "ZRLE-Daten unvollständig".to_string();
    let mut byte = |p: &mut usize| -> Result<u8, String> {
        let b = *d.get(*p).ok_or_else(err)?;
        *p += 1;
        Ok(b)
    };
    let cpixel = |p: &mut usize| -> Result<[u8; 4], String> {
        let s = d.get(*p..*p + 3).ok_or_else(err)?;
        *p += 3;
        Ok([s[2], s[1], s[0], 255])
    };
    let run_len = |p: &mut usize, byte: &mut dyn FnMut(&mut usize) -> Result<u8, String>| -> Result<usize, String> {
        let mut len = 1usize;
        loop {
            let b = byte(p)?;
            len += b as usize;
            if b != 255 {
                return Ok(len);
            }
        }
    };
    for ty in (y..y + h).step_by(64) {
        let th = 64.min(y + h - ty);
        for tx in (x..x + w).step_by(64) {
            let tw = 64.min(x + w - tx);
            let sub = byte(&mut p)?;
            match sub {
                0 => {
                    for yy in 0..th {
                        for xx in 0..tw {
                            let c = cpixel(&mut p)?;
                            fb.set(tx + xx, ty + yy, c);
                        }
                    }
                }
                1 => {
                    let c = cpixel(&mut p)?;
                    fb.fill(tx, ty, tw, th, c);
                }
                2..=16 => {
                    let pal: Vec<[u8; 4]> = (0..sub).map(|_| cpixel(&mut p)).collect::<Result<_, _>>()?;
                    let bits = match sub {
                        2 => 1,
                        3 | 4 => 2,
                        _ => 4,
                    };
                    for yy in 0..th {
                        let (mut cur, mut left) = (0u8, 0);
                        for xx in 0..tw {
                            if left == 0 {
                                cur = byte(&mut p)?;
                                left = 8;
                            }
                            left -= bits;
                            let idx = ((cur >> left) & ((1 << bits) - 1)) as usize;
                            fb.set(tx + xx, ty + yy, *pal.get(idx).ok_or_else(err)?);
                        }
                    }
                }
                128 => {
                    let mut i = 0;
                    while i < tw * th {
                        let c = cpixel(&mut p)?;
                        let n = run_len(&mut p, &mut byte)?;
                        for k in i..(i + n).min(tw * th) {
                            fb.set(tx + k % tw, ty + k / tw, c);
                        }
                        i += n;
                    }
                }
                130..=255 => {
                    let pal: Vec<[u8; 4]> = (0..sub - 128).map(|_| cpixel(&mut p)).collect::<Result<_, _>>()?;
                    let mut i = 0;
                    while i < tw * th {
                        let b = byte(&mut p)?;
                        let c = *pal.get((b & 127) as usize).ok_or_else(err)?;
                        let n = if b & 128 != 0 { run_len(&mut p, &mut byte)? } else { 1 };
                        for k in i..(i + n).min(tw * th) {
                            fb.set(tx + k % tw, ty + k / tw, c);
                        }
                        i += n;
                    }
                }
                s => return Err(format!("Ungültige ZRLE-Unterkodierung {s}")),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// Kleiner Test-Server: Handshake mit VNC-Passwort, dann Raw-, ZRLE- und CopyRect-Rechtecke.
    #[test]
    fn handshake_and_decoding() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            c.write_all(b"RFB 003.008\n").unwrap();
            let mut v = [0u8; 12];
            c.read_exact(&mut v).unwrap();
            assert_eq!(&v, b"RFB 003.008\n");
            c.write_all(&[2, 1, 2]).unwrap(); // None, VNC-Auth
            let mut t = [0u8; 1];
            c.read_exact(&mut t).unwrap();
            assert_eq!(t[0], 2, "mit Passwort wird VNC-Auth gewählt");
            let challenge = [7u8; 16];
            c.write_all(&challenge).unwrap();
            let mut resp = [0u8; 16];
            c.read_exact(&mut resp).unwrap();
            assert_eq!(resp, vnc_auth_response("geheim", &challenge));
            c.write_all(&0u32.to_be_bytes()).unwrap();
            c.read_exact(&mut t).unwrap(); // ClientInit
            // ServerInit 4×2
            let mut m = vec![0, 4, 0, 2];
            m.extend_from_slice(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
            m.extend_from_slice(&4u32.to_be_bytes());
            m.extend_from_slice(b"Test");
            c.write_all(&m).unwrap();
            // SetPixelFormat (20) + SetEncodings (4 + 4*4) + FramebufferUpdateRequest (10)
            let mut buf = vec![0u8; 20 + 20 + 10];
            c.read_exact(&mut buf).unwrap();
            assert_eq!(buf[20], 2);

            // Update: Raw 2×1 bei (0,0) rot/grün; ZRLE solid blau 2×2 bei (2,0); CopyRect (0,0)→(0,1) 2×1
            let mut u = vec![0u8, 0, 0, 3];
            u.extend_from_slice(&[0, 0, 0, 0, 0, 2, 0, 1]);
            u.extend_from_slice(&ENC_RAW.to_be_bytes());
            u.extend_from_slice(&[0, 0, 255, 0, 0, 255, 0, 0]); // BGRX: rot, grün
            u.extend_from_slice(&[0, 2, 0, 0, 0, 2, 0, 2]);
            u.extend_from_slice(&ENC_ZRLE.to_be_bytes());
            let mut z = flate2::Compress::new(flate2::Compression::default(), true);
            let mut zout = Vec::with_capacity(256);
            z.compress_vec(&[1, 255, 0, 0], &mut zout, flate2::FlushCompress::Sync).unwrap(); // solid, BGR = blau
            u.extend_from_slice(&(zout.len() as u32).to_be_bytes());
            u.extend_from_slice(&zout);
            u.extend_from_slice(&[0, 0, 0, 1, 0, 2, 0, 1]);
            u.extend_from_slice(&ENC_COPYRECT.to_be_bytes());
            u.extend_from_slice(&[0, 0, 0, 0]);
            c.write_all(&u).unwrap();
            // nächste (inkrementelle) Anfrage abwarten, dann schließen
            let mut req = [0u8; 10];
            c.read_exact(&mut req).unwrap();
            assert_eq!(req[..2], [3, 1]);
        });

        let shared = Shared {
            fb: Mutex::new(Framebuffer::default()),
            state: Mutex::new(VState::Connecting),
            name: Mutex::new(String::new()),
            clipboard: Mutex::new(None),
        };
        let writer = Mutex::new(None);
        let r = run("127.0.0.1", port, Some("geheim"), &shared, &writer, &egui::Context::default());
        server.join().unwrap();
        assert!(r.is_err(), "Server hat geschlossen");
        assert_eq!(*shared.name.lock().unwrap(), "Test");
        let fb = shared.fb.lock().unwrap();
        let px = |x: usize, y: usize| fb.rgba[(y * fb.w + x) * 4..(y * fb.w + x) * 4 + 3].to_vec();
        assert_eq!(px(0, 0), [255, 0, 0]);
        assert_eq!(px(1, 0), [0, 255, 0]);
        assert_eq!(px(2, 0), [0, 0, 255]);
        assert_eq!(px(3, 1), [0, 0, 255]);
        assert_eq!(px(0, 1), [255, 0, 0], "CopyRect");
        assert_eq!(px(1, 1), [0, 255, 0], "CopyRect");
    }

    #[test]
    fn auth_needs_password() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            c.write_all(b"RFB 003.008\n").unwrap();
            let mut v = [0u8; 12];
            let _ = c.read_exact(&mut v);
            let _ = c.write_all(&[1, 2]); // nur VNC-Auth
            std::thread::sleep(Duration::from_millis(200));
        });
        let shared = Shared {
            fb: Mutex::new(Framebuffer::default()),
            state: Mutex::new(VState::Connecting),
            name: Mutex::new(String::new()),
            clipboard: Mutex::new(None),
        };
        let r = run("127.0.0.1", port, None, &shared, &Mutex::new(None), &egui::Context::default());
        assert!(r.is_err());
        assert_eq!(*shared.state.lock().unwrap(), VState::NeedsPassword);
    }

    #[test]
    fn des_response_properties() {
        let ch: [u8; 16] = core::array::from_fn(|i| i as u8);
        let a = vnc_auth_response("password", &ch);
        let b = vnc_auth_response("password", &ch);
        assert_eq!(a, b);
        assert_ne!(a, ch);
        // Passwort wird auf 8 Zeichen gekürzt
        assert_eq!(vnc_auth_response("password123", &ch), a);
    }
}
