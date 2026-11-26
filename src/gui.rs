//! La fenêtre de mam3d : liste d'images, réglages, aperçu 3D et conversion en lot.

use crate::convert::{self, BatchSummary, Report};
use crate::mesh::{self, Mesh};
use crate::multiview::ViewInfo;
use crate::settings::{Background, Engine, Format, Mode, Settings, Shape, ViewAngle};
use std::collections::HashMap;
use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use image::RgbaImage;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

const ACCENT: Color32 = Color32::from_rgb(255, 122, 69);
const ACCENT_DARK: Color32 = Color32::from_rgb(196, 78, 36);
const BG: Color32 = Color32::from_rgb(22, 24, 30);
const PANEL: Color32 = Color32::from_rgb(30, 33, 41);
const OK: Color32 = Color32::from_rgb(110, 205, 130);
const ERR: Color32 = Color32::from_rgb(235, 95, 95);

#[derive(Clone)]
enum Status {
    Waiting,
    Running,
    Done(Report),
    Failed(String),
}

struct Item {
    path: PathBuf,
    status: Status,
}

enum Msg {
    Log(String),
    Done(Report),
    Finished(BatchSummary),
}

/// Fenêtre de détourage d'une image, avec ses repères (broches) garder / retirer.
struct CutEditor {
    item: usize,
    path: PathBuf,
    /// Image réduite sur laquelle on travaille (≤ 1024 px).
    small: RgbaImage,
    /// Taille réduite / taille d'origine.
    scale: f32,
    pins: Vec<crate::cutout::Pin>,
    keep_mode: bool,
    radius: f32,
    threshold: f32,
    mask: Vec<bool>,
    img_tex: egui::TextureHandle,
    overlay_tex: Option<egui::TextureHandle>,
    dirty: bool,
    millis: f32,
    open: bool,
}

impl CutEditor {
    fn recompute(&mut self, ctx: &egui::Context) {
        let t = std::time::Instant::now();
        self.mask = crate::cutout::refined_mask(&self.small, self.threshold, &self.pins);
        self.millis = t.elapsed().as_secs_f32() * 1000.0;
        let (w, h) = (self.small.width() as usize, self.small.height() as usize);
        let m = &self.mask;
        let mut px = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let edge = m[i]
                    && ((x > 0 && !m[i - 1]) || (x + 1 < w && !m[i + 1]) || (y > 0 && !m[i - w]) || (y + 1 < h && !m[i + w]));
                let c: [u8; 4] = if edge {
                    [255, 220, 40, 255]
                } else if !m[i] {
                    [90, 10, 20, 175]
                } else {
                    [0, 0, 0, 0]
                };
                px[i * 4..i * 4 + 4].copy_from_slice(&c);
            }
        }
        let color = egui::ColorImage::from_rgba_unmultiplied([w, h], &px);
        self.overlay_tex = Some(ctx.load_texture("detourage-masque", color, egui::TextureOptions::NEAREST));
        self.dirty = false;
    }
}

#[derive(PartialEq, Clone)]
struct PreviewKey {
    paths: Vec<PathBuf>,
    angles: Vec<ViewAngle>,
    settings: Settings,
}

struct Preview {
    key: PreviewKey,
    mesh: Option<Mesh>,
    /// Multi-vues : planche de texture reconstruite.
    tex: Option<egui::TextureHandle>,
    error: Option<String>,
    millis: f32,
}

pub struct App {
    items: Vec<Item>,
    selected: Option<usize>,
    settings: Settings,
    out_dir: Option<PathBuf>,
    blender_script: bool,
    // Multi-vues
    view_angles: Vec<ViewAngle>,
    view_infos: Vec<ViewInfo>,
    sheets_separate: bool,
    img_cache: HashMap<PathBuf, RgbaImage>,
    sample_sheet: RgbaImage,
    editor: Option<CutEditor>,
    // IA Tripo
    tripo_key_input: String,
    tripo_has_key: bool,
    pending_ai: Option<PathBuf>,
    ai_result: Option<(Mesh, egui::TextureHandle, String)>,
    // Aperçu
    sample: RgbaImage,
    preview_img: Option<(Option<PathBuf>, RgbaImage, egui::TextureHandle)>,
    preview: Option<Preview>,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    spin: bool,
    // Conversion
    rx: Option<mpsc::Receiver<Msg>>,
    running: bool,
    done_count: usize,
    total_count: usize,
    log: Vec<(bool, String)>,
    // Capture d'écran (--screenshot)
    screenshot: Option<PathBuf>,
    frame: u64,
    shot_rx: Option<mpsc::Receiver<Arc<egui::ColorImage>>>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, files: Vec<PathBuf>, screenshot: Option<PathBuf>) -> Self {
        style(&cc.egui_ctx);
        let mut app = App {
            items: vec![],
            selected: None,
            settings: Settings::default(),
            out_dir: None,
            blender_script: true,
            view_angles: vec![],
            view_infos: vec![],
            sheets_separate: false,
            img_cache: HashMap::new(),
            sample_sheet: sample_sheet(),
            editor: None,
            tripo_key_input: String::new(),
            tripo_has_key: crate::tripo::api_key().is_some(),
            pending_ai: None,
            ai_result: None,
            sample: sample_image(),
            preview_img: None,
            preview: None,
            yaw: -0.55,
            pitch: 0.28,
            zoom: 1.0,
            spin: true,
            rx: None,
            running: false,
            done_count: 0,
            total_count: 0,
            log: vec![],
            screenshot,
            frame: 0,
            shot_rx: None,
        };
        // La fenêtre s'ouvre en multi-vues : le mode le plus complet.
        app.settings.mode = Mode::Multiview;
        app.settings.size = 1.8;
        app.settings.resolution = 192;
        app.add_files(convert::expand_inputs(&files));
        if app.items.is_empty() {
            app.log.push((true, "Bienvenue ! Ajoutez des images (ou glissez-les dans la fenêtre). L'aperçu montre un exemple.".into()));
        }
        app
    }

    fn add_files(&mut self, files: Vec<PathBuf>) {
        let mut added = 0;
        for f in files {
            if !convert::is_image(&f) || self.items.iter().any(|i| i.path == f) {
                continue;
            }
            if self.out_dir.is_none() {
                self.out_dir = Some(crate::mcp::default_output_dir(&f));
            }
            self.items.push(Item { path: f, status: Status::Waiting });
            added += 1;
        }
        if added > 0 {
            self.view_angles.clear();
            if self.selected.is_none() {
                self.selected = Some(self.items.len() - added);
            }
            self.log.push((true, format!("{added} image(s) ajoutée(s).")));
        }
    }

    fn start_batch(&mut self, ctx: &egui::Context) {
        if self.running || self.items.is_empty() {
            return;
        }
        let out = self.out_dir.clone().unwrap_or_else(|| crate::mcp::default_output_dir(&self.items[0].path));
        self.out_dir = Some(out.clone());
        let files: Vec<PathBuf> = self.items.iter().map(|i| i.path.clone()).collect();
        for it in &mut self.items {
            it.status = Status::Running;
        }
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.running = true;
        self.done_count = 0;
        self.total_count = files.len();
        let s = self.settings.clone();
        let script = self.blender_script;
        let ctx = ctx.clone();
        if self.combined_views() {
            // Toutes les images = les côtés d'un seul objet.
            self.total_count = 1;
            let angles = self.view_angles.clone();
            let tx_log = std::sync::Mutex::new(tx.clone());
            let ctx_log = ctx.clone();
            std::thread::spawn(move || {
                let progress = move |m: String| {
                    if let Ok(t) = tx_log.lock() {
                        let _ = t.send(Msg::Log(m));
                    }
                    ctx_log.request_repaint();
                };
                let summary = convert::convert_views(&files, &out, &s, &angles, script, &progress);
                for r in &summary.results {
                    let _ = tx.send(Msg::Done(r.clone()));
                }
                let _ = tx.send(Msg::Finished(summary));
                ctx.request_repaint();
            });
            self.log.push((true, format!("Reconstruction d'un objet à partir de {} image(s)…", self.items.len())));
            return;
        }
        std::thread::spawn(move || {
            let tx2 = tx.clone();
            let ctx2 = ctx.clone();
            let summary = convert::convert_many(&files, &out, &s, None, script, &move |r: &Report| {
                let _ = tx2.send(Msg::Done(r.clone()));
                ctx2.request_repaint();
            });
            let _ = tx.send(Msg::Finished(summary));
            ctx.request_repaint();
        });
        self.log.push((true, format!("Conversion de {} image(s) sur tous les cœurs…", self.total_count)));
    }

    fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        let mut finished = None;
        while let Ok(m) = rx.try_recv() {
            match m {
                Msg::Log(l) => self.log.push((true, l)),
                Msg::Done(r) => {
                    if r.ok && self.settings.engine == Engine::Tripo && self.combined_views() {
                        self.pending_ai = r.outputs.iter().find(|o| o.ends_with("_ia_tripo_original.glb")).map(PathBuf::from);
                    }
                    self.done_count += 1;
                    let status = if r.ok { Status::Done(r.clone()) } else { Status::Failed(r.error.clone().unwrap_or_default()) };
                    let combined = r.input.contains(" + ") || !self.items.iter().any(|i| i.path.display().to_string() == r.input);
                    for it in self.items.iter_mut() {
                        if combined || it.path.display().to_string() == r.input {
                            it.status = status.clone();
                        }
                    }
                    let name = if combined {
                        r.outputs.first().map(|o| file_name(Path::new(o))).unwrap_or_else(|| "objet multi-vues".into())
                    } else {
                        file_name(Path::new(&r.input))
                    };
                    if r.ok {
                        self.log.push((true, format!("OK  {name} : {} triangles en {} ms", fmt_int(r.triangles), r.millis)));
                    } else {
                        self.log.push((false, format!("ÉCHEC  {name} : {}", r.error.unwrap_or_default())));
                    }
                }
                Msg::Finished(s) => finished = Some(s),
            }
        }
        if let Some(s) = finished {
            self.running = false;
            self.rx = None;
            let mut line = format!("Terminé : {} réussie(s), {} échec(s) en {} ms. Dossier : {}", s.ok, s.failed, s.total_millis, s.output_dir);
            if s.blender_script.is_some() {
                line.push_str(" - script Blender : importer_dans_blender.py");
            }
            self.log.push((s.failed == 0, line));
        }
    }

    fn ensure_preview(&mut self, ctx: &egui::Context) {
        let path = self.selected.and_then(|i| self.items.get(i)).map(|i| i.path.clone());
        // Image et texture de l'aperçu
        let need_img = match &self.preview_img {
            Some((p, _, _)) => *p != path,
            None => true,
        };
        let multi = self.settings.mode == Mode::Multiview;
        if need_img || self.preview_img.as_ref().map(|p| p.0.is_none() && (p.1.width() == self.sample_sheet.width()) != multi).unwrap_or(false) {
            let img = match &path {
                Some(p) => match self.load(p) {
                    Ok(i) => i,
                    Err(e) => {
                        let key = PreviewKey { paths: vec![p.clone()], angles: vec![], settings: self.settings.clone() };
                        self.preview = Some(Preview { key, mesh: None, tex: None, error: Some(e), millis: 0.0 });
                        return;
                    }
                },
                None if multi => self.sample_sheet.clone(),
                None => self.sample.clone(),
            };
            let color = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
            let tex = ctx.load_texture("apercu", color, egui::TextureOptions::LINEAR);
            self.preview_img = Some((path.clone(), img, tex));
            self.preview = None;
        }
        let mut s = self.settings.clone();
        if multi {
            // Seuls ces réglages changent la forme : l'aperçu ne se recalcule pas pour les autres.
            s = Settings { mode: Mode::Multiview, shape: s.shape, threshold: s.threshold, resolution: s.resolution.min(110), ..Settings::default() };
        } else {
            s.resolution = s.resolution.min(160);
        }
        let paths: Vec<PathBuf> = if multi && !self.sheets_separate {
            self.items.iter().map(|i| i.path.clone()).collect()
        } else {
            path.iter().cloned().collect()
        };
        let angles = if self.combined_views() { self.view_angles.clone() } else { vec![] };
        let key = PreviewKey { paths: paths.clone(), angles: angles.clone(), settings: s.clone() };
        if self.preview.as_ref().map(|p| p.key != key).unwrap_or(true) {
            let t = std::time::Instant::now();
            if self.preview.as_ref().map(|p| p.key.paths != key.paths || p.key.angles != key.angles).unwrap_or(false) {
                // Autres images : l'ancien résultat de l'IA ne correspond plus.
                self.ai_result = None;
            }
            if multi {
                let mut images = Vec::new();
                let mut err = None;
                if paths.is_empty() {
                    images.push(("exemple".to_string(), self.sample_sheet.clone()));
                }
                for p in &paths {
                    match self.load(p) {
                        Ok(i) => images.push((file_name(p), i)),
                        Err(e) => err = Some(e),
                    }
                }
                let r = match err {
                    Some(e) => Err(e),
                    None => crate::multiview::from_images(images, &angles, &s),
                };
                let millis = t.elapsed().as_secs_f32() * 1000.0;
                self.preview = Some(match r {
                    Ok(rec) => {
                        self.view_infos = rec.views.clone();
                        let tx = shrink(rec.texture, 2048);
                        let color = egui::ColorImage::from_rgba_unmultiplied([tx.width() as usize, tx.height() as usize], tx.as_raw());
                        let tex = ctx.load_texture("apercu-multivues", color, egui::TextureOptions::LINEAR);
                        Preview { key, mesh: Some(rec.mesh), tex: Some(tex), error: None, millis }
                    }
                    Err(e) => Preview { key, mesh: None, tex: None, error: Some(e), millis },
                });
            } else {
                let img = &self.preview_img.as_ref().unwrap().1;
                let r = mesh::build(img, &s);
                let millis = t.elapsed().as_secs_f32() * 1000.0;
                self.preview = Some(match r {
                    Ok(m) => Preview { key, mesh: Some(m), tex: None, error: None, millis },
                    Err(e) => Preview { key, mesh: None, tex: None, error: Some(e), millis },
                });
            }
        }
    }

    /// Détoure toutes les images de la liste et les remplace par leur version transparente.
    fn remove_backgrounds(&mut self) {
        let out = self.out_dir.clone().unwrap_or_else(|| crate::mcp::default_output_dir(&self.items[0].path));
        self.out_dir = Some(out.clone());
        let paths: Vec<PathBuf> = self.items.iter().map(|i| i.path.clone()).collect();
        let t = std::time::Instant::now();
        let reports = convert::remove_backgrounds(&paths, &out, self.settings.threshold, &[]);
        let mut done = 0;
        for r in reports {
            let name = file_name(Path::new(&r.input));
            match (&r.error, r.outputs.first()) {
                (None, Some(o)) => {
                    let newp = PathBuf::from(o);
                    if let Some(it) = self.items.iter_mut().find(|i| std::path::absolute(&i.path).unwrap_or_else(|_| i.path.clone()).display().to_string() == r.input) {
                        it.path = newp.clone();
                        it.status = Status::Waiting;
                    }
                    self.img_cache.remove(&newp);
                    done += 1;
                }
                (e, _) => self.log.push((false, format!("ÉCHEC  {name} : {}", e.clone().unwrap_or_default()))),
            }
        }
        self.preview_img = None;
        self.preview = None;
        self.view_angles.clear();
        self.log.push((true, format!("Arrière-plan retiré sur {done} image(s) en {} ms (fichiers « _sans_fond.png » dans {}).", t.elapsed().as_millis(), out.display())));
    }

    fn open_editor(&mut self, ctx: &egui::Context) {
        let Some(idx) = self.selected.filter(|&i| i < self.items.len()) else { return };
        let path = self.items[idx].path.clone();
        let full = match image::open(&path) {
            Ok(i) => i.to_rgba8(),
            Err(e) => {
                self.log.push((false, format!("Image illisible : {e}")));
                return;
            }
        };
        let small = shrink(full.clone(), 1024);
        let scale = small.width() as f32 / full.width().max(1) as f32;
        let color = egui::ColorImage::from_rgba_unmultiplied([small.width() as usize, small.height() as usize], small.as_raw());
        let img_tex = ctx.load_texture("detourage-image", color, egui::TextureOptions::LINEAR);
        let mut ed = CutEditor {
            item: idx,
            path,
            small,
            scale,
            pins: vec![],
            keep_mode: false,
            radius: 6.0,
            threshold: self.settings.threshold,
            mask: vec![],
            img_tex,
            overlay_tex: None,
            dirty: true,
            millis: 0.0,
            open: true,
        };
        ed.recompute(ctx);
        self.editor = Some(ed);
    }

    /// Atelier de détourage : contour en jaune, fond retiré en rouge, repères à poser.
    fn editor_window(&mut self, ctx: &egui::Context) {
        let Some(ed) = self.editor.as_mut() else { return };
        let mut apply = false;
        let mut cancel = false;
        let title = format!("Détourage - {}", file_name(&ed.path));
        egui::Window::new(title)
            .open(&mut ed.open)
            .default_size([980.0, 760.0])
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Repère :").strong());
                    let keep = egui::Button::new(RichText::new("Garder").color(Color32::WHITE)).fill(if ed.keep_mode { Color32::from_rgb(40, 150, 70) } else { Color32::from_rgb(45, 49, 60) });
                    if ui.add(keep).on_hover_text("Clic ou trait sur une partie de l'objet perdue (gant blanc, cheveux…) : elle revient").clicked() {
                        ed.keep_mode = true;
                    }
                    let rem = egui::Button::new(RichText::new("Retirer").color(Color32::WHITE)).fill(if !ed.keep_mode { Color32::from_rgb(180, 50, 50) } else { Color32::from_rgb(45, 49, 60) });
                    if ui.add(rem).on_hover_text("Clic ou trait sur un reste de fond (ombre, sol, décor) : il part").clicked() {
                        ed.keep_mode = false;
                    }
                    ui.separator();
                    ui.label("Pinceau");
                    ui.add(egui::Slider::new(&mut ed.radius, 1.0..=40.0).suffix(" px"));
                    ui.label("Seuil");
                    if ui.add(egui::Slider::new(&mut ed.threshold, 5.0..=150.0)).on_hover_text("Écart de couleur entre l'objet et le fond ; plus bas = contour plus serré").drag_stopped() {
                        ed.dirty = true;
                    }
                    ui.separator();
                    if ui.add_enabled(!ed.pins.is_empty(), egui::Button::new("Annuler le dernier")).clicked() {
                        ed.pins.pop();
                        ed.dirty = true;
                    }
                    if ui.add_enabled(!ed.pins.is_empty(), egui::Button::new("Effacer les repères")).clicked() {
                        ed.pins.clear();
                        ed.dirty = true;
                    }
                });
                ui.label(
                    RichText::new(format!(
                        "Rouge = fond retiré · jaune = contour · {} repère(s) · calculé en {:.0} ms. Cliquez ou tracez sur l'image pour poser des repères.",
                        ed.pins.len(),
                        ed.millis
                    ))
                    .small()
                    .color(Color32::GRAY),
                );
                let avail = ui.available_size() - Vec2::new(0.0, 46.0);
                let (iw, ih) = (ed.small.width() as f32, ed.small.height() as f32);
                let k = (avail.x / iw).min(avail.y.max(100.0) / ih);
                let size = Vec2::new(iw * k, ih * k);
                let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
                let painter = ui.painter_at(rect);
                // Damier : on voit ce qui sera transparent.
                let cell = 12.0;
                let mut y = rect.top();
                let mut row = 0;
                while y < rect.bottom() {
                    let mut x = rect.left() + if row % 2 == 0 { 0.0 } else { cell };
                    while x < rect.right() {
                        painter.rect_filled(Rect::from_min_size(Pos2::new(x, y), Vec2::splat(cell)).intersect(rect), 0, Color32::from_gray(70));
                        x += cell * 2.0;
                    }
                    y += cell;
                    row += 1;
                }
                let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                painter.image(ed.img_tex.id(), rect, uv, Color32::WHITE);
                if let Some(o) = &ed.overlay_tex {
                    painter.image(o.id(), rect, uv, Color32::WHITE);
                }
                for p in &ed.pins {
                    let c = rect.min + Vec2::new(p.x * k, p.y * k);
                    let col = if p.keep { Color32::from_rgb(60, 210, 100) } else { Color32::from_rgb(235, 70, 70) };
                    painter.circle(c, (p.radius * k).max(4.0), col.gamma_multiply(0.45), Stroke::new(1.5, Color32::WHITE));
                }
                // Repères : clic, ou trait (un repère tous les « pinceau » pixels).
                if let Some(pos) = resp.interact_pointer_pos() {
                    if resp.clicked() || resp.dragged() {
                        let p = (pos - rect.min) / k;
                        if p.x >= 0.0 && p.y >= 0.0 && p.x < iw && p.y < ih {
                            let far = ed.pins.last().map(|l| l.keep != ed.keep_mode || ((l.x - p.x).powi(2) + (l.y - p.y).powi(2)).sqrt() >= ed.radius).unwrap_or(true);
                            if resp.clicked() || resp.drag_started() || far {
                                ed.pins.push(crate::cutout::Pin { x: p.x, y: p.y, keep: ed.keep_mode, radius: ed.radius });
                            }
                        }
                    }
                }
                if resp.clicked() || resp.drag_stopped() {
                    ed.dirty = true;
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let ok = egui::Button::new(RichText::new("Appliquer et enregistrer").strong().color(Color32::WHITE)).fill(ACCENT).min_size(Vec2::new(220.0, 32.0));
                    if ui.add(ok).on_hover_text("Détoure l'image en pleine taille avec ces repères, l'enregistre en « _sans_fond.png » et la met dans la liste à la place de l'originale").clicked() {
                        apply = true;
                    }
                    if ui.button("Fermer sans changer").clicked() {
                        cancel = true;
                    }
                });
            });
        if ed.dirty {
            ed.recompute(ctx);
        }
        if !ed.open || cancel {
            self.editor = None;
            return;
        }
        if apply {
            let ed = self.editor.take().unwrap();
            let out = self.out_dir.clone().unwrap_or_else(|| crate::mcp::default_output_dir(&ed.path));
            self.out_dir = Some(out.clone());
            let inv = 1.0 / ed.scale.max(1e-6);
            let pins: Vec<crate::cutout::Pin> = ed.pins.iter().map(|p| crate::cutout::Pin { x: p.x * inv, y: p.y * inv, keep: p.keep, radius: p.radius * inv }).collect();
            let reports = convert::remove_backgrounds(&[ed.path.clone()], &out, ed.threshold, &pins);
            for r in reports {
                match (&r.error, r.outputs.first()) {
                    (None, Some(o)) => {
                        let newp = PathBuf::from(o);
                        if let Some(it) = self.items.get_mut(ed.item) {
                            it.path = newp.clone();
                            it.status = Status::Waiting;
                        }
                        self.img_cache.remove(&newp);
                        self.settings.threshold = ed.threshold;
                        self.log.push((true, format!("Détourage enregistré ({} repère(s)) : {}", pins.len(), newp.display())));
                    }
                    (e, _) => self.log.push((false, format!("ÉCHEC détourage : {}", e.clone().unwrap_or_default()))),
                }
            }
            self.preview_img = None;
            self.preview = None;
            self.view_angles.clear();
        }
    }

    /// Choix du moteur : local, ou IA en ligne Tripo 3D (clé, modèle, faces, texture).
    fn engine_panel(&mut self, ui: &mut egui::Ui) {
        section(ui, "MOTEUR");
        let s = &mut self.settings;
        for e in [Engine::Local, Engine::Tripo] {
            ui.radio_value(&mut s.engine, e, e.label()).on_hover_text(match e {
                Engine::Local => "Sculpte le volume à partir des silhouettes, sur votre ordinateur, en une seconde.",
                Engine::Tripo => "Envoie les vues détourées à l'IA de Tripo 3D, qui invente les parties cachées (dos, relief du visage, plis) : bien plus détaillé, 1 à 3 minutes, environ 0,20 à 0,30 $ par modèle sur votre compte Tripo.",
            });
        }
        if s.engine != Engine::Tripo {
            return;
        }
        egui::Grid::new("ia").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Modèle IA");
            egui::ComboBox::from_id_salt("ia-modele")
                .selected_text(if s.ai_model == "p2" { "P2 - léger, en quads (jeux)" } else { "H3.1 - détail maximal" })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.ai_model, "h3.1".to_string(), "H3.1 - détail maximal");
                    ui.selectable_value(&mut s.ai_model, "p2".to_string(), "P2 - léger, en quads (jeux)");
                });
            ui.end_row();
            ui.label("Faces");
            let max = if s.ai_model == "p2" { 50_000 } else { 500_000 };
            ui.add(egui::Slider::new(&mut s.ai_faces, 0..=max).logarithmic(true).custom_formatter(|v, _| if v < 1.0 { "auto".into() } else { format!("{v:.0}") }))
                .on_hover_text("Nombre de faces visé ; « auto » laisse l'IA choisir. Moins de faces = plus léger dans un jeu.");
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut s.ai_detailed_texture, "Texture détaillée (plus chère)");
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut s.ai_pbr, "Matériaux PBR (métal, rugosité)");
            ui.end_row();
        });
        if self.tripo_has_key {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Clé Tripo enregistrée").color(OK));
                if ui.small_button("Oublier la clé").clicked() {
                    crate::tripo::forget_key();
                    self.tripo_has_key = crate::tripo::api_key().is_some();
                }
            });
        } else {
            ui.label(RichText::new("Clé Tripo (créez-la sur platform.tripo3d.ai, rubrique API Keys) :").small().color(Color32::GRAY));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.tripo_key_input).password(true).hint_text("tsk_...").desired_width(190.0));
                if ui.add_enabled(!self.tripo_key_input.trim().is_empty(), egui::Button::new("Enregistrer")).clicked() {
                    match crate::tripo::save_key(&self.tripo_key_input) {
                        Ok(()) => {
                            self.log.push((true, "Clé Tripo enregistrée sur cet ordinateur.".into()));
                            self.tripo_has_key = true;
                        }
                        Err(e) => self.log.push((false, format!("Clé non enregistrée : {e}"))),
                    }
                    self.tripo_key_input.clear();
                }
            });
        }
        ui.label(RichText::new("L'aperçu montre le moteur local ; le résultat de l'IA s'affiche ici une fois reçu.").small().color(Color32::GRAY));
    }

    /// Charge le modèle reçu de l'IA pour l'aperçu.
    fn load_ai_result(&mut self, ctx: &egui::Context) {
        let Some(p) = self.pending_ai.take() else { return };
        match std::fs::read(&p).map_err(|e| e.to_string()).and_then(|b| crate::glb::read(&b)) {
            Ok((mut m, tex)) => {
                crate::glb::fit(&mut m, self.settings.size);
                let img = shrink(tex.unwrap_or_else(|| RgbaImage::from_pixel(4, 4, image::Rgba([200, 200, 200, 255]))), 2048);
                let color = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                let t = ctx.load_texture("resultat-ia", color, egui::TextureOptions::LINEAR);
                self.ai_result = Some((m, t, file_name(&p)));
            }
            Err(e) => self.log.push((false, format!("Aperçu du modèle IA impossible : {e} (le fichier est bien enregistré)"))),
        }
    }

    /// Image réduite pour l'aperçu, gardée en mémoire.
    fn load(&mut self, p: &Path) -> Result<RgbaImage, String> {
        if let Some(i) = self.img_cache.get(p) {
            return Ok(i.clone());
        }
        let i = image::open(p).map_err(|e| format!("Image illisible : {e}"))?;
        let i = shrink(i.to_rgba8(), 1024);
        self.img_cache.insert(p.to_path_buf(), i.clone());
        Ok(i)
    }

    /// Multi-vues avec toutes les images comme côtés d'un seul objet.
    fn combined_views(&self) -> bool {
        self.settings.mode == Mode::Multiview && !(self.sheets_separate && self.items.len() > 1)
    }

    /// Liste des vues trouvées, avec le côté de chacune (modifiable).
    fn views_panel(&mut self, ui: &mut egui::Ui) {
        section(ui, "CÔTÉS DE L'OBJET");
        if self.items.len() > 1 {
            ui.checkbox(&mut self.sheets_separate, "Chaque image est une planche séparée")
                .on_hover_text("Coché : chaque image contient tous les côtés d'un objet différent (traitement en lot).\nDécoché : chaque image montre un côté du même objet.");
        }
        if !self.combined_views() {
            ui.label(RichText::new("Chaque planche est découpée et ses côtés devinés automatiquement.").small().color(Color32::GRAY));
            return;
        }
        if self.view_infos.is_empty() {
            ui.label(RichText::new("Les vues apparaîtront ici.").small().color(Color32::GRAY));
            return;
        }
        if self.view_angles.len() != self.view_infos.len() {
            self.view_angles.resize(self.view_infos.len(), ViewAngle::Auto);
        }
        egui::Grid::new("vues").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            for (i, info) in self.view_infos.iter().enumerate() {
                if info.generated {
                    ui.label("Dos");
                    ui.label(RichText::new("créé (personne : cheveux, sans visage)").color(OK))
                        .on_hover_text("Aucune photo de dos : mam3d fabrique le dos à partir de la face. Ajoutez une photo de dos pour un résultat exact, ou choisissez un autre genre d'objet.");
                    ui.end_row();
                    continue;
                }
                let found = ViewAngle::parse(&info.angle).unwrap_or(ViewAngle::Ignore);
                ui.label(format!("Vue {}", i + 1)).on_hover_text(&info.source);
                let a = &mut self.view_angles[i];
                let text = if *a == ViewAngle::Auto { format!("Auto : {}", found.label()) } else { a.label().to_string() };
                egui::ComboBox::from_id_salt(("angle", i)).width(200.0).selected_text(text).show_ui(ui, |ui| {
                    for v in ViewAngle::ALL {
                        ui.selectable_value(a, v, v.label());
                    }
                });
                ui.end_row();
            }
        });
        ui.label(
            RichText::new("Fond uni conseillé. Les vues trois-quarts sont ignorées ; « profil (regarde à droite) » = l'objet regarde vers la droite de l'image.")
                .small()
                .color(Color32::GRAY),
        );
    }

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        let reserve = if self.running { 84.0 } else { 56.0 };
        egui::ScrollArea::vertical().auto_shrink([false, false]).max_height((ui.available_height() - reserve).max(100.0)).show(ui, |ui| {
            section(ui, "IMAGES");
            ui.horizontal(|ui| {
                if ui.button("+ Images…").on_hover_text("Ajouter des images").clicked() {
                    if let Some(files) = rfd::FileDialog::new().add_filter("Images", &convert::IMAGE_EXTENSIONS).pick_files() {
                        self.add_files(files);
                    }
                }
                if ui.button("Dossier…").on_hover_text("Ajouter toutes les images d'un dossier").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.add_files(convert::expand_inputs(&[dir]));
                    }
                }
                if ui.add_enabled(!self.items.is_empty() && !self.running, egui::Button::new("Vider")).clicked() {
                    self.items.clear();
                    self.selected = None;
                    self.view_angles.clear();
                }
            });
            ui.add_space(4.0);
            if self.items.is_empty() {
                egui::Frame::new().fill(BG).corner_radius(8).inner_margin(12).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("Glissez des images ici\n(PNG, JPG, WebP, BMP, TGA, GIF, TIFF)").color(Color32::GRAY));
                });
            }
            let mut remove = None;
            for (idx, it) in self.items.iter().enumerate() {
                let sel = self.selected == Some(idx);
                let color = match &it.status {
                    Status::Waiting => Color32::from_gray(90),
                    Status::Running => ACCENT,
                    Status::Done(_) => OK,
                    Status::Failed(_) => ERR,
                };
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::new(10.0, 16.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 4.5, color);
                    let r = ui.selectable_label(sel, file_name(&it.path));
                    let r = match &it.status {
                        Status::Done(rep) => r.on_hover_text(format!("{} triangles, {} ms\n{}", fmt_int(rep.triangles), rep.millis, rep.outputs.join("\n"))),
                        Status::Failed(e) => r.on_hover_text(e),
                        _ => r.on_hover_text(it.path.display().to_string()),
                    };
                    if r.clicked() {
                        self.selected = Some(idx);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !self.running && ui.small_button("x").on_hover_text("Retirer").clicked() {
                            remove = Some(idx);
                        }
                    });
                });
            }
            if let Some(i) = remove {
                self.items.remove(i);
                self.view_angles.clear();
                self.selected = if self.items.is_empty() { None } else { Some(i.min(self.items.len() - 1)) };
            }
            if !self.items.is_empty() {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let half = (ui.available_width() - 8.0) / 2.0;
                    let btn = egui::Button::new(RichText::new("Détourer…").strong()).min_size(Vec2::new(half, 30.0));
                    if ui
                        .add_enabled(!self.running, btn)
                        .on_hover_text("Ouvre l'image choisie dans l'atelier de détourage : vérifiez le contour et posez des repères verts (garder) ou rouges (retirer).")
                        .clicked()
                    {
                        let ctx = ui.ctx().clone();
                        self.open_editor(&ctx);
                    }
                    let btn = egui::Button::new("Tout détourer (auto)").min_size(Vec2::new(half, 30.0));
                    if ui
                        .add_enabled(!self.running, btn)
                        .on_hover_text("Détoure toutes les images d'un coup, sans repères, et les remplace par leur version transparente « _sans_fond.png ».")
                        .clicked()
                    {
                        self.remove_backgrounds();
                    }
                });
            }

            ui.add_space(10.0);
            section(ui, "TRANSFORMATION");
            for m in Mode::ALL {
                let r = ui.radio_value(&mut self.settings.mode, m, RichText::new(m.label()).strong());
                r.on_hover_text(m.help());
            }
            ui.label(RichText::new(self.settings.mode.help()).small().color(Color32::GRAY));
            ui.add_space(6.0);

            let multi = self.settings.mode == Mode::Multiview;
            if multi {
                self.views_panel(ui);
                ui.add_space(6.0);
                let s = &mut self.settings;
                egui::Grid::new("reglages-mv").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label("Genre d'objet");
                    egui::ComboBox::from_id_salt("genre").selected_text(s.shape.label()).show_ui(ui, |ui| {
                        for sh in Shape::ALL {
                            ui.selectable_value(&mut s.shape, sh, sh.label());
                        }
                    });
                    ui.end_row();
                    ui.label("Détail");
                    ui.add(egui::Slider::new(&mut s.resolution, 48..=384).suffix(" voxels"))
                        .on_hover_text("Finesse du volume. L'aperçu est limité à 110 ; l'export utilise la valeur choisie.");
                    ui.end_row();
                    ui.label("Taille");
                    ui.add(egui::Slider::new(&mut s.size, 0.1..=50.0).logarithmic(true).suffix(" m"))
                        .on_hover_text("Plus grande dimension dans Blender (la hauteur pour une personne : 1,8 m)");
                    ui.end_row();
                    ui.label("Seuil du fond");
                    ui.add(egui::Slider::new(&mut s.threshold, 1.0..=200.0)).on_hover_text("Écart de couleur entre l'objet et le fond uni");
                    ui.end_row();
                });
                ui.add_space(6.0);
                self.engine_panel(ui);
            }

            let s = &mut self.settings;
            if !multi {
            egui::Grid::new("reglages").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label("Détail");
                ui.add(egui::Slider::new(&mut s.resolution, 16..=1024).logarithmic(true).suffix(" cellules"))
                    .on_hover_text("Plus de cellules = plus de détails et un fichier plus lourd. L'aperçu est limité à 160.");
                ui.end_row();
                ui.label(if s.mode == Mode::Relief { "Hauteur" } else { "Épaisseur" });
                ui.add(egui::Slider::new(&mut s.depth, 0.0..=2.0).suffix(" m"));
                ui.end_row();
                ui.label("Taille");
                ui.add(egui::Slider::new(&mut s.size, 0.1..=20.0).logarithmic(true).suffix(" m")).on_hover_text("Taille du plus grand côté dans Blender");
                ui.end_row();
                if s.mode == Mode::Relief {
                    ui.label("Plaque");
                    ui.add(egui::Slider::new(&mut s.base, 0.0..=0.5).suffix(" m"));
                    ui.end_row();
                }
                if s.mode == Mode::Inflate {
                    ui.label("Dos");
                    ui.add(egui::Slider::new(&mut s.back_scale, 0.0..=1.0)).on_hover_text("0 = dos plat (à poser contre un mur), 1 = symétrique");
                    ui.end_row();
                    ui.label("Relief ajouté");
                    ui.add(egui::Slider::new(&mut s.detail, 0.0..=1.0)).on_hover_text("Ajoute les détails clairs/sombres de l'image à la forme gonflée");
                    ui.end_row();
                }
                ui.label("Lissage");
                ui.add(egui::Slider::new(&mut s.smooth, 0..=8));
                ui.end_row();
                ui.label("Fond");
                egui::ComboBox::from_id_salt("fond").selected_text(s.background.label()).show_ui(ui, |ui| {
                    for b in Background::ALL {
                        ui.selectable_value(&mut s.background, b, b.label());
                    }
                });
                ui.end_row();
                if matches!(s.background, Background::Color | Background::Auto) && s.mode != Mode::Relief {
                    ui.label("Seuil du fond");
                    ui.add(egui::Slider::new(&mut s.threshold, 1.0..=200.0));
                    ui.end_row();
                }
                ui.label("");
                ui.checkbox(&mut s.invert, "Inverser clair / sombre");
                ui.end_row();
            });
            }

            ui.add_space(10.0);
            section(ui, "EXPORT POUR BLENDER");
            egui::ComboBox::from_id_salt("format").width(230.0).selected_text(self.settings.format.label()).show_ui(ui, |ui| {
                for f in Format::ALL {
                    ui.selectable_value(&mut self.settings.format, f, f.label());
                }
            });
            ui.checkbox(&mut self.blender_script, "Créer le script d'import Blender").on_hover_text("importer_dans_blender.py : importe tous les modèles d'un coup, rangés côte à côte");
            ui.horizontal(|ui| {
                ui.label("Dossier :");
                let txt = self.out_dir.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "à côté des images (mam3d_sortie)".into());
                ui.label(RichText::new(short(&txt, 34)).color(Color32::LIGHT_GRAY)).on_hover_text(txt);
            });
            ui.horizontal(|ui| {
                if ui.button("Choisir…").clicked() {
                    if let Some(d) = rfd::FileDialog::new().pick_folder() {
                        self.out_dir = Some(d);
                    }
                }
                if let Some(d) = &self.out_dir {
                    if ui.button("Ouvrir le dossier").clicked() {
                        let _ = std::fs::create_dir_all(d);
                        open_folder(d);
                    }
                }
            });
        });
        ui.add_space(8.0);
        {
            let n = self.items.len();
            let label = if self.running {
                format!("Conversion… {}/{}", self.done_count, self.total_count)
            } else if n == 0 {
                "Ajoutez des images pour convertir".to_string()
            } else if self.combined_views() && self.settings.engine == Engine::Tripo {
                "Reconstruire avec l'IA Tripo".to_string()
            } else if self.combined_views() {
                "Reconstruire l'objet en 3D".to_string()
            } else {
                format!("Convertir {n} image(s) en 3D")
            };
            let btn = egui::Button::new(RichText::new(label).size(16.0).strong().color(Color32::WHITE))
                .fill(if n > 0 && !self.running { ACCENT } else { ACCENT_DARK })
                .corner_radius(8)
                .min_size(Vec2::new(ui.available_width(), 40.0));
            if ui.add_enabled(n > 0 && !self.running, btn).clicked() {
                let ctx = ui.ctx().clone();
                self.start_batch(&ctx);
            }
            if self.running && self.total_count > 0 {
                ui.add(egui::ProgressBar::new(self.done_count as f32 / self.total_count as f32).show_percentage());
            }
        }
    }

    fn bottom_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Journal").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "mam3d".into());
                if ui.button("Copier la commande Codex").on_hover_text("Ligne à ajouter dans ~/.codex/config.toml").clicked() {
                    ui.ctx().copy_text(format!("[mcp_servers.mam3d]\ncommand = '{exe}'\nargs = [\"mcp\"]"));
                }
                if ui.button("Copier la commande Claude Code").on_hover_text("Enregistre mam3d comme outil MCP de Claude Code").clicked() {
                    ui.ctx().copy_text(format!("claude mcp add mam3d -- \"{exe}\" mcp"));
                }
                ui.label(RichText::new("Pilotable par Claude Code, Codex et Craft Anything (MCP + ligne de commande)").color(Color32::GRAY));
            });
        });
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
            for (ok, l) in self.log.iter().rev().take(200).collect::<Vec<_>>().into_iter().rev() {
                ui.label(RichText::new(l).monospace().size(12.0).color(if *ok { Color32::LIGHT_GRAY } else { ERR }));
            }
        });
    }

    fn viewer(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        if resp.dragged() {
            let d = resp.drag_delta();
            self.yaw += d.x * 0.01;
            self.pitch = (self.pitch + d.y * 0.01).clamp(-1.5, 1.5);
            self.spin = false;
        }
        if resp.double_clicked() {
            self.yaw = -0.55;
            self.pitch = 0.28;
            self.zoom = 1.0;
            self.spin = true;
        }
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.zoom = (self.zoom * (1.0 + scroll * 0.002)).clamp(0.2, 6.0);
            }
        }
        if self.spin {
            let dt = ui.input(|i| i.stable_dt).min(0.05);
            self.yaw += dt * 0.5;
            ui.ctx().request_repaint();
        }
        let painter = ui.painter_at(rect);
        // Fond en dégradé
        let mut bg = egui::Mesh::default();
        let top = Color32::from_rgb(44, 48, 60);
        let bot = Color32::from_rgb(16, 17, 22);
        bg.colored_vertex(rect.left_top(), top);
        bg.colored_vertex(rect.right_top(), top);
        bg.colored_vertex(rect.right_bottom(), bot);
        bg.colored_vertex(rect.left_bottom(), bot);
        bg.add_triangle(0, 1, 2);
        bg.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(bg));
        // Ombre au sol
        let c = rect.center();
        let rad = rect.width().min(rect.height()) * 0.33 * self.zoom.min(1.6);
        for k in 0..6 {
            let f = 1.0 - k as f32 / 6.0;
            painter.add(egui::Shape::ellipse_filled(
                Pos2::new(c.x, c.y + rad * 1.05),
                Vec2::new(rad * f, rad * 0.16 * f),
                Color32::from_black_alpha(18),
            ));
        }

        let title = match self.selected.and_then(|i| self.items.get(i)) {
            Some(it) => file_name(&it.path),
            None => "Exemple (ajoutez vos images)".to_string(),
        };
        let title = if self.settings.mode == Mode::Multiview && self.combined_views() {
            if self.items.is_empty() { "Exemple de planche multi-vues (ajoutez la vôtre)".to_string() } else { format!("Objet reconstruit à partir de {} vue(s)", self.view_infos.iter().filter(|v| v.angle != "ignore").count()) }
        } else {
            title
        };
        let ai_shown = self.settings.mode == Mode::Multiview && self.settings.engine == Engine::Tripo && self.ai_result.is_some();
        if let Some((m, t, name)) = self.ai_result.as_ref().filter(|_| ai_shown) {
            draw_mesh(&painter, rect, m, t.id(), self.yaw, self.pitch, self.zoom);
            let info = format!("Résultat de l'IA Tripo 3D · {} · {} triangles", name, fmt_int(m.triangle_count()));
            painter.text(rect.left_top() + Vec2::new(14.0, 12.0), egui::Align2::LEFT_TOP, info, egui::FontId::proportional(14.0), Color32::from_gray(220));
        } else if let (Some(p), Some((_, _, tex))) = (&self.preview, &self.preview_img) {
            if let Some(m) = &p.mesh {
                let tid = p.tex.as_ref().map(|t| t.id()).unwrap_or(tex.id());
                draw_mesh(&painter, rect, m, tid, self.yaw, self.pitch, self.zoom);
                let info = format!(
                    "{} · {} · {} triangles · calculé en {:.0} ms",
                    title,
                    self.settings.mode.label(),
                    fmt_int(m.triangle_count()),
                    p.millis
                );
                painter.text(rect.left_top() + Vec2::new(14.0, 12.0), egui::Align2::LEFT_TOP, info, egui::FontId::proportional(14.0), Color32::from_gray(220));
            }
            if let Some(e) = &p.error {
                painter.text(rect.center(), egui::Align2::CENTER_CENTER, e, egui::FontId::proportional(15.0), ERR);
            }
        }
        // Vignette de l'image d'origine
        if let Some((_, img, tex)) = &self.preview_img {
            let w = if img.width() > img.height() * 2 { 220.0 } else { 110.0 };
            let h = w * img.height() as f32 / img.width().max(1) as f32;
            let r = Rect::from_min_size(rect.right_top() + Vec2::new(-w - 14.0, 12.0), Vec2::new(w, h.min(140.0)));
            painter.rect_filled(r.expand(4.0), 6, Color32::from_black_alpha(120));
            painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            painter.text(r.center_bottom() + Vec2::new(0.0, 8.0), egui::Align2::CENTER_TOP, "image 2D", egui::FontId::proportional(11.0), Color32::GRAY);
        }
        painter.text(
            rect.center_bottom() + Vec2::new(0.0, -10.0),
            egui::Align2::CENTER_BOTTOM,
            "Glisser pour tourner · molette pour zoomer · double-clic pour recentrer",
            egui::FontId::proportional(12.0),
            Color32::from_gray(140),
        );
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot.clone() else { return };
        ctx.request_repaint();
        self.frame += 1;
        if self.frame == 120 {
            let (tx, rx) = mpsc::channel();
            self.shot_rx = Some(rx);
            let c2 = ctx.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                let _ = tx.send(img);
                c2.request_repaint();
            })));
        }
        if let Some(rx) = &self.shot_rx {
            if let Ok(img) = rx.try_recv() {
                let [w, h] = img.size;
                let mut buf = Vec::with_capacity(w * h * 4);
                for p in &img.pixels {
                    buf.extend_from_slice(&p.to_srgba_unmultiplied());
                }
                if let Some(rgba) = RgbaImage::from_raw(w as u32, h as u32, buf) {
                    if let Some(dir) = path.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    match rgba.save(&path) {
                        Ok(()) => println!("Capture enregistrée : {}", path.display()),
                        Err(e) => eprintln!("Capture impossible : {e}"),
                    }
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        if self.frame > 1200 {
            eprintln!("Capture : aucune image reçue");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_screenshot(&ctx);
        self.poll();
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if !dropped.is_empty() {
            self.add_files(convert::expand_inputs(&dropped));
        }
        self.ensure_preview(&ctx);
        self.load_ai_result(&ctx);

        egui::Panel::top("haut")
            .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(16, 10)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("mam").size(26.0).strong().color(Color32::WHITE));
                    ui.add_space(-8.0);
                    ui.label(RichText::new("3d").size(26.0).strong().color(ACCENT));
                    ui.add_space(12.0);
                    ui.label(RichText::new("Images 2D en modèles 3D prêts pour Blender").size(15.0).color(Color32::GRAY));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
                        ui.label(RichText::new(format!("{cores} cœurs · Rust")).color(Color32::GRAY));
                    });
                });
            });
        egui::Panel::bottom("journal")
            .resizable(true)
            .default_size(90.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(12, 8)))
            .show(ui, |ui| self.bottom_panel(ui));
        egui::Panel::left("gauche")
            .resizable(true)
            .default_size(330.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(14, 12)))
            .show(ui, |ui| self.left_panel(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| self.viewer(ui));
        // Test : MAM3D_DETOURAGE=1 ouvre l'atelier de détourage sur la première image.
        if self.frame == 2 && self.editor.is_none() && std::env::var("MAM3D_DETOURAGE").is_ok() && !self.items.is_empty() {
            self.selected = Some(0);
            self.open_editor(&ctx);
            if let Some(ed) = self.editor.as_mut() {
                ed.pins.push(crate::cutout::Pin { x: ed.small.width() as f32 * 0.7, y: ed.small.height() as f32 * 0.5, keep: false, radius: 8.0 });
                ed.dirty = true;
            }
        }
        self.editor_window(&ctx);
    }
}

fn style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.selection.bg_fill = ACCENT_DARK;
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    v.widgets.inactive.corner_radius = CornerRadius::same(6);
    v.widgets.hovered.corner_radius = CornerRadius::same(6);
    v.widgets.active.corner_radius = CornerRadius::same(6);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(45, 49, 60);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(58, 63, 77);
    v.slider_trailing_fill = true;
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.button_padding = Vec2::new(10.0, 5.0);
    });
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).size(12.0).strong().color(ACCENT));
    ui.add_space(2.0);
}

/// Dessine le maillage texturé, éclairé, vu par une caméra en perspective.
fn draw_mesh(painter: &egui::Painter, rect: Rect, m: &Mesh, tex: egui::TextureId, yaw: f32, pitch: f32, zoom: f32) {
    let (min, max) = m.bounds();
    let center = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5, (min[2] + max[2]) * 0.5];
    let extent = (max[0] - min[0]).max(max[1] - min[1]).max(max[2] - min[2]).max(1e-6);
    let scale = zoom * rect.width().min(rect.height()) * 0.72 / extent;
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let rot = |p: [f32; 3]| -> [f32; 3] {
        let x = p[0] * cy + p[2] * sy;
        let z = -p[0] * sy + p[2] * cy;
        let y = p[1] * cp - z * sp;
        let z2 = p[1] * sp + z * cp;
        [x, y, z2]
    };
    let focal = extent * 3.5;
    let c = rect.center();
    let light = {
        let l = [-0.45f32, 0.6, 0.75];
        let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
        [l[0] / n, l[1] / n, l[2] / n]
    };
    let mut proj: Vec<(f32, f32, f32)> = Vec::with_capacity(m.positions.len());
    let mut out = egui::Mesh::with_texture(tex);
    out.vertices.reserve(m.positions.len());
    for (i, p) in m.positions.iter().enumerate() {
        let q = rot([p[0] - center[0], p[1] - center[1], p[2] - center[2]]);
        let persp = focal / (focal - q[2]).max(extent * 0.1);
        let sx = c.x + q[0] * scale * persp;
        let syy = c.y - q[1] * scale * persp;
        proj.push((sx, syy, q[2]));
        let n = rot(m.normals[i]);
        let diff = (n[0] * light[0] + n[1] * light[1] + n[2] * light[2]).max(0.0);
        let rim = (1.0 - n[2].abs()).powi(3) * 0.25;
        let shade = (0.32 + 0.68 * diff + rim).min(1.0);
        let g = (shade * 255.0) as u8;
        out.vertices.push(egui::epaint::Vertex { pos: Pos2::new(sx, syy), uv: Pos2::new(m.uvs[i][0], m.uvs[i][1]), color: Color32::from_rgb(g, g, g) });
    }
    let mut tris: Vec<(f32, [u32; 3])> = Vec::with_capacity(m.indices.len() / 3);
    for t in m.indices.chunks_exact(3) {
        let (a, b, cc) = (proj[t[0] as usize], proj[t[1] as usize], proj[t[2] as usize]);
        // Face visible si elle tourne dans le sens direct à l'écran (y vers le bas => aire négative).
        let area = (b.0 - a.0) * (cc.1 - a.1) - (b.1 - a.1) * (cc.0 - a.0);
        if area >= 0.0 {
            continue;
        }
        tris.push((a.2 + b.2 + cc.2, [t[0], t[1], t[2]]));
    }
    tris.sort_unstable_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    out.indices.reserve(tris.len() * 3);
    for (_, t) in tris {
        out.indices.extend_from_slice(&t);
    }
    painter.add(egui::Shape::mesh(out));
}

/// Image d'exemple : un cœur coloré sur fond transparent.
fn sample_image() -> RgbaImage {
    let n = 384u32;
    RgbaImage::from_fn(n, n, |px, py| {
        let x = (px as f32 / n as f32 - 0.5) * 2.6;
        let y = -(py as f32 / n as f32 - 0.47) * 2.6;
        let a = x * x + y * y - 1.0;
        let inside = a * a * a - x * x * y * y * y < 0.0;
        if !inside {
            return image::Rgba([0, 0, 0, 0]);
        }
        let t = ((y + 1.1) / 2.4).clamp(0.0, 1.0);
        let mut r = 230.0 + 25.0 * t;
        let mut g = 60.0 + 90.0 * t;
        let mut b = 110.0 - 60.0 * t;
        let stripe = ((x * 7.0 + y * 3.0).sin() * 0.5 + 0.5) * 18.0;
        r -= stripe;
        g -= stripe;
        b -= stripe * 0.5;
        let hl = ((x + 0.45).powi(2) + (y - 0.45).powi(2)).sqrt();
        if hl < 0.22 {
            let k = 1.0 - hl / 0.22;
            r += (255.0 - r) * k * 0.8;
            g += (255.0 - g) * k * 0.8;
            b += (255.0 - b) * k * 0.8;
        }
        image::Rgba([r.clamp(0.0, 255.0) as u8, g.clamp(0.0, 255.0) as u8, b.clamp(0.0, 255.0) as u8, 255])
    })
}

/// Planche d'exemple : un personnage simple vu de face, de profil et de dos, sur fond blanc.
fn sample_sheet() -> RgbaImage {
    let skin = image::Rgba([196, 140, 104, 255]);
    let hair = image::Rgba([52, 36, 30, 255]);
    let shirt = image::Rgba([64, 118, 196, 255]);
    let pants = image::Rgba([48, 52, 66, 255]);
    let shoe = image::Rgba([24, 24, 28, 255]);
    let eye = image::Rgba([30, 30, 34, 255]);
    let white = image::Rgba([255, 255, 255, 255]);
    RgbaImage::from_fn(540, 360, |px, py| {
        let (x, y) = (px as f32, py as f32);
        let circle = |cx: f32, cy: f32, r: f32| (x - cx).powi(2) + (y - cy).powi(2) < r * r;
        let rect = |x0: f32, x1: f32, y0: f32, y1: f32| x >= x0 && x < x1 && y >= y0 && y < y1;
        // Face (centre 95) et dos (centre 445)
        for (cx, back) in [(95.0f32, false), (445.0, true)] {
            if circle(cx, 44.0, 24.0) {
                if back || y < 30.0 {
                    return hair;
                }
                if circle(cx - 9.0, 44.0, 3.2) || circle(cx + 9.0, 44.0, 3.2) || rect(cx - 7.0, cx + 7.0, 56.0, 59.0) {
                    return eye;
                }
                return skin;
            }
            if rect(cx - 9.0, cx + 9.0, 64.0, 78.0) {
                return skin;
            }
            if rect(cx - 38.0, cx + 38.0, 76.0, 196.0) {
                return shirt;
            }
            for sx in [-1.0f32, 1.0] {
                let ax = cx + sx * 47.0;
                if rect(ax - 9.0, ax + 9.0, 80.0, 180.0) {
                    return shirt;
                }
                if circle(ax, 188.0, 10.0) {
                    return skin;
                }
                let lx = cx + sx * 17.0;
                if rect(lx - 15.0, lx + 15.0, 196.0, 320.0) {
                    return pants;
                }
                if rect(lx - 16.0, lx + 16.0, 320.0, 336.0) {
                    return shoe;
                }
            }
        }
        // Profil (centre 270), regarde vers la droite
        let cx = 270.0;
        if circle(cx, 44.0, 24.0) || (circle(cx + 24.0, 48.0, 6.0) && x > cx) {
            if (y < 32.0 && x < cx + 14.0) || x < cx - 6.0 {
                return hair;
            }
            if circle(cx + 12.0, 42.0, 3.0) {
                return eye;
            }
            return skin;
        }
        if rect(cx - 8.0, cx + 10.0, 64.0, 78.0) {
            return skin;
        }
        if rect(cx - 22.0, cx + 24.0, 76.0, 196.0) {
            return shirt;
        }
        if circle(cx + 2.0, 188.0, 10.0) {
            return skin;
        }
        if rect(cx - 18.0, cx + 18.0, 196.0, 320.0) {
            return pants;
        }
        if rect(cx - 18.0, cx + 30.0, 320.0, 336.0) {
            return shoe;
        }
        white
    })
}

fn shrink(img: RgbaImage, max: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    if w.max(h) <= max {
        return img;
    }
    let s = max as f32 / w.max(h) as f32;
    image::imageops::resize(&img, ((w as f32 * s) as u32).max(1), ((h as f32 * s) as u32).max(1), image::imageops::FilterType::Triangle)
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| p.display().to_string())
}

fn short(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= n {
        s.to_string()
    } else {
        format!("…{}", chars[chars.len() - n..].iter().collect::<String>())
    }
}

fn fmt_int(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

fn open_folder(d: &Path) {
    #[cfg(windows)]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(d).spawn();
}
