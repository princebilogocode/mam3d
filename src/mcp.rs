//! Serveur MCP (Model Context Protocol) sur l'entrée/sortie standard.
//! Claude Code, Codex, Craft Anything ou tout client MCP peuvent piloter mam3d avec.
//!
//! Enregistrement : `claude mcp add mam3d -- <chemin>/mam3d.exe mcp`

use crate::convert;
use crate::settings::{Mode, Settings};
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;

const PROTOCOL: &str = "2025-06-18";

fn settings_schema() -> Value {
    json!({
        "mode": {"type": "string", "enum": ["inflate", "extrude", "relief", "multiview"], "description": "inflate = forme arrondie gonflée (personnages, objets) ; extrude = silhouette épaisse (logos, icônes) ; relief = luminosité en hauteur (photos, cartes de hauteur) ; multiview = chaque image est une planche de plusieurs côtés du même objet. Défaut inflate."},
        "shape": {"type": "string", "enum": ["person", "organic", "vehicle", "building"], "description": "Multi-vues : person (personne : si le dos manque, un dos est créé, cheveux derrière la tête, pas de visage), organic (animal, personnage arrondi), vehicle (voiture, moto, objet), building (bâtiment, meuble). Défaut person."},
        "format": {"type": "string", "enum": ["glb", "obj", "stl"], "description": "glb (texture incluse, recommandé pour Blender), obj (+mtl+png), stl (impression 3D). Défaut glb."},
        "resolution": {"type": "integer", "minimum": 8, "maximum": 2048, "description": "Cellules sur le plus grand côté (détail). Défaut 256."},
        "depth": {"type": "number", "description": "Épaisseur ou hauteur en mètres Blender. Défaut 0.3."},
        "size": {"type": "number", "description": "Taille du plus grand côté en mètres. Défaut 2."},
        "base": {"type": "number", "description": "Relief : épaisseur de la plaque. Défaut 0.04."},
        "invert": {"type": "boolean", "description": "Inverser la luminosité (sombre = haut)."},
        "background": {"type": "string", "enum": ["auto", "alpha", "color", "none"], "description": "Retrait du fond. Défaut auto."},
        "threshold": {"type": "number", "description": "Écart de couleur pour retirer le fond (défaut 40)."},
        "smooth": {"type": "integer", "description": "Passes de lissage (défaut 1)."},
        "detail": {"type": "number", "description": "Gonflage : détail de luminosité ajouté (0-1, défaut 0.15)."},
        "back_scale": {"type": "number", "description": "Gonflage : épaisseur du dos (0 = plat, 1 = symétrique)."},
        "texture_max": {"type": "integer", "description": "Taille max de la texture en pixels (défaut 2048)."}
    })
}

fn tools() -> Value {
    let mut convert_props = settings_schema();
    convert_props["inputs"] = json!({"type": "array", "items": {"type": "string"}, "description": "Chemins absolus d'images (png, jpg, webp, bmp, tga, gif, tiff) ou de dossiers d'images."});
    convert_props["output_dir"] = json!({"type": "string", "description": "Dossier de sortie (créé si besoin). Défaut : sous-dossier mam3d_sortie à côté de la première image."});
    convert_props["threads"] = json!({"type": "integer", "description": "Nombre de fils de calcul (défaut : tous les cœurs)."});
    convert_props["blender_script"] = json!({"type": "boolean", "description": "Écrire aussi importer_dans_blender.py (défaut true)."});
    let mut mv_props = settings_schema();
    if let Some(o) = mv_props.as_object_mut() {
        for k in ["mode", "depth", "base", "invert", "background", "smooth", "detail", "back_scale"] {
            o.remove(k);
        }
    }
    mv_props["inputs"] = json!({"type": "array", "items": {"type": "string"}, "description": "Chemins absolus : UNE planche montrant plusieurs côtés de l'objet (face, profil, dos, côte à côte sur fond uni), ou une image par côté."});
    mv_props["angles"] = json!({"type": "array", "items": {"type": "string", "enum": ["auto", "front", "side_right", "back", "side_left", "top", "ignore"]}, "description": "Côté de chaque vue, dans l'ordre (vues de la planche de gauche à droite, ou images dans l'ordre). side_right = profil où l'objet regarde vers la droite de l'image ; top = vue de dessus, avant de l'objet en bas. Omis ou auto = deviné (la première vue est la face, une vue étroite est un profil, une vue de même largeur est le dos, un trois-quarts est ignoré)."});
    mv_props["output_dir"] = json!({"type": "string", "description": "Dossier de sortie. Défaut : mam3d_sortie à côté de la première image."});
    mv_props["resolution"] = json!({"type": "integer", "minimum": 24, "maximum": 384, "description": "Détail en voxels sur la plus grande dimension. Défaut 192."});
    mv_props["size"] = json!({"type": "number", "description": "Plus grande dimension en mètres (hauteur pour une personne). Défaut 1.8."});
    mv_props["engine"] = json!({"type": "string", "enum": ["local", "tripo"], "description": "local (défaut) : gratuit, hors ligne, ~1 s, reprend les silhouettes. tripo : IA en ligne Tripo 3D, invente les parties cachées (dos, visage en relief), bien plus détaillé ; PAYANT sur le compte Tripo de la personne (~0,20-0,30 $ par modèle) et 1 à 3 minutes. N'utiliser tripo que si la personne l'a demandé. Nécessite une clé (mam3d tripo-key)."});
    mv_props["ai_model"] = json!({"type": "string", "enum": ["h3.1", "p2"], "description": "tripo : h3.1 (détail maximal, défaut) ou p2 (maillage léger en quadrilatères, pour les jeux)."});
    mv_props["ai_faces"] = json!({"type": "integer", "description": "tripo : nombre de faces visé (0 = choisi par l'IA ; p2 : 48 à 50000)."});
    mv_props["ai_detailed_texture"] = json!({"type": "boolean", "description": "tripo : texture détaillée (plus chère)."});
    mv_props["ai_pbr"] = json!({"type": "boolean", "description": "tripo : matériaux PBR (défaut true)."});
    json!([
        {
            "name": "reconstruct_multiview",
            "title": "Reconstruire un objet complet à partir de plusieurs côtés",
            "description": "Reconstruit UN objet 3D complet (personne, voiture, moto, bâtiment...) à partir d'une planche multi-vues ou d'une image par côté : silhouettes croisées, forme arrondie selon le genre d'objet, texture prise sur la photo de chaque côté. Renvoie le fichier écrit et le côté attribué à chaque vue (à corriger avec angles si besoin). Les creux invisibles sur les silhouettes (orbites, intérieur d'une voiture) ne sont pas reconstruits.",
            "inputSchema": {"type": "object", "properties": mv_props, "required": ["inputs"]}
        },
        {
            "name": "convert_images",
            "title": "Convertir des images 2D en modèles 3D",
            "description": "Transforme une ou plusieurs images 2D en modèles 3D prêts pour Blender, en parallèle. Renvoie, pour chaque image, les fichiers écrits, le nombre de sommets et triangles, la taille en mètres et le temps.",
            "inputSchema": {"type": "object", "properties": convert_props, "required": ["inputs"]}
        },
        {
            "name": "remove_background",
            "title": "Retirer l'arrière-plan",
            "description": "Détoure des photos (fond uni ou dégradé de studio relié au bord de l'image) et les enregistre en PNG transparents <nom>_sans_fond.png. Utile avant convert_images ou reconstruct_multiview, ou pour vérifier le détourage.",
            "inputSchema": {"type": "object", "properties": {
                "inputs": {"type": "array", "items": {"type": "string"}, "description": "Chemins absolus d'images ou de dossiers."},
                "output_dir": {"type": "string", "description": "Dossier de sortie. Défaut : mam3d_sortie à côté de la première image."},
                "threshold": {"type": "number", "description": "Écart de couleur objet/fond (défaut 40 ; plus bas = détourage plus serré)."},
                "keep_points": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "description": "Repères « garder » [x, y] en pixels de l'image : la zone de couleur semblable autour fait partie de l'objet (ex. un gant blanc perdu)."},
                "remove_points": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "description": "Repères « retirer » [x, y] en pixels : la zone de couleur semblable autour est du fond (ex. une ombre au sol)."},
                "radius": {"type": "number", "description": "Rayon des repères en pixels (défaut 6)."}
            }, "required": ["inputs"]}
        },
        {
            "name": "list_modes",
            "title": "Modes et formats disponibles",
            "description": "Liste les modes de conversion, les formats de sortie et les réglages par défaut de mam3d.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "inspect_image",
            "title": "Examiner une image",
            "description": "Donne la taille d'une image, si elle a de la transparence, la part de l'image retenue comme objet et le mode conseillé.",
            "inputSchema": {"type": "object", "properties": {"path": {"type": "string", "description": "Chemin absolu de l'image."}}, "required": ["path"]}
        }
    ])
}

pub fn modes_info() -> Value {
    json!({
        "modes": Mode::ALL.iter().map(|m| json!({"id": m.id(), "nom": m.label(), "usage": m.help()})).collect::<Vec<_>>(),
        "formats": ["glb", "obj", "stl"],
        "image_extensions": convert::IMAGE_EXTENSIONS,
        "defaults": Settings::default(),
    })
}

pub fn inspect(path: &str) -> Result<Value, String> {
    let img = image::open(path).map_err(|e| format!("lecture de l'image : {e}"))?.to_rgba8();
    let transparent = crate::mesh::has_transparency(&img);
    let s = Settings { mode: Mode::Extrude, resolution: 128, ..Settings::default() };
    let g = crate::mesh::make_grid(&img, &s);
    let coverage = g.mask.iter().filter(|&&m| m).count() as f32 / g.mask.len().max(1) as f32;
    let suggested = if transparent || coverage < 0.85 { "inflate" } else { "relief" };
    Ok(json!({
        "path": path,
        "width": img.width(),
        "height": img.height(),
        "transparency": transparent,
        "object_coverage": (coverage as f64 * 1000.0).round() / 1000.0,
        "suggested_mode": suggested
    }))
}

pub fn run_convert(args: &Value) -> Result<Value, String> {
    let inputs: Vec<PathBuf> = args
        .get("inputs")
        .and_then(|v| v.as_array())
        .ok_or("« inputs » (liste de chemins) est obligatoire")?
        .iter()
        .filter_map(|v| v.as_str().map(PathBuf::from))
        .collect();
    if inputs.is_empty() {
        return Err("« inputs » est vide".into());
    }
    let mut obj = args.as_object().cloned().unwrap_or_default();
    for k in ["inputs", "output_dir", "threads", "blender_script"] {
        obj.remove(k);
    }
    let s: Settings = serde_json::from_value(Value::Object(obj)).map_err(|e| format!("réglage invalide : {e}"))?;
    let out_dir = match args.get("output_dir").and_then(|v| v.as_str()) {
        Some(d) => PathBuf::from(d),
        None => default_output_dir(&inputs[0]),
    };
    let threads = args.get("threads").and_then(|v| v.as_u64()).map(|t| t as usize);
    let script = args.get("blender_script").and_then(|v| v.as_bool()).unwrap_or(true);
    let summary = convert::convert_many(&inputs, &out_dir, &s, threads, script, &|_| {});
    serde_json::to_value(summary).map_err(|e| e.to_string())
}

pub fn run_multiview(args: &Value) -> Result<Value, String> {
    let inputs: Vec<PathBuf> = args
        .get("inputs")
        .and_then(|v| v.as_array())
        .ok_or("« inputs » (liste de chemins) est obligatoire")?
        .iter()
        .filter_map(|v| v.as_str().map(PathBuf::from))
        .collect();
    if inputs.is_empty() {
        return Err("« inputs » est vide".into());
    }
    let mut angles = Vec::new();
    if let Some(list) = args.get("angles").and_then(|v| v.as_array()) {
        for a in list {
            let t = a.as_str().unwrap_or("");
            angles.push(crate::settings::ViewAngle::parse(t).ok_or(format!("côté inconnu : {t}"))?);
        }
    }
    let mut obj = args.as_object().cloned().unwrap_or_default();
    for k in ["inputs", "output_dir", "angles", "blender_script", "threads"] {
        obj.remove(k);
    }
    let mut base = serde_json::to_value(Settings { resolution: 192, size: 1.8, ..Settings::default() }).map_err(|e| e.to_string())?;
    for (k, v) in obj {
        base[k] = v;
    }
    let mut s: Settings = serde_json::from_value(base).map_err(|e| format!("réglage invalide : {e}"))?;
    s.mode = Mode::Multiview;
    s.resolution = s.resolution.clamp(24, 384);
    let out_dir = match args.get("output_dir").and_then(|v| v.as_str()) {
        Some(d) => PathBuf::from(d),
        None => default_output_dir(&inputs[0]),
    };
    let script = args.get("blender_script").and_then(|v| v.as_bool()).unwrap_or(true);
    let summary = convert::convert_views(&inputs, &out_dir, &s, &angles, script, &|m| eprintln!("{m}"));
    serde_json::to_value(summary).map_err(|e| e.to_string())
}

pub fn default_output_dir(first: &std::path::Path) -> PathBuf {
    let base = if first.is_dir() { first.to_path_buf() } else { first.parent().map(|p| p.to_path_buf()).unwrap_or_default() };
    base.join("mam3d_sortie")
}

fn handle(msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let id = id?; // notifications : pas de réponse
    let result: Result<Value, (i64, String)> = match method {
        "initialize" => {
            let version = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or(PROTOCOL).to_string();
            Ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "mam3d", "title": "mam3d - images 2D vers 3D", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "mam3d transforme des images 2D en modèles 3D (GLB/OBJ/STL) prêts pour Blender. Utilisez inspect_image pour choisir un mode, puis convert_images avec des chemins absolus ; plusieurs images sont traitées en parallèle."
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let out = match name {
                "convert_images" => run_convert(&args),
                "reconstruct_multiview" => run_multiview(&args),
                "remove_background" => (|| {
                    let inputs: Vec<PathBuf> = args
                        .get("inputs")
                        .and_then(|v| v.as_array())
                        .ok_or("« inputs » (liste de chemins) est obligatoire")?
                        .iter()
                        .filter_map(|v| v.as_str().map(PathBuf::from))
                        .collect();
                    if inputs.is_empty() {
                        return Err("« inputs » est vide".to_string());
                    }
                    let out = args.get("output_dir").and_then(|v| v.as_str()).map(PathBuf::from).unwrap_or_else(|| default_output_dir(&inputs[0]));
                    let thr = args.get("threshold").and_then(|v| v.as_f64()).unwrap_or(40.0) as f32;
                    let radius = args.get("radius").and_then(|v| v.as_f64()).unwrap_or(6.0) as f32;
                    let mut pins = Vec::new();
                    for (key, keep) in [("keep_points", true), ("remove_points", false)] {
                        for p in args.get(key).and_then(|v| v.as_array()).into_iter().flatten() {
                            let xy: Vec<f32> = p.as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).map(|v| v as f32).collect()).unwrap_or_default();
                            if xy.len() != 2 {
                                return Err(format!("« {key} » attend des paires [x, y]"));
                            }
                            pins.push(crate::cutout::Pin { x: xy[0], y: xy[1], keep, radius });
                        }
                    }
                    let reports = convert::remove_backgrounds(&inputs, &out, thr, &pins);
                    let ok = reports.iter().filter(|r| r.ok).count();
                    Ok(json!({"ok": ok, "failed": reports.len() - ok, "results": reports}))
                })(),
                "list_modes" => Ok(modes_info()),
                "inspect_image" => match args.get("path").and_then(|v| v.as_str()) {
                    Some(p) => inspect(p),
                    None => Err("« path » est obligatoire".into()),
                },
                other => return Some(error(id, -32602, &format!("outil inconnu : {other}"))),
            };
            Ok(match out {
                Ok(v) => {
                    let failed = v.get("failed").and_then(|f| f.as_u64()).unwrap_or(0);
                    let ok = v.get("ok").and_then(|f| f.as_u64());
                    let is_error = ok == Some(0) && failed > 0;
                    json!({
                        "content": [{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}],
                        "structuredContent": v,
                        "isError": is_error
                    })
                }
                Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
            })
        }
        "resources/list" => Ok(json!({"resources": []})),
        "prompts/list" => Ok(json!({"prompts": []})),
        _ => Err((-32601, format!("méthode inconnue : {method}"))),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err((code, m)) => error(id, code, &m),
    })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

pub fn serve() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(line) {
            Ok(Value::Array(batch)) => {
                let r: Vec<Value> = batch.iter().filter_map(handle).collect();
                if r.is_empty() { None } else { Some(Value::Array(r)) }
            }
            Ok(msg) => handle(&msg),
            Err(e) => Some(error(Value::Null, -32700, &format!("JSON invalide : {e}"))),
        };
        if let Some(r) = reply {
            let mut out = stdout.lock();
            let _ = writeln!(out, "{}", serde_json::to_string(&r).unwrap_or_default());
            let _ = out.flush();
        }
    }
}
