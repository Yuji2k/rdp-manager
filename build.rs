// Bettet Programmsymbol und Versionsinfo als Windows-Ressource in die exe ein.
//
// windres wird direkt aufgerufen (statt über ein Crate), weil es Pfade mit
// Leerzeichen ("C:\Claude Code\...") nicht an seinen Präprozessor weiterreichen kann:
// Es läuft deshalb im OUT_DIR mit relativem Dateinamen, der Icon-Pfad steht in der .rc-Datei.
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let icon = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets").join("icon.ico");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut v: Vec<u16> = version.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    v.resize(4, 0);
    let commas = format!("{},{},{},{}", v[0], v[1], v[2], v[3]);

    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {commas}
PRODUCTVERSION {commas}
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040704B0"
    BEGIN
      VALUE "FileDescription", "RDP Manager"
      VALUE "ProductName", "RDP Manager"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
      VALUE "OriginalFilename", "rdp_manager.exe"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0407, 1200
  END
END
"#,
        icon = icon.display().to_string().replace('\\', "/"),
    );
    std::fs::write(out.join("app.rc"), rc).unwrap();

    let status = Command::new("windres")
        .current_dir(&out)
        .args(["app.rc", "-O", "coff", "-o", "app_res.o"])
        .status()
        .expect("windres nicht gefunden (MinGW im PATH? build.ps1 verwenden)");
    assert!(status.success(), "windres ist fehlgeschlagen");
    println!("cargo:rustc-link-arg-bins={}", out.join("app_res.o").display());
}
