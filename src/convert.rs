//! Conversion d'une ou plusieurs images, en parallèle sur tous les cœurs du processeur.

use crate::export;
use crate::mesh;
use crate::settings::{Engine, Format, Mode, Settings, ViewAngle};
use rayon::prelude::*;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const IMAGE_EXTENSIONS: [&str; 9] = ["png", "jpg", "jpeg", "webp", "bmp", "tga", "gif", "tif", "tiff"];

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub input: String,
    pub ok: bool,
    pub outputs: Vec<String>,
    pub vertices: usize,
    pub triangles: usize,
    pub size_m: [f32; 3],
    pub millis: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Multi-vues : les vues trouvées et le côté de chacune.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub views: Option<Vec<crate::multiview::ViewInfo>>,
}

impl Report {
    fn new(input: String) -> Self {
        Report { input, ok: false, outputs: vec![], vertices: 0, triangles: 0, size_m: [0.0; 3], millis: 0, error: None, views: None }
    }
}

/// Écrit le modèle dans le format choisi et complète le rapport.
fn write_model(m: &mesh::Mesh, texture: &image::RgbaImage, name: &str, out_dir: &Path, s: &Settings, tex_max: u32, r: &mut Report) -> Result<(), String> {
    std::fs::create_dir_all(out_dir).map_err(|e| format!("dossier de sortie : {e}"))?;
    let out = out_dir.join(format!("{name}.{}", s.format.id()));
    match s.format {
        Format::Glb => {
            let tex = export::texture_png(texture, tex_max)?;
            export::write_glb(&out, m, name, Some(&tex))?;
            r.outputs.push(out.display().to_string());
        }
        Format::Obj => {
            let tex = export::texture_png(texture, tex_max)?;
            for f in export::write_obj(&out, m, name, Some(&tex))? {
                r.outputs.push(f.display().to_string());
            }
        }
        Format::Stl => {
            export::write_stl(&out, m)?;
            r.outputs.push(out.display().to_string());
        }
    }
    let (min, max) = m.bounds();
    r.size_m = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    r.vertices = m.positions.len();
    r.triangles = m.triangle_count();
    Ok(())
}

/// Retire l'arrière-plan de chaque image (en parallèle) et l'enregistre en PNG transparent
/// `<nom>_sans_fond.png` dans le dossier de sortie.
pub fn remove_backgrounds(inputs: &[PathBuf], out_dir: &Path, threshold: f32, pins: &[crate::cutout::Pin]) -> Vec<Report> {
    let files = expand_inputs(&inputs.iter().map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone())).collect::<Vec<_>>());
    let out_dir = std::path::absolute(out_dir).unwrap_or_else(|_| out_dir.to_path_buf());
    files
        .par_iter()
        .map(|f| {
            let start = Instant::now();
            let mut r = Report::new(f.display().to_string());
            let res = (|| -> Result<(), String> {
                let img = image::open(f).map_err(|e| format!("lecture de l'image : {e}"))?.to_rgba8();
                let cut = crate::cutout::cutout(&img, threshold, pins);
                let kept = cut.pixels().filter(|p| p.0[3] > 0).count();
                if kept == 0 {
                    return Err("aucun objet trouvé : baissez le seuil du fond".into());
                }
                std::fs::create_dir_all(&out_dir).map_err(|e| format!("dossier de sortie : {e}"))?;
                let out = out_dir.join(format!("{}_sans_fond.png", safe_stem(f)));
                cut.save(&out).map_err(|e| format!("écriture : {e}"))?;
                r.outputs.push(out.display().to_string());
                Ok(())
            })();
            match res {
                Ok(()) => r.ok = true,
                Err(e) => r.error = Some(e),
            }
            r.millis = start.elapsed().as_millis();
            r
        })
        .collect()
}

/// Multi-vues : toutes les images sont des côtés du même objet (ou une seule planche).
/// Multi-vues avec le moteur choisi (local ou IA Tripo), puis écriture du modèle.
fn multiview_model(
    images: Vec<(String, image::RgbaImage)>,
    angles: &[ViewAngle],
    s: &Settings,
    out_dir: &Path,
    name: &str,
    progress: &(dyn Fn(String) + Sync),
    r: &mut Report,
) -> Result<(), String> {
    match s.engine {
        Engine::Local => {
            let rec = crate::multiview::from_images(images, angles, s)?;
            r.views = Some(rec.views.clone());
            write_model(&rec.mesh, &rec.texture, name, out_dir, s, s.texture_max.max(4096), r)
        }
        Engine::Tripo => {
            let ai = crate::tripo::reconstruct(images, angles, s, out_dir, name, progress)?;
            r.views = Some(ai.views.clone());
            let tex = ai.texture.unwrap_or_else(|| image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 200, 200, 255])));
            let ai_name = format!("{name}_ia");
            write_model(&ai.mesh, &tex, &ai_name, out_dir, s, s.texture_max.max(4096), r)?;
            // Le fichier d'origine de l'IA (matériaux PBR complets) est gardé à côté.
            r.outputs.push(ai.original_glb.display().to_string());
            progress(format!("IA terminée (tâche {})", ai.task_id));
            Ok(())
        }
    }
}

pub fn convert_views(
    inputs: &[PathBuf],
    out_dir: &Path,
    s: &Settings,
    angles: &[ViewAngle],
    blender_script: bool,
    progress: &(dyn Fn(String) + Sync),
) -> BatchSummary {
    let start = Instant::now();
    let inputs: Vec<PathBuf> = expand_inputs(&inputs.iter().map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone())).collect::<Vec<_>>());
    let out_dir = std::path::absolute(out_dir).unwrap_or_else(|_| out_dir.to_path_buf());
    let mut r = Report::new(inputs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(" + "));
    let result = (|| -> Result<(), String> {
        if inputs.is_empty() {
            return Err("aucune image".into());
        }
        let images: Vec<(String, image::RgbaImage)> = inputs
            .par_iter()
            .map(|p| {
                image::open(p)
                    .map(|i| (p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default(), i.to_rgba8()))
                    .map_err(|e| format!("{} : {e}", p.display()))
            })
            .collect::<Result<_, _>>()?;
        multiview_model(images, angles, s, &out_dir, &safe_stem(&inputs[0]), progress, &mut r)
    })();
    match result {
        Ok(()) => r.ok = true,
        Err(e) => r.error = Some(e),
    }
    r.millis = start.elapsed().as_millis();
    let ok = r.ok as usize;
    let mut summary = BatchSummary {
        ok,
        failed: 1 - ok,
        total_millis: 0,
        output_dir: out_dir.display().to_string(),
        blender_script: None,
        settings: s.clone(),
        results: vec![r],
    };
    if blender_script && ok > 0 {
        if let Ok(p) = write_blender_script(&out_dir, &summary.results, s.format) {
            summary.blender_script = Some(p.display().to_string());
        }
    }
    summary.total_millis = start.elapsed().as_millis();
    summary
}

#[derive(Clone, Debug, Serialize)]
pub struct BatchSummary {
    pub ok: usize,
    pub failed: usize,
    pub total_millis: u128,
    pub output_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blender_script: Option<String>,
    pub settings: Settings,
    pub results: Vec<Report>,
}

pub fn is_image(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Remplace chaque dossier par les images qu'il contient (sans sous-dossiers).
pub fn expand_inputs(inputs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in inputs {
        if p.is_dir() {
            if let Ok(rd) = std::fs::read_dir(p) {
                let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|f| f.is_file() && is_image(f)).collect();
                files.sort();
                out.extend(files);
            }
        } else {
            out.push(p.clone());
        }
    }
    out.dedup();
    out
}

fn safe_stem(p: &Path) -> String {
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "modele".into());
    let s: String = stem
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if s.is_empty() { "modele".into() } else { s }
}

pub fn convert_one(input: &Path, out_dir: &Path, s: &Settings) -> Report {
    let start = Instant::now();
    let mut r = Report::new(input.display().to_string());
    let result = (|| -> Result<(), String> {
        let img = image::open(input).map_err(|e| format!("lecture de l'image : {e}"))?.to_rgba8();
        let name = safe_stem(input);
        if s.mode == Mode::Multiview {
            // Chaque image est une planche (plusieurs côtés du même objet).
            return multiview_model(vec![(name.clone(), img)], &[], s, out_dir, &name, &|_| {}, &mut r);
        }
        let m = mesh::build(&img, s)?;
        write_model(&m, &img, &name, out_dir, s, s.texture_max, &mut r)
    })();
    match result {
        Ok(()) => r.ok = true,
        Err(e) => r.error = Some(e),
    }
    r.millis = start.elapsed().as_millis();
    r
}

/// Convertit toutes les images en parallèle. `on_done` est appelé à chaque image terminée.
pub fn convert_many(
    inputs: &[PathBuf],
    out_dir: &Path,
    s: &Settings,
    threads: Option<usize>,
    blender_script: bool,
    on_done: &(dyn Fn(&Report) + Sync),
) -> BatchSummary {
    let start = Instant::now();
    // Chemins absolus : le script Blender et les agents les retrouvent quel que soit le dossier courant.
    let inputs: Vec<PathBuf> = inputs.iter().map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone())).collect();
    let abs_out = std::path::absolute(out_dir).unwrap_or_else(|_| out_dir.to_path_buf());
    let out_dir = abs_out.as_path();
    let files = expand_inputs(&inputs);
    let run = || -> Vec<Report> {
        files
            .par_iter()
            .map(|f| {
                let r = convert_one(f, out_dir, s);
                on_done(&r);
                r
            })
            .collect()
    };
    let results = match threads.filter(|&t| t > 0) {
        Some(t) => match rayon::ThreadPoolBuilder::new().num_threads(t).build() {
            Ok(pool) => pool.install(run),
            Err(_) => run(),
        },
        None => run(),
    };
    let ok = results.iter().filter(|r| r.ok).count();
    let mut summary = BatchSummary {
        ok,
        failed: results.len() - ok,
        total_millis: 0,
        output_dir: out_dir.display().to_string(),
        blender_script: None,
        settings: s.clone(),
        results,
    };
    if blender_script && ok > 0 {
        match write_blender_script(out_dir, &summary.results, s.format) {
            Ok(p) => summary.blender_script = Some(p.display().to_string()),
            Err(e) => eprintln!("script Blender non écrit : {e}"),
        }
    }
    summary.total_millis = start.elapsed().as_millis();
    summary
}

/// Écrit un script Python qui importe tous les modèles dans Blender, rangés côte à côte.
/// Blender : onglet Scripting > Ouvrir > Exécuter, ou `blender --python importer_dans_blender.py`.
pub fn write_blender_script(out_dir: &Path, results: &[Report], format: Format) -> Result<PathBuf, String> {
    let files: Vec<String> = results
        .iter()
        .filter(|r| r.ok)
        .filter_map(|r| r.outputs.first().cloned())
        .collect();
    let list = serde_json::to_string_pretty(&files).map_err(|e| e.to_string())?;
    let script = format!(
        r#"# Script généré par mam3d : importe les modèles dans Blender (4.x et plus).
# Utilisation : onglet Scripting > Ouvrir ce fichier > Exécuter le script.
import bpy

FICHIERS = {list}
FORMAT = "{fmt}"
ECART = 0.5  # espace entre deux modèles, en mètres

x = 0.0
for chemin in FICHIERS:
    avant = set(bpy.context.scene.objects)
    if FORMAT == "glb":
        bpy.ops.import_scene.gltf(filepath=chemin)
    elif FORMAT == "obj":
        bpy.ops.wm.obj_import(filepath=chemin)
    else:
        bpy.ops.wm.stl_import(filepath=chemin)
    nouveaux = [o for o in bpy.context.scene.objects if o not in avant and o.parent is None]
    largeur = max((o.dimensions.x for o in nouveaux), default=1.0)
    for o in nouveaux:
        o.location.x = x + largeur / 2
    x += largeur + ECART
print("mam3d :", len(FICHIERS), "modèle(s) importé(s)")
"#,
        fmt = format.id()
    );
    let p = out_dir.join("importer_dans_blender.py");
    std::fs::write(&p, script).map_err(|e| e.to_string())?;
    Ok(p)
}
