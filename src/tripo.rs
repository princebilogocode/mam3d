//! Moteur IA en ligne : Tripo 3D (API v3, https://platform.tripo3d.ai).
//!
//! Les vues sont découpées et détourées par mam3d, envoyées à Tripo (une vue : image vers 3D ;
//! plusieurs : face / gauche / dos / droite), puis le modèle rendu est téléchargé. Tripo invente
//! les parties cachées (dos, creux du visage...) avec son IA. Payant sur le compte Tripo de la
//! personne (de l'ordre de 0,20 à 0,30 $ par modèle) ; la clé n'est jamais affichée ni journalisée.

use crate::multiview::{self, SourceView, ViewInfo};
use crate::settings::{Settings, ViewAngle};
use image::RgbaImage;
use serde_json::{Value, json};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const API: &str = "https://openapi.tripo3d.ai";
/// Modèles acceptés par l'API (noms exacts).
pub const MODEL_H31: &str = "v3.1-20260211";
pub const MODEL_P2: &str = "P2-20260801";

fn key_file() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("mam3d").join("tripo_key.txt"))
}

/// La clé : variable TRIPO_API_KEY, sinon celle enregistrée par la fenêtre.
pub fn api_key() -> Option<String> {
    if let Ok(k) = std::env::var("TRIPO_API_KEY") {
        if !k.trim().is_empty() {
            return Some(k.trim().to_string());
        }
    }
    let k = std::fs::read_to_string(key_file()?).ok()?;
    let k = k.trim().to_string();
    if k.is_empty() { None } else { Some(k) }
}

pub fn save_key(k: &str) -> Result<(), String> {
    let p = key_file().ok_or("dossier de réglages introuvable")?;
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&p, k.trim()).map_err(|e| e.to_string())
}

pub fn forget_key() {
    if let Some(p) = key_file() {
        let _ = std::fs::remove_file(p);
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(300)))
        .user_agent(concat!("mam3d/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Lit l'enveloppe { code, message, data } de l'API.
fn envelope(mut resp: ureq::http::Response<ureq::Body>) -> Result<Value, String> {
    let status = resp.status().as_u16();
    let v: Value = resp.body_mut().read_json().map_err(|_| format!("réponse illisible de Tripo (HTTP {status})"))?;
    let code = v["code"].as_i64().unwrap_or(-1);
    if status >= 400 || code != 0 {
        let msg = v["message"].as_str().unwrap_or("erreur inconnue");
        let hint = match (status, code) {
            (401, _) | (_, 1002) => " - clé Tripo refusée : vérifiez-la",
            (_, 2010) => " - crédits Tripo insuffisants : rechargez votre compte",
            (429, _) => " - trop de demandes, réessayez dans un instant",
            _ => "",
        };
        return Err(format!("Tripo : {msg} (HTTP {status}, code {code}){hint}"));
    }
    Ok(v["data"].clone())
}

fn upload_png(a: &ureq::Agent, key: &str, name: &str, png: &[u8]) -> Result<String, String> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let boundary = format!("mam3d{:x}", nanos ^ png.len() as u128);
    let mut body = Vec::with_capacity(png.len() + 300);
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: image/png\r\n\r\n").as_bytes());
    body.extend_from_slice(png);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let resp = a
        .post(format!("{API}/v3/files"))
        .header("Authorization", &format!("Bearer {key}"))
        .header("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .send(&body[..])
        .map_err(|e| format!("envoi de l'image impossible : {e}"))?;
    let d = envelope(resp)?;
    d["file_token"].as_str().map(|s| s.to_string()).ok_or_else(|| "Tripo n'a pas rendu de jeton de fichier".into())
}

/// Vue détourée, recadrée sur l'objet (fond transparent), en PNG.
fn view_png(v: &SourceView) -> Result<Vec<u8>, String> {
    let [x0, y0, x1, y1] = v.bbox;
    let (iw, ih) = v.img.dimensions();
    let m = ((x1 - x0).max(y1 - y0) / 12).max(8);
    let (cx0, cy0, cx1, cy1) = (x0.saturating_sub(m), y0.saturating_sub(m), (x1 + m).min(iw), (y1 + m).min(ih));
    let crop = RgbaImage::from_fn(cx1 - cx0, cy1 - cy0, |x, y| {
        let (sx, sy) = (cx0 + x, cy0 + y);
        let inside = sx >= x0 && sx < x1 && sy >= y0 && sy < y1 && v.mask[(sy * iw + sx) as usize];
        let mut p = *v.img.get_pixel(sx, sy);
        p.0[3] = if inside { 255 } else { 0 };
        p
    });
    let mut out = Vec::new();
    crop.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out)
}

pub struct AiResult {
    pub mesh: crate::mesh::Mesh,
    pub texture: Option<RgbaImage>,
    pub original_glb: PathBuf,
    pub views: Vec<ViewInfo>,
    pub task_id: String,
}

/// Reconstruit avec l'IA Tripo. `progress` reçoit des messages lisibles (jamais la clé).
pub fn reconstruct(images: Vec<(String, RgbaImage)>, angles: &[ViewAngle], s: &Settings, out_dir: &Path, name: &str, progress: &(dyn Fn(String) + Sync)) -> Result<AiResult, String> {
    let key = api_key().ok_or("aucune clé Tripo : enregistrez-la dans la fenêtre (moteur IA) ou définissez TRIPO_API_KEY")?;
    let mut views = multiview::prepare(images, angles, s.threshold)?;
    let guessed = multiview::classify(&mut views);
    let infos = multiview::view_infos(&views, &guessed);
    let slot = |a: ViewAngle| match a {
        ViewAngle::Front => Some("front"),
        ViewAngle::SideLeft => Some("left"),
        ViewAngle::Back => Some("back"),
        ViewAngle::SideRight => Some("right"),
        _ => None,
    };
    let used: Vec<(&'static str, &SourceView)> = views.iter().filter_map(|v| slot(v.angle).map(|s| (s, v))).collect();
    if !used.iter().any(|(s, _)| *s == "front") {
        return Err("il faut une vue de face pour l'IA".into());
    }
    let a = agent();
    let mut tokens: Vec<(&str, String)> = Vec::new();
    for (i, (slot, v)) in used.iter().enumerate() {
        progress(format!("Envoi de la vue {} ({slot}) à Tripo…", i + 1));
        let png = view_png(v)?;
        tokens.push((slot, upload_png(&a, &key, &format!("{name}_{slot}.png"), &png)?));
    }

    let model = if s.ai_model == "p2" { MODEL_P2 } else { MODEL_H31 };
    let mut payload = json!({
        "model": model,
        "texture": true,
        "pbr": s.ai_pbr,
        "texture_quality": if s.ai_detailed_texture { "detailed" } else { "standard" },
    });
    if s.ai_faces > 0 {
        let max = if model == MODEL_P2 { 50_000 } else { 2_000_000 };
        payload["face_limit"] = json!(s.ai_faces.clamp(48, max));
    }
    let endpoint = if tokens.len() == 1 {
        payload["input"] = json!(tokens[0].1);
        "/v3/generation/image-to-model"
    } else {
        let order = ["front", "left", "back", "right"];
        let inputs: Vec<Value> = order
            .iter()
            .filter_map(|o| tokens.iter().find(|(s, _)| s == o).map(|(s, t)| json!({ *s: t })))
            .collect();
        payload["inputs"] = json!(inputs);
        "/v3/generation/multiview-to-model"
    };
    progress(format!("Création du modèle par l'IA ({} vue(s), modèle {})…", tokens.len(), if model == MODEL_P2 { "P2" } else { "H3.1" }));
    let resp = a
        .post(format!("{API}{endpoint}"))
        .header("Authorization", &format!("Bearer {key}"))
        .send_json(&payload)
        .map_err(|e| format!("Tripo injoignable : {e}"))?;
    let task_id = envelope(resp)?["task_id"].as_str().ok_or("Tripo n'a pas rendu de numéro de tâche")?.to_string();

    // Attente du résultat.
    let start = Instant::now();
    let task = loop {
        std::thread::sleep(Duration::from_secs(3));
        let resp = a
            .get(format!("{API}/v3/tasks/{task_id}"))
            .header("Authorization", &format!("Bearer {key}"))
            .call()
            .map_err(|e| format!("Tripo injoignable : {e}"))?;
        let t = envelope(resp)?;
        let status = t["status"].as_str().unwrap_or("");
        match status {
            "success" => break t,
            "failed" | "cancelled" | "banned" | "expired" => {
                let why = t["error_msg"].as_str().or(t["message"].as_str()).unwrap_or("");
                return Err(format!("l'IA a échoué ({status}) {why}"));
            }
            _ => {}
        }
        if start.elapsed() > Duration::from_secs(20 * 60) {
            return Err(format!("toujours en cours après 20 minutes (tâche {task_id})"));
        }
        let pct = t["progress"].as_u64().map(|p| format!(" {p} %")).unwrap_or_default();
        progress(format!("IA en cours :{pct} ({} s)", start.elapsed().as_secs()));
    };

    // URL du modèle : PBR si demandé, sinon le modèle, sinon le premier .glb trouvé.
    let mut urls: Vec<(String, String)> = Vec::new();
    collect_urls(&task["output"], "", &mut urls);
    let pick = |want: &str| urls.iter().find(|(k, _)| k.ends_with(want)).map(|(_, u)| u.clone());
    let url = (if s.ai_pbr { pick("pbr_model") } else { None })
        .or_else(|| pick("model"))
        .or_else(|| urls.iter().find(|(_, u)| u.split('?').next().unwrap_or("").ends_with(".glb")).map(|(_, u)| u.clone()))
        .ok_or("Tripo n'a pas rendu de fichier de modèle")?;
    progress("Téléchargement du modèle…".into());
    let mut resp = a.get(&url).call().map_err(|e| format!("téléchargement impossible : {e}"))?;
    if resp.status().as_u16() >= 400 {
        return Err(format!("téléchargement refusé (HTTP {})", resp.status().as_u16()));
    }
    let bytes = resp.body_mut().with_config().limit(1 << 30).read_to_vec().map_err(|e| format!("téléchargement interrompu : {e}"))?;
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let original = out_dir.join(format!("{name}_ia_tripo_original.glb"));
    std::fs::write(&original, &bytes).map_err(|e| e.to_string())?;
    let (mut mesh, texture) = crate::glb::read(&bytes).map_err(|e| format!("modèle reçu ({}) mais illisible : {e}", original.display()))?;
    crate::glb::fit(&mut mesh, s.size);
    Ok(AiResult { mesh, texture, original_glb: original, views: infos, task_id })
}

fn collect_urls(v: &Value, path: &str, out: &mut Vec<(String, String)>) {
    match v {
        Value::String(s) if s.starts_with("http") => out.push((path.to_string(), s.clone())),
        Value::Object(m) => {
            for (k, x) in m {
                let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                let p = p.trim_end_matches(".url").trim_end_matches("_url").to_string();
                collect_urls(x, &p, out);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                collect_urls(x, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}
