//! mam3d : transforme des images 2D en modèles 3D prêts pour Blender.
//!
//! * `mam3d` ouvre la fenêtre (on peut lui passer des images).
//! * `mam3d convert <images|dossiers>...` convertit en ligne de commande (`--json` pour les agents).
//! * `mam3d mcp` lance le serveur MCP pour Claude Code, Codex, Craft Anything.

mod convert;
mod cutout;
mod glb;
mod tripo;
mod export;
mod gui;
mod mcp;
mod mesh;
mod multiview;
mod settings;

use clap::{Args, Parser, Subcommand};
use settings::{Background, Format, Mode, Settings, Shape, ViewAngle};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "mam3d", version, about = "Transforme des images 2D en modèles 3D prêts pour Blender (GLB, OBJ, STL).", args_conflicts_with_subcommands = true)]
struct Cli {
    /// Ouvre la fenêtre, dessine ~120 images, enregistre la fenêtre dans ce PNG et quitte.
    #[arg(long, value_name = "FICHIER.png")]
    screenshot: Option<PathBuf>,
    /// Images ou dossiers à ouvrir dans la fenêtre.
    files: Vec<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Convertit des images (ou des dossiers d'images) en modèles 3D, en parallèle.
    Convert(ConvertArgs),
    /// Reconstruit UN objet complet à partir de plusieurs côtés : une planche (face, profil,
    /// dos... sur une seule image) ou une image par côté.
    Multiview(MultiviewArgs),
    /// Retire l'arrière-plan des images : PNG transparents `<nom>_sans_fond.png`.
    RemoveBg {
        /// Images ou dossiers d'images.
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// Dossier de sortie (défaut : mam3d_sortie à côté de la première image).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Écart de couleur entre l'objet et le fond (plus bas = détourage plus serré).
        #[arg(long, default_value_t = 40.0)]
        threshold: f32,
        /// Repère « garder » : x,y en pixels (la zone de couleur semblable reste). Répétable.
        #[arg(long, value_name = "X,Y")]
        keep: Vec<String>,
        /// Repère « retirer » : x,y en pixels (la zone de couleur semblable part). Répétable.
        #[arg(long, value_name = "X,Y")]
        remove: Vec<String>,
        /// Rayon des repères en pixels.
        #[arg(long, default_value_t = 6.0)]
        radius: f32,
        /// Résultat en JSON (pour les agents).
        #[arg(long)]
        json: bool,
    },
    /// Liste les modes, formats et réglages par défaut.
    Modes {
        /// Sortie JSON.
        #[arg(long)]
        json: bool,
    },
    /// Examine une image et conseille un mode.
    Inspect { path: PathBuf },
    /// Serveur MCP sur stdin/stdout (pour Claude Code, Codex, Craft Anything).
    Mcp,
    /// Enregistre (ou oublie) la clé de l'IA en ligne Tripo 3D (platform.tripo3d.ai).
    TripoKey {
        /// La clé (tsk_...). Sans clé : indique si une clé est enregistrée.
        key: Option<String>,
        /// Oublie la clé enregistrée.
        #[arg(long)]
        forget: bool,
    },
}

#[derive(Args)]
struct MultiviewArgs {
    /// Une planche, ou une image par côté (dans l'ordre de --angles).
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    /// Dossier de sortie (défaut : mam3d_sortie à côté de la première image).
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Côté de chaque vue, séparés par des virgules : auto, front, side_right, back, side_left, top, ignore.
    #[arg(short, long, value_delimiter = ',')]
    angles: Vec<String>,
    /// Genre d'objet : person (dos créé si absent), organic (animal), vehicle (voiture, moto), building.
    #[arg(long, value_enum, default_value = "person")]
    shape: Shape,
    /// local (gratuit, hors ligne) ou tripo (IA en ligne, payant sur votre compte Tripo, bien plus détaillé).
    #[arg(short, long, value_enum, default_value = "local")]
    engine: settings::Engine,
    /// IA : h3.1 (détail maximal) ou p2 (maillage léger en quadrilatères, pour les jeux).
    #[arg(long, default_value = "h3.1")]
    ai_model: String,
    /// IA : nombre de faces visé (0 = choisi par l'IA).
    #[arg(long, default_value_t = 0)]
    faces: u32,
    /// IA : texture détaillée (plus chère).
    #[arg(long)]
    detailed_texture: bool,
    /// IA : sans matériaux PBR.
    #[arg(long)]
    no_pbr: bool,
    #[arg(short, long, value_enum, default_value = "glb")]
    format: Format,
    /// Détail : voxels sur la plus grande dimension (24 à 384).
    #[arg(short, long, default_value_t = 192)]
    resolution: u32,
    /// Taille de la plus grande dimension en mètres (hauteur d'une personne).
    #[arg(short, long, default_value_t = 1.8)]
    size: f32,
    /// Écart de couleur pour retirer le fond.
    #[arg(long, default_value_t = 40.0)]
    threshold: f32,
    /// N'écrit pas importer_dans_blender.py.
    #[arg(long)]
    no_blender_script: bool,
    /// Résultat en JSON (pour les agents).
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct ConvertArgs {
    /// Images ou dossiers d'images.
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    /// Dossier de sortie (défaut : mam3d_sortie à côté de la première image).
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// multiview : chaque image est une planche (plusieurs côtés du même objet).
    #[arg(short, long, value_enum, default_value = "inflate")]
    mode: Mode,
    /// Multi-vues : genre d'objet.
    #[arg(long, value_enum, default_value = "person")]
    shape: Shape,
    #[arg(short, long, value_enum, default_value = "glb")]
    format: Format,
    /// Cellules sur le plus grand côté (8 à 2048).
    #[arg(short, long, default_value_t = 256)]
    resolution: u32,
    /// Épaisseur / hauteur en mètres.
    #[arg(short, long, default_value_t = 0.3)]
    depth: f32,
    /// Taille du plus grand côté en mètres.
    #[arg(short, long, default_value_t = 2.0)]
    size: f32,
    /// Relief : épaisseur de la plaque.
    #[arg(long, default_value_t = 0.04)]
    base: f32,
    /// Inverse clair / sombre.
    #[arg(long)]
    invert: bool,
    #[arg(long, value_enum, default_value = "auto")]
    background: Background,
    /// Écart de couleur pour retirer le fond.
    #[arg(long, default_value_t = 40.0)]
    threshold: f32,
    /// Passes de lissage.
    #[arg(long, default_value_t = 1)]
    smooth: u32,
    /// Gonflage : détail de luminosité ajouté (0-1).
    #[arg(long, default_value_t = 0.15)]
    detail: f32,
    /// Gonflage : épaisseur du dos (0 = plat, 1 = symétrique).
    #[arg(long, default_value_t = 1.0)]
    back_scale: f32,
    /// Taille max de la texture (pixels).
    #[arg(long, default_value_t = 2048)]
    texture_max: u32,
    /// Nombre de fils de calcul (défaut : tous les cœurs).
    #[arg(short = 'j', long)]
    threads: Option<usize>,
    /// N'écrit pas importer_dans_blender.py.
    #[arg(long)]
    no_blender_script: bool,
    /// Résultat en JSON (pour les agents).
    #[arg(long)]
    json: bool,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Convert(a)) => std::process::exit(run_convert(a)),
        Some(Command::Multiview(a)) => std::process::exit(run_multiview(a)),
        Some(Command::RemoveBg { inputs, output, threshold, keep, remove, radius, json }) => {
            let mut pins = Vec::new();
            for (list, k) in [(&keep, true), (&remove, false)] {
                for t in list {
                    let v: Vec<f32> = t.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                    if v.len() != 2 {
                        eprintln!("repère invalide : {t} (attendu x,y)");
                        std::process::exit(2);
                    }
                    pins.push(cutout::Pin { x: v[0], y: v[1], keep: k, radius });
                }
            }
            let out = output.unwrap_or_else(|| mcp::default_output_dir(&inputs[0]));
            let reports = convert::remove_backgrounds(&inputs, &out, threshold, &pins);
            if json {
                println!("{}", serde_json::to_string_pretty(&reports).unwrap());
            } else {
                for r in &reports {
                    match &r.error {
                        None => println!("✔ {} → {}", r.input, r.outputs.join(", ")),
                        Some(e) => eprintln!("✖ {} : {e}", r.input),
                    }
                }
            }
            let ok = !reports.is_empty() && reports.iter().all(|r| r.ok);
            std::process::exit(if ok { 0 } else { 1 });
        }
        Some(Command::Modes { json }) => {
            let info = mcp::modes_info();
            if json {
                println!("{}", serde_json::to_string_pretty(&info).unwrap());
            } else {
                println!("Modes :");
                for m in Mode::ALL {
                    println!("  {:<8} {} — {}", m.id(), m.label(), m.help());
                }
                println!("Formats : glb (recommandé), obj, stl");
            }
        }
        Some(Command::Inspect { path }) => match mcp::inspect(&path.display().to_string()) {
            Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        },
        Some(Command::Mcp) => mcp::serve(),
        Some(Command::TripoKey { key, forget }) => {
            if forget {
                tripo::forget_key();
                println!("Clé Tripo oubliée.");
            } else if let Some(k) = key {
                match tripo::save_key(&k) {
                    Ok(()) => println!("Clé Tripo enregistrée."),
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(1);
                    }
                }
            } else {
                println!("{}", if tripo::api_key().is_some() { "Une clé Tripo est configurée." } else { "Aucune clé Tripo." });
            }
        }
        None => run_window(cli.files, cli.screenshot),
    }
}

fn run_multiview(a: MultiviewArgs) -> i32 {
    let mut angles = Vec::new();
    for t in &a.angles {
        match ViewAngle::parse(t) {
            Some(v) => angles.push(v),
            None => {
                eprintln!("côté inconnu : {t} (auto, front, side_right, back, side_left, top, ignore)");
                return 2;
            }
        }
    }
    let s = Settings {
        mode: Mode::Multiview,
        shape: a.shape,
        engine: a.engine,
        ai_model: a.ai_model.to_ascii_lowercase(),
        ai_faces: a.faces,
        ai_detailed_texture: a.detailed_texture,
        ai_pbr: !a.no_pbr,
        format: a.format,
        resolution: a.resolution.clamp(24, 384),
        size: a.size,
        threshold: a.threshold,
        ..Settings::default()
    };
    let out = a.output.clone().unwrap_or_else(|| mcp::default_output_dir(&a.inputs[0]));
    let summary = convert::convert_views(&a.inputs, &out, &s, &angles, !a.no_blender_script, &|m| eprintln!("{m}"));
    if a.json {
        println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    } else {
        for r in &summary.results {
            if let Some(vs) = &r.views {
                for v in vs {
                    println!("  vue {} : {}{}", v.source, v.angle, if v.guessed { " (deviné)" } else { "" });
                }
            }
            if r.ok {
                println!("✔ {} ({} triangles, {} ms)", r.outputs.first().cloned().unwrap_or_default(), r.triangles, r.millis);
            } else {
                eprintln!("✖ {}", r.error.clone().unwrap_or_default());
            }
        }
        if let Some(p) = &summary.blender_script {
            println!("Script Blender : {p}");
        }
    }
    if summary.ok == 1 { 0 } else { 1 }
}

fn run_convert(a: ConvertArgs) -> i32 {
    let s = Settings {
        shape: a.shape,
        mode: a.mode,
        format: a.format,
        resolution: a.resolution.clamp(8, 2048),
        depth: a.depth,
        size: a.size,
        base: a.base,
        invert: a.invert,
        background: a.background,
        threshold: a.threshold,
        smooth: a.smooth,
        detail: a.detail,
        back_scale: a.back_scale,
        texture_max: a.texture_max,
        ..Settings::default()
    };
    let out = a.output.clone().unwrap_or_else(|| mcp::default_output_dir(&a.inputs[0]));
    let json = a.json;
    let summary = convert::convert_many(&a.inputs, &out, &s, a.threads, !a.no_blender_script, &|r| {
        if !json {
            if r.ok {
                println!("✔ {} → {} ({} triangles, {} ms)", r.input, r.outputs.first().cloned().unwrap_or_default(), r.triangles, r.millis);
            } else {
                eprintln!("✖ {} : {}", r.input, r.error.clone().unwrap_or_default());
            }
        }
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    } else {
        println!("{} réussie(s), {} échec(s) en {} ms → {}", summary.ok, summary.failed, summary.total_millis, summary.output_dir);
        if let Some(p) = &summary.blender_script {
            println!("Script Blender : {p}");
        }
    }
    if summary.results.is_empty() {
        eprintln!("Aucune image trouvée.");
        2
    } else if summary.failed > 0 {
        1
    } else {
        0
    }
}

#[cfg(windows)]
fn hide_own_console() {
    // Lancé depuis l'explorateur, Windows ouvre une console vide : on la ferme pour la fenêtre.
    unsafe extern "system" {
        fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
        fn FreeConsole() -> i32;
    }
    let mut ids = [0u32; 4];
    unsafe {
        if GetConsoleProcessList(ids.as_mut_ptr(), 4) == 1 {
            FreeConsole();
        }
    }
}

fn run_window(files: Vec<PathBuf>, screenshot: Option<PathBuf>) {
    #[cfg(windows)]
    if screenshot.is_none() {
        hide_own_console();
    }
    let shot = screenshot.is_some();
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("mam3d — images 2D vers 3D")
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([900.0, 560.0])
        .with_drag_and_drop(true);
    if let Ok(icon) = image::load_from_memory(include_bytes!("../assets/icon_256.png")) {
        let icon = icon.to_rgba8();
        let (w, h) = icon.dimensions();
        viewport = viewport.with_icon(std::sync::Arc::new(eframe::egui::IconData { rgba: icon.into_raw(), width: w, height: h }));
    }
    if shot {
        viewport = viewport.with_active(false);
        #[cfg(windows)]
        {
            viewport = viewport.with_window_level(eframe::egui::WindowLevel::AlwaysOnBottom);
        }
    }
    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions { viewport, ..Default::default() };
    #[cfg(target_os = "macos")]
    if shot {
        options.event_loop_builder = Some(Box::new(|b| {
            use winit::platform::macos::EventLoopBuilderExtMacOS;
            b.with_activate_ignoring_other_apps(false);
        }));
    }
    let r = eframe::run_native("mam3d", options, Box::new(move |cc| Ok(Box::new(gui::App::new(cc, files, screenshot)))));
    if let Err(e) = r {
        eprintln!("Impossible d'ouvrir la fenêtre : {e}");
        std::process::exit(1);
    }
}
