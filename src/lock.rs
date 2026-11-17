//! Programmsperre: Hauptkennwort und/oder Windows Hello beim Start und nach Leerlauf.
//!
//! Die Sperre schützt vor der Benutzung durch andere am entsperrten PC (Sitzungen ansehen,
//! Verbindungen mit gespeicherten Passwörtern öffnen, Export mit Passwörtern). Die Passwörter
//! selbst liegen weiterhin verschlüsselt im Windows-Tresor des Benutzers.

use crate::model::LockSettings;
use base64::Engine as _;
use eframe::egui;
use std::sync::mpsc;
use windows::Win32::Foundation::HWND;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
const ITERATIONS: u32 = 200_000;

/// Hash für das Hauptkennwort: "iterationen$salt$hash" (Base64)
pub fn hash_password(pw: &str) -> String {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).expect("Zufallszahlen");
    format!("{ITERATIONS}${}${}", B64.encode(salt), B64.encode(derive(pw, &salt, ITERATIONS)))
}

fn derive(pw: &str, salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(pw.as_bytes(), salt, iterations, &mut out);
    out
}

pub fn verify_password(pw: &str, stored: &str) -> bool {
    let mut parts = stored.split('$');
    let (Some(it), Some(salt), Some(hash), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let (Ok(it), Ok(salt), Ok(hash)) = (it.parse::<u32>(), B64.decode(salt), B64.decode(hash)) else { return false };
    let got = derive(pw, &salt, it);
    // Vergleich in konstanter Zeit
    got.len() == hash.len() && got.iter().zip(&hash).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

// ---------------------------------------------------------------------------
// Windows Hello
// ---------------------------------------------------------------------------

/// Ist Windows Hello (PIN, Fingerabdruck, Gesicht) eingerichtet?
pub fn hello_available() -> bool {
    use windows::Security::Credentials::UI::{UserConsentVerifier, UserConsentVerifierAvailability};
    UserConsentVerifier::CheckAvailabilityAsync()
        .and_then(|op| op.join())
        .is_ok_and(|a| a == UserConsentVerifierAvailability::Available)
}

/// Fragt Windows Hello im Hintergrund ab; das Ergebnis kommt über den Kanal (true = bestätigt).
pub fn request_hello(parent: HWND, message: &str, ctx: &egui::Context) -> mpsc::Receiver<Result<bool, String>> {
    let (tx, rx) = mpsc::channel();
    let hwnd = parent.0 as isize;
    let message = message.to_string();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let r = hello_blocking(HWND(hwnd as *mut std::ffi::c_void), &message);
        let _ = tx.send(r);
        ctx.request_repaint();
    });
    rx
}

fn hello_blocking(parent: HWND, message: &str) -> Result<bool, String> {
    use windows_future::IAsyncOperation;
    use windows::Security::Credentials::UI::{UserConsentVerificationResult, UserConsentVerifier};
    use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
    let interop = windows::core::factory::<UserConsentVerifier, IUserConsentVerifierInterop>()
        .map_err(|e| format!("Windows Hello nicht verfügbar: {e}"))?;
    let op: IAsyncOperation<UserConsentVerificationResult> =
        unsafe { interop.RequestVerificationForWindowAsync(parent, &windows::core::HSTRING::from(message)) }
            .map_err(|e| format!("Windows Hello: {e}"))?;
    match op.join().map_err(|e| format!("Windows Hello: {e}"))? {
        UserConsentVerificationResult::Verified => Ok(true),
        UserConsentVerificationResult::Canceled => Ok(false),
        UserConsentVerificationResult::DeviceNotPresent | UserConsentVerificationResult::NotConfiguredForUser => {
            Err("Windows Hello ist auf diesem PC nicht eingerichtet.".into())
        }
        UserConsentVerificationResult::DisabledByPolicy => Err("Windows Hello ist per Richtlinie deaktiviert.".into()),
        UserConsentVerificationResult::RetriesExhausted => Err("Zu viele Fehlversuche bei Windows Hello.".into()),
        _ => Err("Windows Hello ist gerade nicht verfügbar.".into()),
    }
}

// ---------------------------------------------------------------------------
// Sperrbildschirm
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct Lock {
    pub locked: bool,
    input: String,
    error: Option<String>,
    hello: Option<mpsc::Receiver<Result<bool, String>>>,
    /// Windows Hello beim Sperren automatisch einmal anbieten
    hello_pending: bool,
}

impl Lock {
    pub fn lock(&mut self, settings: &LockSettings) {
        if !settings.enabled() || self.locked {
            return;
        }
        self.locked = true;
        self.input.clear();
        self.error = None;
        self.hello_pending = settings.hello;
    }

    /// Zeichnet den Sperrbildschirm. Rückgabe: true, wenn gerade entsperrt wurde.
    pub fn ui(&mut self, ui: &mut egui::Ui, settings: &LockSettings, parent: HWND) -> bool {
        let ctx = ui.ctx().clone();
        // Ergebnis von Windows Hello
        if let Some(rx) = &self.hello {
            match rx.try_recv() {
                Ok(Ok(true)) => {
                    self.hello = None;
                    return self.unlock();
                }
                Ok(Ok(false)) => self.hello = None,
                Ok(Err(e)) => {
                    self.error = Some(e);
                    self.hello = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => self.hello = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if std::mem::take(&mut self.hello_pending) && self.hello.is_none() {
            self.hello = Some(request_hello(parent, "RDP Manager entsperren", &ctx));
        }
        let mut unlocked = false;
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() / 3.0 - 60.0).max(20.0));
                ui.heading(egui::RichText::new("🔒 RDP Manager ist gesperrt").size(26.0));
                ui.add_space(6.0);
                ui.weak("Offene Sitzungen bleiben verbunden und sind nach dem Entsperren wieder da.");
                ui.add_space(16.0);
                if !settings.password_hash.is_empty() {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.input)
                            .password(true)
                            .hint_text("Hauptkennwort")
                            .desired_width(260.0),
                    );
                    if self.hello.is_none() && !r.has_focus() && ui.memory(|m| m.focused().is_none()) {
                        r.request_focus();
                    }
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.add_space(6.0);
                    if (ui.button("Entsperren").clicked() || enter) && !self.input.is_empty() {
                        if verify_password(&self.input, &settings.password_hash) {
                            unlocked = true;
                        } else {
                            self.error = Some("Falsches Kennwort.".into());
                            self.input.clear();
                        }
                    }
                }
                if settings.hello {
                    ui.add_space(6.0);
                    if self.hello.is_some() {
                        ui.horizontal(|ui| {
                            ui.add_space((ui.available_width() - 220.0).max(0.0) / 2.0);
                            ui.spinner();
                            ui.label("Warte auf Windows Hello …");
                        });
                    } else if ui.button("🙂 Mit Windows Hello entsperren").clicked() {
                        self.error = None;
                        self.hello = Some(request_hello(parent, "RDP Manager entsperren", &ctx));
                    }
                }
                if let Some(e) = &self.error {
                    ui.add_space(8.0);
                    ui.colored_label(egui::Color32::from_rgb(220, 70, 70), e);
                    if settings.password_hash.is_empty() {
                        ui.weak("Falls Windows Hello dauerhaft nicht geht: Programm beenden und in %APPDATA%\\RdpManager\\connections.json den Abschnitt „lock“ löschen.");
                    }
                }
            });
        });
        if unlocked {
            return self.unlock();
        }
        if self.hello.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        false
    }

    fn unlock(&mut self) -> bool {
        self.locked = false;
        self.input.clear();
        self.error = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let h = hash_password("geheim äöü");
        assert!(verify_password("geheim äöü", &h));
        assert!(!verify_password("geheim", &h));
        assert!(!verify_password("x", "kaputt"));
        assert_ne!(hash_password("a"), hash_password("a")); // Salz
    }
}
