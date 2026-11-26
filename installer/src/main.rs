//! Installateur de mam3d pour Windows : un seul fichier, sans droits administrateur.
//!
//! * `mam3d-setup.exe`              fenêtre d'installation
//! * `mam3d-setup.exe /S`           installation silencieuse (options par défaut)
//! * `desinstaller.exe --uninstall` fenêtre de désinstallation (`/S` : silencieuse)
//!
//! Installe dans %LOCALAPPDATA%\Programs\mam3d : mam3d.exe, README.md, desinstaller.exe ;
//! raccourcis menu Démarrer (et bureau), entrée « Applications installées », et en option
//! le dossier dans le PATH et la connexion à Claude Code (serveur MCP).

#![windows_subsystem = "windows"]

use eframe::egui::{self, Color32, RichText, Vec2};
use std::path::{Path, PathBuf};
use std::process::Command;

const APP: &[u8] = include_bytes!(env!("MAM3D_EXE"));
const README: &[u8] = include_bytes!("../../README.md");
const VERSION: &str = match option_env!("MAM3D_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
const UNINSTALL_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\mam3d";
const ACCENT: Color32 = Color32::from_rgb(255, 122, 69);

fn install_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\mam3d"));
    base.join("Programs").join("mam3d")
}

fn start_menu_link() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
    base.join(r"Microsoft\Windows\Start Menu\Programs\mam3d.lnk")
}

/// Commande cachée (pas de fenêtre noire).
fn hidden(program: &str) -> Command {
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn powershell(script: &str) -> Result<String, String> {
    let out = hidden("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .output()
        .map_err(|e| format!("PowerShell introuvable : {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn desktop_link() -> Option<PathBuf> {
    powershell("[Environment]::GetFolderPath('Desktop')").ok().filter(|s| !s.is_empty()).map(|d| PathBuf::from(d).join("mam3d.lnk"))
}

fn make_shortcut(link: &Path, target: &Path) -> Result<(), String> {
    if let Some(p) = link.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let dir = target.parent().unwrap_or(Path::new("."));
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut({});$s.TargetPath={};$s.WorkingDirectory={};$s.IconLocation={};$s.Description='Images 2D en modèles 3D pour Blender';$s.Save()",
        ps_quote(&link.display().to_string()),
        ps_quote(&target.display().to_string()),
        ps_quote(&dir.display().to_string()),
        ps_quote(&format!("{},0", target.display())),
    );
    powershell(&script).map(|_| ())
}

fn claude_available() -> bool {
    hidden("cmd").args(["/c", "where", "claude"]).output().map(|o| o.status.success()).unwrap_or(false)
}

#[derive(Clone)]
struct Options {
    desktop: bool,
    path: bool,
    claude: bool,
    launch: bool,
}

/// Installe (ou met à jour) mam3d. Renvoie le compte rendu, étape par étape.
fn install(o: &Options) -> Result<Vec<String>, String> {
    let dir = install_dir();
    let mut log = Vec::new();
    std::fs::create_dir_all(&dir).map_err(|e| format!("dossier {} : {e}", dir.display()))?;
    let exe = dir.join("mam3d.exe");
    std::fs::write(&exe, APP).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(32) {
            "mam3d est ouvert : fermez-le puis relancez l'installation".to_string()
        } else {
            format!("copie de mam3d.exe : {e}")
        }
    })?;
    let _ = std::fs::write(dir.join("README.md"), README);
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let uninstaller = dir.join("desinstaller.exe");
    if me != uninstaller {
        std::fs::copy(&me, &uninstaller).map_err(|e| format!("copie du désinstallateur : {e}"))?;
    }
    log.push(format!("Programme installé dans {}", dir.display()));

    match make_shortcut(&start_menu_link(), &exe) {
        Ok(()) => log.push("Raccourci ajouté au menu Démarrer".into()),
        Err(e) => log.push(format!("Raccourci du menu Démarrer non créé : {e}")),
    }
    if o.desktop {
        match desktop_link().ok_or("bureau introuvable".to_string()).and_then(|l| make_shortcut(&l, &exe)) {
            Ok(()) => log.push("Raccourci ajouté sur le bureau".into()),
            Err(e) => log.push(format!("Raccourci du bureau non créé : {e}")),
        }
    }

    // Entrée « Applications installées » (Paramètres Windows), pour la désinstallation.
    let size_kb = (APP.len() as u64 * 2 / 1024).to_string();
    let values: [(&str, &str, String); 9] = [
        ("DisplayName", "REG_SZ", "mam3d".into()),
        ("DisplayVersion", "REG_SZ", VERSION.into()),
        ("Publisher", "REG_SZ", "mam3d".into()),
        ("DisplayIcon", "REG_SZ", exe.display().to_string()),
        ("InstallLocation", "REG_SZ", dir.display().to_string()),
        ("UninstallString", "REG_SZ", format!("\"{}\" --uninstall", uninstaller.display())),
        ("QuietUninstallString", "REG_SZ", format!("\"{}\" --uninstall /S", uninstaller.display())),
        ("NoModify", "REG_DWORD", "1".into()),
        ("EstimatedSize", "REG_DWORD", size_kb),
    ];
    let mut reg_ok = true;
    for (name, kind, data) in values.iter() {
        let ok = hidden("reg").args(["add", UNINSTALL_KEY, "/v", name, "/t", kind, "/d", data, "/f"]).output().map(|o| o.status.success()).unwrap_or(false);
        reg_ok &= ok;
    }
    let _ = hidden("reg").args(["add", UNINSTALL_KEY, "/v", "NoRepair", "/t", "REG_DWORD", "/d", "1", "/f"]).output();
    log.push(if reg_ok { "Ajouté aux applications installées de Windows".into() } else { "Entrée des applications installées incomplète".into() });

    if o.path {
        let d = ps_quote(&dir.display().to_string());
        let script = format!(
            "$d={d};$p=[Environment]::GetEnvironmentVariable('Path','User');if($null -eq $p){{$p=''}};if(-not (($p -split ';') -contains $d)){{[Environment]::SetEnvironmentVariable('Path',($p.TrimEnd(';')+';'+$d).TrimStart(';'),'User')}}"
        );
        match powershell(&script) {
            Ok(_) => log.push("La commande « mam3d » marche dans les nouveaux terminaux (PATH)".into()),
            Err(e) => log.push(format!("PATH non modifié : {e}")),
        }
    }
    if o.claude {
        let _ = hidden("cmd").args(["/c", "claude", "mcp", "remove", "--scope", "user", "mam3d"]).output();
        let ok = hidden("cmd")
            .args(["/c", "claude", "mcp", "add", "--scope", "user", "mam3d", "--", &exe.display().to_string(), "mcp"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        log.push(if ok { "mam3d est connecté à Claude Code (outils MCP)".into() } else { "Connexion à Claude Code impossible (faites-la depuis la fenêtre de mam3d)".into() });
    }
    Ok(log)
}

fn uninstall() -> Vec<String> {
    let dir = install_dir();
    let mut log = Vec::new();
    let _ = std::fs::remove_file(start_menu_link());
    if let Some(l) = desktop_link() {
        let _ = std::fs::remove_file(l);
    }
    log.push("Raccourcis retirés".into());
    let _ = hidden("reg").args(["delete", UNINSTALL_KEY, "/f"]).output();
    let d = ps_quote(&dir.display().to_string());
    let _ = powershell(&format!(
        "$d={d};$p=[Environment]::GetEnvironmentVariable('Path','User');if($p){{[Environment]::SetEnvironmentVariable('Path',(($p -split ';') | Where-Object {{ $_ -and $_ -ne $d }}) -join ';','User')}}"
    ));
    // Ne retire la connexion Claude Code que si elle mène à cette installation.
    if claude_available() {
        let info = hidden("cmd").args(["/c", "claude", "mcp", "get", "mam3d"]).output().map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase()).unwrap_or_default();
        if info.contains(&dir.display().to_string().to_lowercase()) {
            let _ = hidden("cmd").args(["/c", "claude", "mcp", "remove", "--scope", "user", "mam3d"]).output();
            log.push("Connexion à Claude Code retirée".into());
        }
    }
    let _ = std::fs::remove_file(dir.join("mam3d.exe"));
    let _ = std::fs::remove_file(dir.join("README.md"));
    // Le désinstallateur ne peut pas s'effacer lui-même pendant qu'il tourne : on demande à
    // Windows de supprimer le dossier deux secondes après sa fermeture.
    let mut cleanup = hidden("cmd");
    cleanup.current_dir(std::env::temp_dir());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Ligne brute : cmd ne comprend pas les guillemets échappés à la manière de Rust.
        cleanup.raw_arg(format!("/c ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"", dir.display()));
    }
    let _ = cleanup.spawn();
    log.push(format!("mam3d retiré de {}", dir.display()));
    log
}

fn launch_app() {
    let _ = Command::new(install_dir().join("mam3d.exe")).spawn();
}

enum State {
    Ready,
    Done(Vec<String>),
    Failed(String),
}

struct Setup {
    uninstall_mode: bool,
    opts: Options,
    has_claude: bool,
    state: State,
}

impl eframe::App for Setup {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("mam").size(30.0).strong().color(Color32::WHITE));
                ui.add_space(-8.0);
                ui.label(RichText::new("3d").size(30.0).strong().color(ACCENT));
                ui.label(RichText::new(format!("  version {VERSION}")).color(Color32::GRAY));
            });
            ui.label(RichText::new("Images 2D en modèles 3D prêts pour Blender").color(Color32::GRAY));
            ui.add_space(12.0);
            match &self.state {
                State::Ready if self.uninstall_mode => {
                    ui.label("Retirer mam3d de cet ordinateur ? Vos modèles 3D et vos images ne sont pas touchés.");
                    ui.add_space(16.0);
                    if big_button(ui, "Désinstaller").clicked() {
                        self.state = State::Done(uninstall());
                    }
                }
                State::Ready => {
                    ui.label(format!("mam3d sera installé pour vous seul, sans droits administrateur, dans :\n{}", install_dir().display()));
                    ui.add_space(10.0);
                    ui.checkbox(&mut self.opts.desktop, "Raccourci sur le bureau");
                    ui.checkbox(&mut self.opts.path, "Commande « mam3d » dans les terminaux (pour Codex, scripts…)");
                    if self.has_claude {
                        ui.checkbox(&mut self.opts.claude, "Connecter mam3d à Claude Code (outils MCP)");
                    }
                    ui.checkbox(&mut self.opts.launch, "Ouvrir mam3d à la fin");
                    ui.add_space(16.0);
                    if big_button(ui, "Installer").clicked() {
                        self.state = match install(&self.opts) {
                            Ok(l) => State::Done(l),
                            Err(e) => State::Failed(e),
                        };
                    }
                }
                State::Done(log) => {
                    for l in log {
                        ui.label(RichText::new(format!("•  {l}")).color(Color32::LIGHT_GRAY));
                    }
                    ui.add_space(16.0);
                    let label = if self.uninstall_mode { "Fermer" } else { "Terminer" };
                    if big_button(ui, label).clicked() {
                        if !self.uninstall_mode && self.opts.launch {
                            launch_app();
                        }
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                State::Failed(e) => {
                    ui.label(RichText::new(format!("L'installation n'a pas abouti : {e}")).color(Color32::from_rgb(235, 95, 95)));
                    ui.add_space(16.0);
                    if big_button(ui, "Réessayer").clicked() {
                        self.state = State::Ready;
                    }
                }
            }
        });
    }
}

fn big_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).size(17.0).strong().color(Color32::WHITE)).fill(ACCENT).corner_radius(8).min_size(Vec2::new(200.0, 40.0)))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let uninstall_mode = args.iter().any(|a| a == "--uninstall");
    let silent = args.iter().any(|a| a.eq_ignore_ascii_case("/S") || a == "--silent");
    let has_claude = claude_available();
    let flag = |f: &str| args.iter().any(|a| a.eq_ignore_ascii_case(f));
    // /nodesktop /nopath /noclaude : décocher une option (utile en installation silencieuse).
    let opts = Options { desktop: !flag("/nodesktop"), path: !flag("/nopath"), claude: has_claude && !flag("/noclaude"), launch: true };
    if silent {
        let log = if uninstall_mode {
            Ok(uninstall())
        } else {
            install(&Options { launch: false, ..opts })
        };
        // Compte rendu à côté du programme d'installation (une fenêtre cachée n'affiche rien).
        let text = match &log {
            Ok(l) => l.join("\n"),
            Err(e) => format!("ÉCHEC : {e}"),
        };
        let _ = std::fs::write(std::env::temp_dir().join("mam3d-setup.log"), text);
        if log.is_err() {
            std::process::exit(1);
        }
        return;
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(if uninstall_mode { "Désinstaller mam3d" } else { "Installer mam3d" })
        .with_inner_size([560.0, 400.0])
        .with_resizable(false);
    if let Ok(icon) = image::load_from_memory(include_bytes!("../../assets/icon_256.png")) {
        let icon = icon.to_rgba8();
        let (w, h) = icon.dimensions();
        viewport = viewport.with_icon(std::sync::Arc::new(egui::IconData { rgba: icon.into_raw(), width: w, height: h }));
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    let _ = eframe::run_native(
        "mam3d-setup",
        options,
        Box::new(move |cc| {
            let mut v = egui::Visuals::dark();
            v.panel_fill = Color32::from_rgb(30, 33, 41);
            v.selection.bg_fill = Color32::from_rgb(196, 78, 36);
            cc.egui_ctx.set_visuals(v);
            Ok(Box::new(Setup { uninstall_mode, opts, has_claude, state: State::Ready }))
        }),
    );
}
