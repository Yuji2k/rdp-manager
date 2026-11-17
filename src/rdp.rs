//! Eingebettete RDP-Sitzungen über das Microsoft-RDP-Client-ActiveX-Control (mstscax.dll).
//!
//! Wie mRemoteNG wird nicht `mstsc.exe` gestartet, sondern das RDP-Control direkt in einem
//! Kindfenster unseres Hauptfensters gehostet. Als ActiveX-Container dient das Fenster
//! "AtlAxWin" aus der Windows-eigenen atl.dll. Eigenschaften werden per IDispatch gesetzt,
//! Ereignisse (verbunden / getrennt) kommen über einen kleinen eigenen Event-Sink.

use crate::model::{Connection, GatewayUsage, Resolution};
use std::cell::Cell;
use std::ffi::c_void;
use std::io::Write as _;
use std::path::PathBuf;
use std::ptr::null_mut;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{E_NOINTERFACE, E_NOTIMPL, HWND, S_OK};
use windows::Win32::Graphics::Gdi::{CombineRgn, CreateRectRgn, DeleteObject, RGN_DIFF, SetWindowRgn};
use windows::Win32::Security::Credentials::{
    CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredEnumerateW, CredFree,
    CredReadW, CredWriteW,
};
use windows::Win32::System::Com::{
    DISPATCH_FLAGS, DISPATCH_METHOD, DISPATCH_PROPERTYGET, DISPATCH_PROPERTYPUT, DISPPARAMS, IConnectionPoint,
    IConnectionPointContainer, IDispatch,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Ole::{DISPID_PROPERTYPUT, OleInitialize};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BSTR, GUID, HRESULT, HSTRING, IUnknown, Interface, PCWSTR, s, w};

/// MsRdpClient9NotSafeForScripting (Version 10 des Controls, ab Windows 8.1)
const RDP_CLSID: &str = "{8B918B82-7985-4C24-89DF-C33AD2BBFBCD}";
/// Dispinterface der Control-Ereignisse
const DIID_MSTSCAX_EVENTS: GUID = GUID::from_u128(0x336d5562_efa8_482e_8cb3_c5c0fc7a7db6);
/// IMsTscNonScriptable (zum Setzen des Klartext-Passworts)
const IID_NON_SCRIPTABLE: GUID = GUID::from_u128(0xc1e6743a_41c1_4a74_832a_0dd06c1c7a0e);
/// IMsRdpClientNonScriptable (erbt von IMsTscNonScriptable, enthält SendKeys)
const IID_RDP_NON_SCRIPTABLE: GUID = GUID::from_u128(0x2f079c4c_87b2_4afd_97ab_20cdb43038ae);

/// Flag für erweiterte Tasten (E0-Präfix), wie KBDFLAGS_EXTENDED im RDP-Protokoll
pub const EXT: u16 = 0x100;

/// Tastenkombinationen, die an die Remotesitzung gesendet werden können.
/// (Bezeichnung, Scancodes in Drückreihenfolge)
pub const HOTKEYS: &[(&str, &[u16])] = &[
    ("Strg+Alt+Entf", &[0x1D, 0x38, 0x53]),
    ("Strg+Umschalt+Esc (Task-Manager)", &[0x1D, 0x2A, 0x01]),
    ("Strg+Esc (Startmenü)", &[0x1D, 0x01]),
    ("Alt+Tab", &[0x38, 0x0F]),
    ("Alt+Umschalt+Tab", &[0x38, 0x2A, 0x0F]),
    ("Alt+F4", &[0x38, 0x3E]),
    ("Alt+Leertaste (Fenstermenü)", &[0x38, 0x39]),
    ("Druck", &[0x37 | EXT]),
    ("Alt+Druck", &[0x38, 0x37 | EXT]),
    ("Windows-Taste", &[0x5B | EXT]),
    ("Win+R (Ausführen)", &[0x5B | EXT, 0x13]),
    ("Win+E (Explorer)", &[0x5B | EXT, 0x12]),
    ("Win+D (Desktop)", &[0x5B | EXT, 0x20]),
    ("Win+X (Schnellmenü)", &[0x5B | EXT, 0x2D]),
    ("Win+I (Einstellungen)", &[0x5B | EXT, 0x17]),
    ("Win+L (Sperren)", &[0x5B | EXT, 0x26]),
];
const LCID: u32 = 0x0400; // LOCALE_USER_DEFAULT

// ---------------------------------------------------------------------------
// Logdatei (hilft bei der Fehlersuche, falls eine Einstellung nicht übernommen wird)
// ---------------------------------------------------------------------------

pub fn log(msg: &str) {
    let path = crate::model::Store::data_dir().join("rdp_manager.log");
    let _ = std::fs::create_dir_all(crate::model::Store::data_dir());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{secs}] {msg}");
    }
}

// ---------------------------------------------------------------------------
// Passwörter (Windows-Anmeldeinformationsverwaltung, verschlüsselt per DPAPI)
// ---------------------------------------------------------------------------

fn cred_target(conn_id: u64) -> HSTRING {
    HSTRING::from(format!("RdpManager/{conn_id}"))
}

pub fn store_password(conn_id: u64, user: &str, password: &str) -> Result<(), String> {
    let target = cred_target(conn_id);
    let user = HSTRING::from(user);
    let blob: Vec<u8> = password.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    let cred = CREDENTIALW {
        Flags: CRED_FLAGS(0),
        Type: CRED_TYPE_GENERIC,
        TargetName: windows::core::PWSTR(target.as_ptr() as *mut u16),
        UserName: windows::core::PWSTR(user.as_ptr() as *mut u16),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_ptr() as *mut u8,
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..Default::default()
    };
    unsafe { CredWriteW(&cred, 0) }.map_err(|e| format!("Passwort konnte nicht gespeichert werden: {e}"))
}

pub fn read_password(conn_id: u64) -> Option<String> {
    let target = cred_target(conn_id);
    let mut p: *mut CREDENTIALW = null_mut();
    unsafe {
        CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None, &mut p).ok()?;
        let c = &*p;
        let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
        let wide: Vec<u16> = bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
        CredFree(p as *const c_void);
        Some(String::from_utf16_lossy(&wide))
    }
}

pub fn has_password(conn_id: u64) -> bool {
    read_password(conn_id).is_some()
}

/// IDs aller Passwörter, die dieses Programm im Windows-Tresor abgelegt hat.
pub fn stored_password_ids() -> Vec<u64> {
    let filter = HSTRING::from("RdpManager/*");
    let mut count = 0u32;
    let mut list: *mut *mut CREDENTIALW = null_mut();
    let mut ids = Vec::new();
    unsafe {
        if CredEnumerateW(PCWSTR(filter.as_ptr()), None, &mut count, &mut list).is_err() {
            return ids; // keine Einträge
        }
        for i in 0..count as usize {
            let c = &**list.add(i);
            if let Ok(name) = c.TargetName.to_string()
                && let Some(id) = name.strip_prefix("RdpManager/").and_then(|n| n.parse().ok())
            {
                ids.push(id);
            }
        }
        CredFree(list as *const c_void);
    }
    ids
}

#[cfg(test)]
mod cred_tests {
    /// Schreibt kurz einen Testeintrag in den Windows-Tresor (nur manuell: cargo test -- --ignored)
    #[test]
    #[ignore]
    fn enumerate_roundtrip() {
        const ID: u64 = 999_999_001;
        super::store_password(ID, "test", "geheim").unwrap();
        let found = super::stored_password_ids().contains(&ID);
        super::delete_password(ID);
        assert!(found);
        assert!(!super::stored_password_ids().contains(&ID));
    }
}

pub fn delete_password(conn_id: u64) {
    let target = cred_target(conn_id);
    unsafe {
        let _ = CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None);
    }
}

// ---------------------------------------------------------------------------
// IDispatch-Hilfen
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Disp(IDispatch);

impl Disp {
    fn dispid(&self, name: &str) -> windows::core::Result<i32> {
        let n = HSTRING::from(name);
        let p = PCWSTR(n.as_ptr());
        let mut id = 0;
        unsafe { self.0.GetIDsOfNames(&GUID::zeroed(), &p, 1, LCID, &mut id)? };
        Ok(id)
    }

    fn invoke(&self, name: &str, flags: DISPATCH_FLAGS, args: Vec<VARIANT>) -> windows::core::Result<VARIANT> {
        let id = self.dispid(name)?;
        // Argumente werden bei IDispatch in umgekehrter Reihenfolge übergeben
        let mut args: Vec<VARIANT> = args.into_iter().rev().collect();
        let mut named = [DISPID_PROPERTYPUT];
        let is_put = flags == DISPATCH_PROPERTYPUT;
        let params = DISPPARAMS {
            rgvarg: if args.is_empty() { null_mut() } else { args.as_mut_ptr() },
            rgdispidNamedArgs: if is_put { named.as_mut_ptr() } else { null_mut() },
            cArgs: args.len() as u32,
            cNamedArgs: is_put as u32,
        };
        let mut result = VARIANT::default();
        unsafe {
            self.0.Invoke(
                id,
                &GUID::zeroed(),
                LCID,
                flags,
                &params,
                if is_put { None } else { Some(&mut result) },
                None,
                None,
            )?
        };
        Ok(result)
    }

    fn put(&self, name: &str, value: impl Into<VARIANT>) -> windows::core::Result<()> {
        self.invoke(name, DISPATCH_PROPERTYPUT, vec![value.into()]).map(|_| ())
    }

    fn get(&self, name: &str) -> windows::core::Result<VARIANT> {
        self.invoke(name, DISPATCH_PROPERTYGET, vec![])
    }

    fn call(&self, name: &str, args: Vec<VARIANT>) -> windows::core::Result<VARIANT> {
        self.invoke(name, DISPATCH_METHOD, args)
    }

    fn get_i32(&self, name: &str) -> Option<i32> {
        let v = self.get(name).ok()?;
        i32::try_from(&v).ok().or_else(|| i16::try_from(&v).ok().map(i32::from))
    }

    /// Liefert das erste vorhandene Unterobjekt (z. B. die neueste Version von `AdvancedSettings`).
    fn object(&self, names: &[&str]) -> Option<Disp> {
        names.iter().find_map(|n| {
            let v = self.get(n).ok()?;
            IDispatch::try_from(&v).ok().map(Disp)
        })
    }
}

/// Setzt eine Eigenschaft und protokolliert Fehler, statt abzubrechen.
fn set(errors: &mut Vec<String>, d: Option<&Disp>, name: &str, value: impl Into<VARIANT>) {
    match d {
        Some(d) => {
            if let Err(e) = d.put(name, value) {
                errors.push(format!("{name}: {e}"));
            }
        }
        None => errors.push(format!("{name}: Objekt nicht verfügbar")),
    }
}

// ---------------------------------------------------------------------------
// ATL-Container
// ---------------------------------------------------------------------------

type AtlAxWinInitFn = unsafe extern "system" fn() -> i32;
type AtlAxGetControlFn = unsafe extern "system" fn(HWND, *mut *mut c_void) -> HRESULT;

fn atl_get_control() -> Result<AtlAxGetControlFn, String> {
    static ATL: OnceLock<Result<usize, String>> = OnceLock::new();
    let r = ATL.get_or_init(|| unsafe {
        if let Err(e) = OleInitialize(None) {
            log(&format!("OleInitialize: {e}"));
        }
        let h = LoadLibraryW(w!("atl.dll")).map_err(|e| format!("atl.dll konnte nicht geladen werden: {e}"))?;
        let init = GetProcAddress(h, s!("AtlAxWinInit")).ok_or_else(|| "AtlAxWinInit fehlt".to_string())?;
        let init: AtlAxWinInitFn = std::mem::transmute(init);
        if init() == 0 {
            return Err("AtlAxWinInit fehlgeschlagen".into());
        }
        let get = GetProcAddress(h, s!("AtlAxGetControl")).ok_or_else(|| "AtlAxGetControl fehlt".to_string())?;
        Ok(get as usize)
    });
    r.clone().map(|p| unsafe { std::mem::transmute::<usize, AtlAxGetControlFn>(p) })
}

// ---------------------------------------------------------------------------
// Event-Sink (minimales COM-Objekt, implementiert IDispatch für IMsTscAxEvents)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Events {
    connected: Cell<bool>,
    login_complete: Cell<bool>,
    disconnected: Cell<Option<i32>>,
    fatal_error: Cell<Option<i32>>,
    leave_fullscreen: Cell<bool>,
}

#[repr(C)]
struct Sink {
    vtbl: *const SinkVtbl,
    refs: AtomicU32,
    events: Rc<Events>,
}

#[repr(C)]
struct SinkVtbl {
    query_interface: unsafe extern "system" fn(*mut Sink, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut Sink) -> u32,
    release: unsafe extern "system" fn(*mut Sink) -> u32,
    get_type_info_count: unsafe extern "system" fn(*mut Sink, *mut u32) -> HRESULT,
    get_type_info: unsafe extern "system" fn(*mut Sink, u32, u32, *mut *mut c_void) -> HRESULT,
    get_ids_of_names: unsafe extern "system" fn(*mut Sink, *const GUID, *const PCWSTR, u32, u32, *mut i32) -> HRESULT,
    invoke: unsafe extern "system" fn(
        *mut Sink,
        i32,
        *const GUID,
        u32,
        u16,
        *const DISPPARAMS,
        *mut VARIANT,
        *mut c_void,
        *mut u32,
    ) -> HRESULT,
}

static SINK_VTBL: SinkVtbl = SinkVtbl {
    query_interface: sink_qi,
    add_ref: sink_add_ref,
    release: sink_release,
    get_type_info_count: sink_type_info_count,
    get_type_info: sink_type_info,
    get_ids_of_names: sink_ids_of_names,
    invoke: sink_invoke,
};

unsafe extern "system" fn sink_qi(this: *mut Sink, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    unsafe {
        let iid = *iid;
        if iid == IUnknown::IID || iid == IDispatch::IID || iid == DIID_MSTSCAX_EVENTS {
            *out = this as *mut c_void;
            sink_add_ref(this);
            S_OK
        } else {
            *out = null_mut();
            E_NOINTERFACE
        }
    }
}

unsafe extern "system" fn sink_add_ref(this: *mut Sink) -> u32 {
    unsafe { (*this).refs.fetch_add(1, Ordering::SeqCst) + 1 }
}

unsafe extern "system" fn sink_release(this: *mut Sink) -> u32 {
    unsafe {
        let n = (*this).refs.fetch_sub(1, Ordering::SeqCst) - 1;
        if n == 0 {
            drop(Box::from_raw(this));
        }
        n
    }
}

unsafe extern "system" fn sink_type_info_count(_: *mut Sink, n: *mut u32) -> HRESULT {
    unsafe { *n = 0 };
    S_OK
}

unsafe extern "system" fn sink_type_info(_: *mut Sink, _: u32, _: u32, _: *mut *mut c_void) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn sink_ids_of_names(_: *mut Sink, _: *const GUID, _: *const PCWSTR, _: u32, _: u32, _: *mut i32) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn sink_invoke(
    this: *mut Sink,
    dispid: i32,
    _: *const GUID,
    _: u32,
    _: u16,
    params: *const DISPPARAMS,
    _: *mut VARIANT,
    _: *mut c_void,
    _: *mut u32,
) -> HRESULT {
    unsafe {
        let ev = &(*this).events;
        // Erstes Argument = letztes Element in rgvarg
        let first_arg = || -> Option<i32> {
            let p = params.as_ref()?;
            if p.cArgs == 0 || p.rgvarg.is_null() {
                return None;
            }
            let v = &*p.rgvarg.add(p.cArgs as usize - 1);
            i32::try_from(v).ok().or_else(|| i16::try_from(v).ok().map(i32::from))
        };
        match dispid {
            2 => ev.connected.set(true),                       // OnConnected
            3 => ev.login_complete.set(true),                  // OnLoginComplete
            4 => ev.disconnected.set(Some(first_arg().unwrap_or(0))), // OnDisconnected(discReason)
            6 => ev.leave_fullscreen.set(true),                // OnLeaveFullScreenMode
            10 => ev.fatal_error.set(Some(first_arg().unwrap_or(0))), // OnFatalError(errorCode)
            _ => {}
        }
        S_OK
    }
}

// ---------------------------------------------------------------------------
// Sitzung
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Eq)]
pub enum State {
    Connecting,
    Connected,
    /// Getrennt mit Fehlermeldung – Tab bleibt offen
    Failed(String),
    /// Vom Benutzer beendet (Abmelden / Trennen) – Tab wird geschlossen
    Closed,
}

pub struct Session {
    pub id: u64,
    pub title: String,
    pub host: String,
    pub icon: &'static str,
    /// Einstellungen, mit denen verbunden wurde (für "Neu verbinden", auch bei Quick Connect)
    pub conn: Connection,
    hwnd: HWND,
    control: IUnknown,
    ctl: Disp,
    events: Rc<Events>,
    advise: Option<(IConnectionPoint, u32)>,
    state: State,
    started: Instant,
    last_rect: Option<(i32, i32, i32, i32)>,
    visible: bool,
    /// "An Panel anpassen" + automatische Größenanpassung
    dynamic_resize: bool,
    scale: u32,
    applied: (i32, i32, u32),
    resize_due: Option<Instant>,
    cutouts: Vec<(i32, i32, i32, i32)>,
}

/// Desktop-Skalierung, die RDP akzeptiert, passend zur Windows-Skalierung.
pub fn scale_percent(pixels_per_point: f32) -> u32 {
    const STEPS: [u32; 7] = [100, 125, 150, 175, 200, 250, 300];
    let want = (pixels_per_point * 100.0).round() as u32;
    *STEPS.iter().min_by_key(|s| s.abs_diff(want)).unwrap()
}

impl Session {
    /// Erstellt das RDP-Control als Kindfenster von `parent` an Position `rect` (physische Pixel)
    /// und startet die Verbindung.
    pub fn start(
        id: u64,
        conn: &Connection,
        password: Option<&str>,
        parent: HWND,
        rect: (i32, i32, i32, i32),
        scale: u32,
    ) -> Result<Self, String> {
        let get_control = atl_get_control()?;
        let (x, y, w, h) = rect;
        let clsid = HSTRING::from(RDP_CLSID);
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("AtlAxWin"),
                PCWSTR(clsid.as_ptr()),
                WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                x,
                y,
                w.max(1),
                h.max(1),
                Some(parent),
                None,
                None,
                None,
            )
        }
        .map_err(|e| format!("RDP-Fenster konnte nicht erstellt werden: {e}"))?;

        let destroy = |msg: String| {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            msg
        };

        let unknown = unsafe {
            let mut p = null_mut();
            let hr = get_control(hwnd, &mut p);
            if hr.is_err() || p.is_null() {
                return Err(destroy(format!(
                    "RDP-Control (mstscax.dll) konnte nicht geladen werden: {hr:?}"
                )));
            }
            IUnknown::from_raw(p)
        };
        let ctl = Disp(unknown.cast::<IDispatch>().map_err(|e| destroy(format!("IDispatch: {e}")))?);

        let mut errors = Vec::new();
        let dynamic_resize = conn.resolution == Resolution::FitToPanel && conn.auto_resize;
        let (dw, dh) = match conn.resolution {
            Resolution::Fixed => (conn.fixed_width as i32, conn.fixed_height as i32),
            Resolution::Fullscreen => unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) },
            _ => (w & !1, h),
        };
        let (dw, dh) = (dw.clamp(200, 8192), dh.clamp(200, 8192));

        // --- Basis
        let c = Some(&ctl);
        set(&mut errors, c, "Server", BSTR::from(conn.host.trim()));
        if !conn.username.trim().is_empty() {
            set(&mut errors, c, "UserName", BSTR::from(conn.username.trim()));
        }
        if !conn.domain.trim().is_empty() {
            set(&mut errors, c, "Domain", BSTR::from(conn.domain.trim()));
        }
        set(&mut errors, c, "DesktopWidth", dw);
        set(&mut errors, c, "DesktopHeight", dh);
        set(&mut errors, c, "ColorDepth", conn.colors as i32);
        set(&mut errors, c, "ConnectingText", BSTR::from(format!("Verbinde mit {} …", conn.host.trim())));

        // --- Erweiterte Einstellungen
        let adv = ctl.object(&[
            "AdvancedSettings9",
            "AdvancedSettings8",
            "AdvancedSettings7",
            "AdvancedSettings6",
            "AdvancedSettings5",
            "AdvancedSettings2",
        ]);
        let a = adv.as_ref();
        set(&mut errors, a, "RDPPort", conn.port as i32);
        set(&mut errors, a, "EnableCredSspSupport", conn.use_credssp);
        set(&mut errors, a, "AuthenticationLevel", conn.auth_level.value());
        set(&mut errors, a, "ConnectToAdministerServer", conn.admin_session);
        set(&mut errors, a, "MinutesToIdleTimeout", conn.idle_minutes as i32);
        if !conn.load_balance_info.trim().is_empty() {
            set(&mut errors, a, "LoadBalanceInfo", BSTR::from(conn.load_balance_info.trim()));
        }
        set(&mut errors, a, "SmartSizing", conn.resolution == Resolution::SmartSize);
        set(&mut errors, a, "BitmapPeristence", conn.bitmap_cache as i32); // sic! (Schreibfehler in der API)
        set(&mut errors, a, "PerformanceFlags", conn.performance_flags());
        set(&mut errors, a, "RedirectDrives", conn.redirect_drives);
        set(&mut errors, a, "RedirectPrinters", conn.redirect_printers);
        set(&mut errors, a, "RedirectPorts", conn.redirect_ports);
        set(&mut errors, a, "RedirectSmartCards", conn.redirect_smartcards);
        set(&mut errors, a, "RedirectClipboard", conn.redirect_clipboard);
        set(&mut errors, a, "EnableAutoReconnect", true);
        set(&mut errors, a, "ContainerHandledFullScreen", 0i32);
        set(&mut errors, a, "DisplayConnectionBar", true);
        set(&mut errors, a, "GrabFocusOnConnect", true);

        // --- Sound / Tastenkombinationen
        let sec = ctl.object(&["SecuredSettings3", "SecuredSettings2"]);
        set(&mut errors, sec.as_ref(), "AudioRedirectionMode", conn.sound.value());
        set(&mut errors, sec.as_ref(), "KeyboardHookMode", if conn.redirect_keys { 1i32 } else { 0i32 });

        // --- Gateway
        if conn.gateway_usage != GatewayUsage::Never && !conn.gateway.trim().is_empty() {
            let tr = ctl.object(&["TransportSettings4", "TransportSettings3", "TransportSettings2", "TransportSettings"]);
            let t = tr.as_ref();
            let method: u32 = if conn.gateway_usage == GatewayUsage::Always { 1 } else { 2 };
            set(&mut errors, t, "GatewayHostname", BSTR::from(conn.gateway.trim()));
            set(&mut errors, t, "GatewayUsageMethod", method);
            set(&mut errors, t, "GatewayProfileUsageMethod", 1u32);
            set(&mut errors, t, "GatewayCredsSource", 4u32);
            set(&mut errors, t, "GatewayCredSharing", 1u32);
        }

        if conn.resolution == Resolution::Fullscreen {
            set(&mut errors, c, "FullScreen", true);
        }

        // --- Passwort
        if let Some(pw) = password.filter(|p| !p.is_empty()) {
            if let Err(e) = unsafe { set_clear_text_password(&unknown, pw) } {
                errors.push(e);
            }
        }

        for e in &errors {
            log(&format!("{}: {e}", conn.host));
        }

        // --- Ereignisse abonnieren
        let events = Rc::new(Events::default());
        let advise = unsafe { advise_events(&unknown, events.clone()) };
        if let Err(e) = &advise {
            log(&format!("{}: Ereignisse nicht verfügbar: {e}", conn.host));
        }

        if let Err(e) = ctl.call("Connect", vec![]) {
            return Err(destroy(format!("Verbindung konnte nicht gestartet werden: {e}")));
        }

        Ok(Self {
            id,
            title: conn.display_name().to_string(),
            host: conn.host.trim().to_string(),
            icon: conn.icon.glyph(),
            conn: conn.clone(),
            hwnd,
            control: unknown,
            ctl,
            events,
            advise: advise.ok(),
            state: State::Connecting,
            started: Instant::now(),
            last_rect: Some(rect),
            visible: false,
            dynamic_resize,
            scale,
            applied: (dw, dh, 100),
            resize_due: None,
            cutouts: Vec::new(),
        })
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn seconds_running(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// Zustand aus Ereignissen bzw. per Abfrage aktualisieren. Einmal pro Frame aufrufen.
    pub fn update(&mut self) {
        if matches!(self.state, State::Failed(_) | State::Closed) {
            return;
        }
        if let Some(code) = self.events.fatal_error.take() {
            self.state = State::Failed(format!("Schwerer Fehler im RDP-Control (Code {code})"));
            return;
        }
        if let Some(reason) = self.events.disconnected.take() {
            self.state = self.disconnect_state(reason);
            return;
        }
        if self.events.connected.get() && self.state == State::Connecting {
            self.state = State::Connected;
        }
        // Fallback, falls keine Ereignisse ankommen
        if self.advise.is_none() {
            match self.ctl.get_i32("Connected") {
                Some(1) => self.state = State::Connected,
                Some(0) if self.started.elapsed() > Duration::from_secs(2) => {
                    self.state = State::Failed("Verbindung getrennt.".into())
                }
                _ => {}
            }
        }
        if self.events.login_complete.get() && self.dynamic_resize && self.resize_due.is_none() {
            let (_, _, w, h) = self.last_rect.unwrap_or_default();
            if (w & !1, h, self.scale) != self.applied {
                self.resize_due = Some(Instant::now());
            }
        }
        self.apply_resize();
    }

    fn disconnect_state(&self, reason: i32) -> State {
        let ext = self.ctl.get_i32("ExtendedDisconnectReason").unwrap_or(0);
        // 1 = lokal getrennt, 2 = remote vom Benutzer getrennt/abgemeldet;
        // ext 1/2/11/12 = per API bzw. vom Benutzer getrennt/abgemeldet
        if matches!(reason, 1 | 2) || matches!(ext, 1 | 2 | 11 | 12) {
            return State::Closed;
        }
        let desc = self
            .ctl
            .call("GetErrorDescription", vec![VARIANT::from(reason as u32), VARIANT::from(ext)])
            .ok()
            .and_then(|v| BSTR::try_from(&v).ok())
            .map(|b| b.to_string())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Die Verbindung wurde getrennt.".into());
        State::Failed(format!("{} (Code {reason}/{ext})", desc.trim()))
    }

    fn apply_resize(&mut self) {
        let Some(due) = self.resize_due else { return };
        if !self.events.login_complete.get() || due.elapsed() < Duration::from_millis(400) {
            return;
        }
        let Some((_, _, w, h)) = self.last_rect else { return };
        let (w, h) = ((w & !1).clamp(200, 8192), h.clamp(200, 8192));
        let args = vec![
            VARIANT::from(w as u32),
            VARIANT::from(h as u32),
            VARIANT::from(w as u32),
            VARIANT::from(h as u32),
            VARIANT::from(0u32),
            VARIANT::from(self.scale),
            VARIANT::from(100u32),
        ];
        match self.ctl.call("UpdateSessionDisplaySettings", args) {
            Ok(_) => {
                self.applied = (w, h, self.scale);
                self.resize_due = None;
            }
            // Kurz nach der Anmeldung lehnt der Server das evtl. noch ab -> später erneut versuchen
            Err(_) => self.resize_due = Some(Instant::now() + Duration::from_millis(600)),
        }
    }

    /// Positioniert und zeigt das Control (physische Pixel relativ zum Client-Bereich).
    pub fn place(&mut self, x: i32, y: i32, w: i32, h: i32, scale: u32) {
        let rect = (x, y, w.max(1), h.max(1));
        unsafe {
            if self.last_rect != Some(rect) || !self.visible {
                let _ = MoveWindow(self.hwnd, rect.0, rect.1, rect.2, rect.3, true);
                if self.dynamic_resize && self.last_rect.map(|r| (r.2, r.3)) != Some((rect.2, rect.3)) {
                    self.resize_due = Some(Instant::now());
                }
                self.last_rect = Some(rect);
            }
            if self.scale != scale {
                self.scale = scale;
                if self.dynamic_resize {
                    self.resize_due = Some(Instant::now());
                }
            }
            if !self.visible {
                let _ = ShowWindow(self.hwnd, SW_SHOW);
                self.visible = true;
            }
        }
    }

    /// Schneidet Flächen (Menüs, Tooltips …) aus dem RDP-Fenster aus, damit sie sichtbar bleiben.
    /// Rechtecke in Pixeln relativ zur linken oberen Ecke des RDP-Fensters.
    pub fn set_cutouts(&mut self, cutouts: &[(i32, i32, i32, i32)]) {
        if self.cutouts == cutouts {
            return;
        }
        self.cutouts = cutouts.to_vec();
        let (_, _, w, h) = self.last_rect.unwrap_or((0, 0, 1, 1));
        apply_cutouts(self.hwnd, w, h, cutouts);
    }

    pub fn hide(&mut self) {
        if self.visible {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.visible = false;
        }
    }

    /// Hängt das Control in ein anderes Fenster (Tab abdocken / wieder andocken).
    pub fn set_parent(&mut self, parent: HWND) {
        set_parent(self.hwnd, parent);
        self.last_rect = None;
        self.visible = false;
        self.cutouts.clear();
    }

    pub fn focus(&self) {
        unsafe {
            let target = GetWindow(self.hwnd, GW_CHILD).unwrap_or(self.hwnd);
            let _ = SetFocus(Some(target));
        }
    }

    /// Sendet eine Tastenkombination an die Remotesitzung: alle Tasten drücken, dann rückwärts loslassen.
    pub fn send_combo(&self, scancodes: &[u16]) -> Result<(), String> {
        if *self.state() != State::Connected {
            return Err("Sitzung ist nicht verbunden.".into());
        }
        let mut up: Vec<i16> = Vec::new(); // VARIANT_BOOL: -1 = true
        let mut data: Vec<i32> = Vec::new();
        for &k in scancodes {
            up.push(0);
            data.push(k as i32);
        }
        for &k in scancodes.iter().rev() {
            up.push(-1);
            data.push(k as i32);
        }
        self.focus();
        unsafe {
            let mut p = null_mut();
            self.control
                .query(&IID_RDP_NON_SCRIPTABLE, &mut p)
                .ok()
                .map_err(|e| format!("IMsRdpClientNonScriptable: {e}"))?;
            let obj = IUnknown::from_raw(p);
            // vtable: IUnknown(3) + IMsTscNonScriptable(10) + NotifyRedirectDeviceChange, SendKeys
            let vtbl = *(p as *const *const usize);
            let send: unsafe extern "system" fn(*mut c_void, i32, *const i16, *const i32) -> HRESULT =
                std::mem::transmute(*vtbl.add(14));
            let hr = send(p, data.len() as i32, up.as_ptr(), data.as_ptr());
            drop(obj);
            hr.ok().map_err(|e| format!("Tasten senden fehlgeschlagen: {e}"))
        }
    }

    /// Fensterhandle des Controls (für Miniaturansichten)
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Auflösung der Remotesitzung sofort an die Tab-Größe anpassen (ohne neu zu verbinden)
    pub fn fit_now(&mut self) -> Result<(), String> {
        if !self.events.login_complete.get() {
            return Err("Die Anmeldung ist noch nicht abgeschlossen.".into());
        }
        self.resize_due = Some(Instant::now());
        Ok(())
    }

    /// Passt sich die Auflösung automatisch an die Tab-Größe an?
    pub fn auto_resize(&self) -> bool {
        self.dynamic_resize
    }

    pub fn set_auto_resize(&mut self, on: bool) {
        self.dynamic_resize = on;
        if on {
            self.resize_due = Some(Instant::now());
        }
    }

    pub fn set_fullscreen(&self) -> Result<(), String> {
        self.ctl.put("FullScreen", true).map_err(|e| format!("Vollbild: {e}"))
    }

    /// Smart Sizing (Skalieren statt Auflösung ändern) umschalten. Gibt den neuen Zustand zurück.
    pub fn toggle_smart_sizing(&mut self) -> Result<bool, String> {
        let adv = self
            .ctl
            .object(&["AdvancedSettings9", "AdvancedSettings8", "AdvancedSettings7", "AdvancedSettings2"])
            .ok_or("AdvancedSettings nicht verfügbar")?;
        let current = adv.get("SmartSizing").ok().and_then(|v| bool::try_from(&v).ok()).unwrap_or(false);
        adv.put("SmartSizing", !current).map_err(|e| format!("Smart Sizing: {e}"))?;
        // Bei aktivem Smart Sizing keine automatische Auflösungsänderung
        self.dynamic_resize = current && self.conn.resolution == Resolution::FitToPanel && self.conn.auto_resize;
        if self.dynamic_resize {
            self.resize_due = Some(Instant::now());
        }
        Ok(!current)
    }

    /// Trennt die Verbindung und zerstört das Control.
    pub fn close(&mut self) {
        if self.hwnd.is_invalid() {
            return;
        }
        if matches!(self.state, State::Connecting | State::Connected) {
            let _ = self.ctl.call("Disconnect", vec![]);
        }
        if let Some((cp, cookie)) = self.advise.take() {
            unsafe {
                let _ = cp.Unadvise(cookie);
            }
        }
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
        self.hwnd = HWND::default();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

unsafe fn set_clear_text_password(control: &IUnknown, password: &str) -> Result<(), String> {
    unsafe {
        let mut p = null_mut();
        control
            .query(&IID_NON_SCRIPTABLE, &mut p)
            .ok()
            .map_err(|e| format!("IMsTscNonScriptable: {e}"))?;
        let obj = IUnknown::from_raw(p); // gibt die Referenz beim Drop frei
        // vtable: [QueryInterface, AddRef, Release, put_ClearTextPassword, ...]
        let vtbl = *(p as *const *const usize);
        let put: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT = std::mem::transmute(*vtbl.add(3));
        let b = BSTR::from(password);
        let hr = put(p, b.as_ptr());
        drop(obj);
        hr.ok().map_err(|e| format!("Passwort setzen: {e}"))
    }
}

unsafe fn advise_events(control: &IUnknown, events: Rc<Events>) -> windows::core::Result<(IConnectionPoint, u32)> {
    unsafe {
        let cpc: IConnectionPointContainer = control.cast()?;
        let cp = cpc.FindConnectionPoint(&DIID_MSTSCAX_EVENTS)?;
        let sink = Box::into_raw(Box::new(Sink { vtbl: &SINK_VTBL, refs: AtomicU32::new(1), events }));
        let sink = IUnknown::from_raw(sink as *mut c_void); // übernimmt die erste Referenz
        let cookie = cp.Advise(&sink)?;
        Ok((cp, cookie))
    }
}

/// Unser Hauptfenster soll nicht über die eingebetteten Fenster malen.
/// Schneidet Rechtecke (relativ zum Fenster) aus einem eingebetteten Kindfenster aus.
pub fn apply_cutouts(hwnd: HWND, w: i32, h: i32, cutouts: &[(i32, i32, i32, i32)]) {
    unsafe {
        if cutouts.is_empty() {
            SetWindowRgn(hwnd, None, true);
            return;
        }
        let region = CreateRectRgn(0, 0, w, h);
        for &(x, y, cw, ch) in cutouts {
            let cut = CreateRectRgn(x, y, x + cw, y + ch);
            CombineRgn(Some(region), Some(region), Some(cut), RGN_DIFF);
            let _ = DeleteObject(cut.into());
        }
        // Das System übernimmt die Region (nicht selbst freigeben)
        SetWindowRgn(hwnd, Some(region), true);
    }
}

/// Kindfenster in ein anderes Fenster umhängen (bleibt unsichtbar bis zum nächsten `place`).
pub fn set_parent(child: HWND, parent: HWND) {
    if child.is_invalid() || parent.is_invalid() {
        return;
    }
    unsafe {
        let _ = ShowWindow(child, SW_HIDE);
        let _ = SetWindowRgn(child, None, false);
        let _ = SetParent(child, Some(parent));
    }
}

/// Fenster dieses Prozesses mit genau diesem Titel (z. B. ein abgedocktes egui-Fenster)
pub fn find_own_window(title: &str) -> Option<HWND> {
    let t = HSTRING::from(title);
    let hwnd = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(t.as_ptr())) }.ok()?;
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (pid == std::process::id()).then_some(hwnd)
}

pub fn prepare_parent(parent: HWND) {
    unsafe {
        let style = GetWindowLongPtrW(parent, GWL_STYLE);
        SetWindowLongPtrW(parent, GWL_STYLE, style | WS_CLIPCHILDREN.0 as isize);
    }
}

pub fn focus_window(hwnd: HWND) {
    unsafe {
        let _ = SetFocus(Some(hwnd));
    }
}

// ---------------------------------------------------------------------------
// Externe Verbindung (mstsc.exe in eigenem Fenster)
// ---------------------------------------------------------------------------

pub fn rdp_file_text(conn: &Connection) -> String {
    let address = if conn.port == 3389 { conn.host.trim().to_string() } else { format!("{}:{}", conn.host.trim(), conn.port) };
    let (sm, w, h) = match conn.resolution {
        _ if conn.multimon => (2, 1920, 1080),
        Resolution::Fullscreen => (2, 1920, 1080),
        Resolution::Fixed => (1, conn.fixed_width, conn.fixed_height),
        _ => (1, 1600, 900),
    };
    let mut lines = vec![
        format!("full address:s:{address}"),
        format!("screen mode id:i:{sm}"),
        format!("desktopwidth:i:{w}"),
        format!("desktopheight:i:{h}"),
        format!("session bpp:i:{}", conn.colors),
        format!("smart sizing:i:{}", (conn.resolution == Resolution::SmartSize) as i32),
        format!("dynamic resolution:i:{}", (conn.resolution == Resolution::FitToPanel && conn.auto_resize) as i32),
        format!("authentication level:i:{}", conn.auth_level.value()),
        format!("enablecredsspsupport:i:{}", conn.use_credssp as i32),
        format!("administrative session:i:{}", conn.admin_session as i32),
        format!("redirectclipboard:i:{}", conn.redirect_clipboard as i32),
        format!("redirectprinters:i:{}", conn.redirect_printers as i32),
        format!("redirectcomports:i:{}", conn.redirect_ports as i32),
        format!("redirectsmartcards:i:{}", conn.redirect_smartcards as i32),
        format!("audiomode:i:{}", conn.sound.value()),
        format!("keyboardhook:i:{}", if conn.redirect_keys { 1 } else { 0 }),
        format!("bitmapcachepersistenable:i:{}", conn.bitmap_cache as i32),
        format!("disable wallpaper:i:{}", (!conn.wallpaper) as i32),
        format!("disable themes:i:{}", (!conn.themes) as i32),
        format!("allow font smoothing:i:{}", conn.font_smoothing as i32),
        format!("allow desktop composition:i:{}", conn.desktop_composition as i32),
    ];
    if conn.redirect_drives {
        lines.push("drivestoredirect:s:*".into());
    }
    if conn.multimon {
        lines.push("use multimon:i:1".into());
        let mons: String = conn.monitors.chars().filter(|c| c.is_ascii_digit() || *c == ',').collect();
        let mons = mons.trim_matches(',');
        if !mons.is_empty() {
            lines.push(format!("selectedmonitors:s:{mons}"));
        }
    }
    let user = conn.full_username();
    if !user.is_empty() {
        lines.push(format!("username:s:{user}"));
    }
    if !conn.load_balance_info.trim().is_empty() {
        lines.push(format!("loadbalanceinfo:s:{}", conn.load_balance_info.trim()));
    }
    if conn.gateway_usage != GatewayUsage::Never && !conn.gateway.trim().is_empty() {
        lines.push(format!("gatewayhostname:s:{}", conn.gateway.trim()));
        lines.push(format!("gatewayusagemethod:i:{}", if conn.gateway_usage == GatewayUsage::Always { 1 } else { 2 }));
        lines.push("gatewayprofileusagemethod:i:1".into());
        lines.push("promptcredentialonce:i:1".into());
    }
    lines.join("\r\n") + "\r\n"
}

fn write_rdp_file(conn: &Connection) -> std::io::Result<PathBuf> {
    let dir = crate::model::Store::data_dir().join("rdp");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.rdp", conn.id));
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(rdp_file_text(conn).encode_utf16().flat_map(|c| c.to_le_bytes()));
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// Anmeldedaten für mstsc hinterlegen (TERMSRV/host, nur bis zur Abmeldung von Windows gültig)
fn store_termsrv_credential(host: &str, user: &str, password: &str) -> Result<(), String> {
    use windows::Win32::Security::Credentials::{CRED_PERSIST_SESSION, CRED_TYPE_DOMAIN_PASSWORD};
    let target = HSTRING::from(format!("TERMSRV/{host}"));
    let user = HSTRING::from(user);
    let blob: Vec<u8> = password.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    let cred = CREDENTIALW {
        Flags: CRED_FLAGS(0),
        Type: CRED_TYPE_DOMAIN_PASSWORD,
        TargetName: windows::core::PWSTR(target.as_ptr() as *mut u16),
        UserName: windows::core::PWSTR(user.as_ptr() as *mut u16),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_ptr() as *mut u8,
        Persist: CRED_PERSIST_SESSION,
        ..Default::default()
    };
    unsafe { CredWriteW(&cred, 0) }.map_err(|e| format!("Anmeldedaten für mstsc: {e}"))
}

/// Verbindung in mstsc.exe öffnen (eigenes Fenster, bei `multimon` über alle Monitore).
/// Ein bekanntes Passwort wird als Windows-Anmeldeinformation für diesen Host hinterlegt.
pub fn launch_external(conn: &Connection, password: Option<&str>) -> Result<std::process::Child, String> {
    let file = write_rdp_file(conn).map_err(|e| e.to_string())?;
    let user = conn.full_username();
    if let Some(pw) = password.filter(|p| !p.is_empty())
        && !user.is_empty()
        && let Err(e) = store_termsrv_credential(conn.host.trim(), &user, pw)
    {
        log(&e);
    }
    std::process::Command::new("mstsc.exe").arg(&file).spawn().map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Miniaturansicht: Fensterinhalt abgreifen
// ---------------------------------------------------------------------------

/// Inhalt eines (sichtbaren) Fensters als RGBA-Bild, auf höchstens `max_w` Pixel Breite verkleinert.
pub fn capture(hwnd: HWND, max_w: usize) -> Option<(usize, usize, Vec<u8>)> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
    if hwnd.is_invalid() {
        return None;
    }
    unsafe {
        let mut r = RECT::default();
        GetClientRect(hwnd, &mut r).ok()?;
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        if w < 16 || h < 16 {
            return None;
        }
        let wdc = GetDC(Some(hwnd));
        let mdc = CreateCompatibleDC(Some(wdc));
        let bmp = CreateCompatibleBitmap(wdc, w, h);
        let old = SelectObject(mdc, bmp.into());
        // 2 = PW_RENDERFULLCONTENT (auch DirectX-/DWM-Inhalte)
        let ok = PrintWindow(hwnd, mdc, PRINT_WINDOW_FLAGS(2)).as_bool();
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // von oben nach unten
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        SelectObject(mdc, old);
        let lines = GetDIBits(mdc, bmp, 0, h as u32, Some(bgra.as_mut_ptr() as *mut c_void), &mut info, DIB_RGB_COLORS);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mdc);
        ReleaseDC(Some(hwnd), wdc);
        if !ok || lines == 0 {
            return None;
        }
        let img = downscale_bgra(&bgra, w as usize, h as usize, max_w);
        // komplett schwarz = Inhalt nicht greifbar (z. B. minimiert)
        if img.2.chunks_exact(4).all(|p| p[0] < 8 && p[1] < 8 && p[2] < 8) {
            return None;
        }
        Some(img)
    }
}

/// BGRA → verkleinertes RGBA (Mittelwert über Blöcke)
pub fn downscale_bgra(src: &[u8], w: usize, h: usize, max_w: usize) -> (usize, usize, Vec<u8>) {
    let f = w.div_ceil(max_w.max(1)).max(1);
    let (ow, oh) = ((w / f).max(1), (h / f).max(1));
    let mut out = vec![0u8; ow * oh * 4];
    for oy in 0..oh {
        for ox in 0..ow {
            let mut acc = [0u32; 3];
            let mut n = 0u32;
            for y in oy * f..((oy + 1) * f).min(h) {
                for x in ox * f..((ox + 1) * f).min(w) {
                    let i = (y * w + x) * 4;
                    acc[0] += src[i + 2] as u32;
                    acc[1] += src[i + 1] as u32;
                    acc[2] += src[i] as u32;
                    n += 1;
                }
            }
            let o = (oy * ow + ox) * 4;
            let n = n.max(1);
            out[o] = (acc[0] / n) as u8;
            out[o + 1] = (acc[1] / n) as u8;
            out[o + 2] = (acc[2] / n) as u8;
            out[o + 3] = 255;
        }
    }
    (ow, oh, out)
}

/// Sekunden, seit der Benutzer zuletzt Maus oder Tastatur benutzt hat (systemweit)
pub fn idle_seconds() -> u64 {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    unsafe {
        if !GetLastInputInfo(&mut info).as_bool() {
            return 0;
        }
        (GetTickCount().wrapping_sub(info.dwTime) / 1000) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downscale() {
        // 4×2 BGRA: links blau, rechts rot → 2×1 RGBA
        let mut src = Vec::new();
        for _ in 0..2 {
            src.extend([255, 0, 0, 0, 255, 0, 0, 0, 0, 0, 255, 0, 0, 0, 255, 0]);
        }
        let (w, h, px) = downscale_bgra(&src, 4, 2, 2);
        assert_eq!((w, h), (2, 1));
        assert_eq!(px, vec![0, 0, 255, 255, 255, 0, 0, 255]);
    }

    #[test]
    fn multimon_file() {
        let c = Connection { host: "srv".into(), multimon: true, monitors: " 0, 2 ".into(), ..Default::default() };
        let t = rdp_file_text(&c);
        assert!(t.contains("use multimon:i:1"));
        assert!(t.contains("screen mode id:i:2"));
        assert!(t.contains("selectedmonitors:s:0,2"));
        let c = Connection { host: "srv".into(), ..Default::default() };
        assert!(!rdp_file_text(&c).contains("multimon"));
    }
}
