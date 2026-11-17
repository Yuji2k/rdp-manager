//! Web-Tabs (HTTP/HTTPS) mit Microsoft Edge WebView2 – z. B. für iDRAC/iLO, Drucker, Switches.
//!
//! Das WebView2-Steuerelement liegt wie das RDP-Control in einem eigenen Kindfenster
//! unseres Hauptfensters. Der kleine „WebView2Loader.dll“ (Microsoft, frei weitergebbar)
//! steckt in der exe und wird erst beim ersten Web-Tab nach %APPDATA%\RdpManager entpackt
//! und geladen – die exe bleibt dadurch eine einzelne, portable Datei.

use crate::model::Connection;
use crate::rdp::State;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::OnceLock;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    DocumentTitleChangedEventHandler, NavigationCompletedEventHandler, NewWindowRequestedEventHandler,
    ServerCertificateErrorDetectedEventHandler, SourceChangedEventHandler,
};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, HRESULT, HSTRING, Interface, PCWSTR, PWSTR, s, w};

const LOADER_DLL: &[u8] = include_bytes!("../assets/WebView2Loader.dll");

type CreateEnvFn = unsafe extern "system" fn(PCWSTR, PCWSTR, *mut c_void, *mut c_void) -> HRESULT;

/// Lädt den eingebetteten WebView2Loader (einmal pro Prozess).
fn create_env_fn() -> Result<CreateEnvFn, String> {
    static FN: OnceLock<Result<usize, String>> = OnceLock::new();
    let r = FN.get_or_init(|| {
        let dir = crate::model::Store::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("WebView2Loader.dll");
        let up_to_date = std::fs::metadata(&path).map(|m| m.len() == LOADER_DLL.len() as u64).unwrap_or(false);
        if !up_to_date {
            std::fs::write(&path, LOADER_DLL).map_err(|e| format!("WebView2Loader.dll konnte nicht entpackt werden: {e}"))?;
        }
        unsafe {
            let lib = LoadLibraryW(&HSTRING::from(path.as_os_str()))
                .map_err(|e| format!("WebView2Loader.dll konnte nicht geladen werden: {e}"))?;
            let f = GetProcAddress(lib, s!("CreateCoreWebView2EnvironmentWithOptions"))
                .ok_or("CreateCoreWebView2EnvironmentWithOptions nicht gefunden")?;
            Ok(f as usize)
        }
    });
    match r {
        Ok(addr) => Ok(unsafe { std::mem::transmute::<usize, CreateEnvFn>(*addr) }),
        Err(e) => Err(e.clone()),
    }
}

/// Zustand, den die (asynchronen) WebView2-Rückrufe aktualisieren
#[derive(Default)]
struct Shared {
    controller: RefCell<Option<ICoreWebView2Controller>>,
    webview: RefCell<Option<ICoreWebView2>>,
    error: RefCell<Option<String>>,
    doc_title: RefCell<String>,
    url: RefCell<String>,
    loading: Cell<bool>,
    /// gewünschte Größe/Sichtbarkeit, falls der Controller erst später fertig wird
    size: Cell<(i32, i32)>,
    visible: Cell<bool>,
}

pub struct WebSession {
    pub id: u64,
    pub title: String,
    pub host: String,
    pub icon: &'static str,
    pub conn: Connection,
    container: HWND,
    shared: Rc<Shared>,
    state: State,
    last_rect: Option<(i32, i32, i32, i32)>,
    visible: bool,
    cutouts: Vec<(i32, i32, i32, i32)>,
    started: std::time::Instant,
}

fn take_pwstr(p: PWSTR) -> String {
    webview2_com::take_pwstr(p)
}

impl WebSession {
    /// Fensterhandle des Containers (für Miniaturansichten)
    pub fn hwnd(&self) -> HWND {
        self.container
    }

    pub fn start(id: u64, conn: &Connection, parent: HWND) -> Result<Self, String> {
        let create_env = create_env_fn()?;
        let container = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("Static"),
                None,
                WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0,
                0,
                1,
                1,
                Some(parent),
                None,
                None,
                None,
            )
        }
        .map_err(|e| format!("Fenster für den Web-Tab konnte nicht erstellt werden: {e}"))?;

        let url = conn.web_url();
        let shared = Rc::new(Shared::default());
        *shared.url.borrow_mut() = url.clone();
        shared.loading.set(true);
        let ignore_cert = conn.ignore_cert_errors;

        let sh = shared.clone();
        let env_handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |res, env| {
            let env = match (res, env) {
                (Ok(()), Some(env)) => env,
                (r, _) => {
                    *sh.error.borrow_mut() = Some(format!(
                        "WebView2 nicht verfügbar ({}). Ist die „Microsoft Edge WebView2 Runtime“ installiert?",
                        r.err().map(|e| e.message()).unwrap_or_default()
                    ));
                    return Ok(());
                }
            };
            let sh2 = sh.clone();
            let url = url.clone();
            let ctl_handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |res, ctl| {
                let ctl = match (res, ctl) {
                    (Ok(()), Some(c)) => c,
                    (r, _) => {
                        *sh2.error.borrow_mut() = Some(format!(
                            "Web-Ansicht konnte nicht erstellt werden: {}",
                            r.err().map(|e| e.message()).unwrap_or_default()
                        ));
                        return Ok(());
                    }
                };
                if let Err(e) = setup_controller(&ctl, &sh2, &url, ignore_cert) {
                    *sh2.error.borrow_mut() = Some(format!("Web-Ansicht: {e}"));
                }
                *sh2.controller.borrow_mut() = Some(ctl);
                Ok(())
            }));
            unsafe { env.CreateCoreWebView2Controller(container, &ctl_handler) }
        }));

        let data_dir = HSTRING::from(crate::model::Store::data_dir().join("WebView2").as_os_str());
        let hr = unsafe { create_env(PCWSTR::null(), PCWSTR(data_dir.as_ptr()), std::ptr::null_mut(), env_handler.as_raw()) };
        if hr.is_err() {
            unsafe {
                let _ = DestroyWindow(container);
            }
            return Err(format!(
                "WebView2 konnte nicht gestartet werden ({}). Ist die „Microsoft Edge WebView2 Runtime“ installiert?",
                windows::core::Error::from(hr).message()
            ));
        }

        Ok(Self {
            id,
            title: conn.display_name().to_string(),
            host: conn.host.trim().to_string(),
            icon: conn.icon.glyph(),
            conn: conn.clone(),
            container,
            shared,
            state: State::Connecting,
            last_rect: None,
            visible: false,
            cutouts: Vec::new(),
            started: std::time::Instant::now(),
        })
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn seconds_running(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    pub fn update(&mut self) {
        if let Some(e) = self.shared.error.borrow_mut().take() {
            self.state = State::Failed(e);
        } else if self.state == State::Connecting && self.shared.controller.borrow().is_some() {
            self.state = State::Connected;
            // Größe/Sichtbarkeit nachholen, die vor dem Start gesetzt wurde
            self.apply_bounds();
        }
    }

    /// Aktuelle Adresse und Seitentitel (für die Adressleiste)
    pub fn url(&self) -> String {
        self.shared.url.borrow().clone()
    }

    pub fn page_title(&self) -> String {
        self.shared.doc_title.borrow().clone()
    }

    pub fn is_loading(&self) -> bool {
        self.shared.loading.get()
    }

    fn webview(&self) -> Option<ICoreWebView2> {
        self.shared.webview.borrow().clone()
    }

    pub fn navigate(&self, url: &str) {
        let url = if url.contains("://") { url.to_string() } else { format!("https://{url}") };
        if let Some(wv) = self.webview() {
            let u = HSTRING::from(url);
            unsafe {
                let _ = wv.Navigate(PCWSTR(u.as_ptr()));
            }
        }
    }

    pub fn reload(&self) {
        if let Some(wv) = self.webview() {
            unsafe {
                let _ = wv.Reload();
            }
        }
    }

    pub fn go_back(&self) {
        if let Some(wv) = self.webview() {
            unsafe {
                let _ = wv.GoBack();
            }
        }
    }

    pub fn go_forward(&self) {
        if let Some(wv) = self.webview() {
            unsafe {
                let _ = wv.GoForward();
            }
        }
    }

    pub fn can_go_back(&self) -> bool {
        let mut b = BOOL(0);
        self.webview().is_some_and(|wv| unsafe { wv.CanGoBack(&mut b).is_ok() } && b.as_bool())
    }

    pub fn can_go_forward(&self) -> bool {
        let mut b = BOOL(0);
        self.webview().is_some_and(|wv| unsafe { wv.CanGoForward(&mut b).is_ok() } && b.as_bool())
    }

    fn apply_bounds(&self) {
        let (w, h) = self.shared.size.get();
        if let Some(ctl) = self.shared.controller.borrow().as_ref() {
            unsafe {
                let _ = ctl.SetBounds(RECT { left: 0, top: 0, right: w, bottom: h });
                let _ = ctl.SetIsVisible(self.shared.visible.get());
            }
        }
    }

    /// Positioniert und zeigt die Web-Ansicht (physische Pixel relativ zum Client-Bereich).
    pub fn place(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let rect = (x, y, w.max(1), h.max(1));
        if self.last_rect != Some(rect) || !self.visible {
            unsafe {
                let _ = MoveWindow(self.container, rect.0, rect.1, rect.2, rect.3, true);
                let _ = ShowWindow(self.container, SW_SHOW);
            }
            self.last_rect = Some(rect);
            self.visible = true;
            self.shared.size.set((rect.2, rect.3));
            self.shared.visible.set(true);
            self.apply_bounds();
        }
    }

    pub fn set_cutouts(&mut self, cutouts: &[(i32, i32, i32, i32)]) {
        if self.cutouts == cutouts {
            return;
        }
        self.cutouts = cutouts.to_vec();
        let (_, _, w, h) = self.last_rect.unwrap_or((0, 0, 1, 1));
        crate::rdp::apply_cutouts(self.container, w, h, cutouts);
    }

    pub fn hide(&mut self) {
        if self.visible {
            unsafe {
                let _ = ShowWindow(self.container, SW_HIDE);
            }
            self.visible = false;
            self.shared.visible.set(false);
            self.apply_bounds();
        }
    }

    /// In ein anderes Fenster umhängen (Tab abdocken / wieder andocken).
    pub fn set_parent(&mut self, parent: HWND) {
        crate::rdp::set_parent(self.container, parent);
        self.last_rect = None;
        self.visible = false;
        self.cutouts.clear();
        if let Some(ctl) = self.shared.controller.borrow().as_ref() {
            unsafe {
                let _ = ctl.NotifyParentWindowPositionChanged();
            }
        }
    }

    pub fn focus(&self) {
        if let Some(ctl) = self.shared.controller.borrow().as_ref() {
            unsafe {
                let _ = ctl.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
            }
        }
    }

    pub fn close(&mut self) {
        if let Some(ctl) = self.shared.controller.borrow_mut().take() {
            unsafe {
                let _ = ctl.Close();
            }
        }
        self.shared.webview.borrow_mut().take();
        if !self.container.is_invalid() {
            unsafe {
                let _ = DestroyWindow(self.container);
            }
            self.container = HWND::default();
        }
        self.state = State::Closed;
    }
}

impl Drop for WebSession {
    fn drop(&mut self) {
        if !self.container.is_invalid() {
            self.close();
        }
    }
}

/// Richtet die frisch erstellte Web-Ansicht ein: Ereignisse, Zertifikate, Startseite.
fn setup_controller(ctl: &ICoreWebView2Controller, sh: &Rc<Shared>, url: &str, ignore_cert: bool) -> windows::core::Result<()> {
    unsafe {
        let (w, h) = sh.size.get();
        ctl.SetBounds(RECT { left: 0, top: 0, right: w.max(1), bottom: h.max(1) })?;
        ctl.SetIsVisible(sh.visible.get())?;
        let wv = ctl.CoreWebView2()?;
        let mut token = Default::default();

        // Seitentitel
        let s = sh.clone();
        wv.add_DocumentTitleChanged(
            &DocumentTitleChangedEventHandler::create(Box::new(move |wv, _| {
                if let Some(wv) = wv {
                    let mut p = PWSTR::null();
                    if wv.DocumentTitle(&mut p).is_ok() {
                        *s.doc_title.borrow_mut() = take_pwstr(p);
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;
        // aktuelle Adresse
        let s = sh.clone();
        wv.add_SourceChanged(
            &SourceChangedEventHandler::create(Box::new(move |wv, _| {
                if let Some(wv) = wv {
                    let mut p = PWSTR::null();
                    if wv.Source(&mut p).is_ok() {
                        *s.url.borrow_mut() = take_pwstr(p);
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;
        // Laden fertig
        let s = sh.clone();
        wv.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |_, _| {
                s.loading.set(false);
                Ok(())
            })),
            &mut token,
        )?;
        // Neue Fenster (target=_blank, window.open) im selben Tab öffnen
        wv.add_NewWindowRequested(
            &NewWindowRequestedEventHandler::create(Box::new(move |wv, args| {
                if let (Some(wv), Some(args)) = (wv, args) {
                    let mut p = PWSTR::null();
                    if args.Uri(&mut p).is_ok() {
                        let uri = HSTRING::from(take_pwstr(p));
                        args.SetHandled(true)?;
                        wv.Navigate(PCWSTR(uri.as_ptr()))?;
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;
        // Selbstsignierte Zertifikate (Verwaltungsoberflächen) auf Wunsch akzeptieren
        if ignore_cert && let Ok(wv14) = wv.cast::<ICoreWebView2_14>() {
            wv14.add_ServerCertificateErrorDetected(
                &ServerCertificateErrorDetectedEventHandler::create(Box::new(|_, args| {
                    if let Some(args) = args {
                        args.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_ALWAYS_ALLOW)?;
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }
        let u = HSTRING::from(url);
        wv.Navigate(PCWSTR(u.as_ptr()))?;
        *sh.webview.borrow_mut() = Some(wv);
    }
    Ok(())
}
