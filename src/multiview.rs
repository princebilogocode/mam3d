//! Reconstruction multi-vues : plusieurs côtés d'un même objet (face, profil, dos, dessus)
//! deviennent un volume complet.
//!
//! 1. Chaque vue est détourée (le fond touchant le bord de l'image est retiré) ; une planche
//!    unique est découpée en vues séparées par les colonnes vides.
//! 2. Un volume de voxels est sculpté : on ne garde que ce qui tombe dans la silhouette de
//!    chaque vue (enveloppe visuelle), arrondi selon le genre d'objet (personne, véhicule...).
//! 3. La surface est extraite (surface nets), lissée, puis texturée : chaque triangle prend la
//!    couleur de la photo qui le voit le mieux, en tenant compte de ce qui le cache.

use crate::mesh::{Mesh, quad_facing};
use crate::settings::{Settings, Shape, ViewAngle};
use image::{Rgba, RgbaImage};
use rayon::prelude::*;
use serde::Serialize;
use std::sync::Arc;

#[derive(Clone)]
pub struct SourceView {
    pub name: String,
    pub img: Arc<RgbaImage>,
    pub mask: Arc<Vec<bool>>,
    /// x0, y0, x1, y1 (x1/y1 exclus)
    pub bbox: [u32; 4],
    pub angle: ViewAngle,
}

impl SourceView {
    fn w(&self) -> f32 {
        (self.bbox[2] - self.bbox[0]) as f32
    }
    fn h(&self) -> f32 {
        (self.bbox[3] - self.bbox[1]) as f32
    }
    /// Le point (en pixels) tombe-t-il dans la silhouette ? (1 pixel de tolérance)
    fn inside(&self, px: f32, py: f32) -> bool {
        let x = px.floor() as i64;
        let y = py.floor() as i64;
        let w = self.img.width() as i64;
        for (dx, dy) in [(0i64, 0i64), (1, 0), (-1, 0), (0, 1), (0, -1)] {
            let (xx, yy) = (x + dx, y + dy);
            if xx >= self.bbox[0] as i64
                && xx < self.bbox[2] as i64
                && yy >= self.bbox[1] as i64
                && yy < self.bbox[3] as i64
                && self.mask[(yy * w + xx) as usize]
            {
                return true;
            }
        }
        false
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ViewInfo {
    pub source: String,
    /// Côté utilisé (front, side_right, back, side_left, top, ignore).
    pub angle: String,
    /// Le côté a-t-il été deviné automatiquement ?
    pub guessed: bool,
    pub width: u32,
    pub height: u32,
    /// Vue fabriquée par mam3d (dos d'une personne), pas une photo fournie.
    pub generated: bool,
}

/// Détoure l'objet : transparence si l'image en a, sinon tout ce qui n'est pas relié au bord
/// par la couleur du fond (les zones claires à l'intérieur, comme une chemise blanche, restent).
pub fn object_mask(img: &RgbaImage, threshold: f32) -> Vec<bool> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let n = w * h;
    let raw = img.as_raw();
    let mut fg = vec![false; n];
    if w == 0 || h == 0 {
        return fg;
    }
    if crate::mesh::has_transparency(img) {
        for (i, f) in fg.iter_mut().enumerate() {
            *f = raw[i * 4 + 3] >= 128;
        }
    } else {
        let bg = background_flood(img, threshold);
        for i in 0..n {
            fg[i] = !bg[i];
        }
        open_mask(&mut fg, w, h);
    }
    remove_small(&mut fg, w, h);
    fg
}

/// Fond relié au bord de l'image, trouvé de proche en proche : la couleur de référence suit
/// le fond au fil de son parcours, si bien qu'un fond de studio dégradé, un vignettage ou une
/// ombre douce sont retirés, alors que le saut de couleur au bord de la personne l'arrête.
pub fn background_flood(img: &RgbaImage, threshold: f32) -> Vec<bool> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let n = w * h;
    let raw = img.as_raw();
    let col = |i: usize| [raw[i * 4] as f32, raw[i * 4 + 1] as f32, raw[i * 4 + 2] as f32];
    let dist = |a: [f32; 3], b: [f32; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
    let mut seen = vec![false; n];
    if n == 0 {
        return seen;
    }
    let mut border = Vec::with_capacity(2 * (w + h));
    for x in 0..w {
        border.push(x);
        border.push((h - 1) * w + x);
    }
    for y in 0..h {
        border.push(y * w);
        border.push(y * w + w - 1);
    }
    // Couleur typique du bord ; les graines doivent en être assez proches (pas la personne
    // qui toucherait le bord de l'image).
    let mut med = [0f32; 3];
    for (k, m) in med.iter_mut().enumerate() {
        let mut ch: Vec<u8> = border.iter().map(|&i| raw[i * 4 + k]).collect();
        ch.sort_unstable();
        *m = ch[ch.len() / 2] as f32;
    }
    let thr = threshold.max(1.0);
    let mut refc = vec![[0f32; 3]; n];
    let mut queue = std::collections::VecDeque::new();
    for &i in &border {
        if !seen[i] && dist(col(i), med) <= thr * 2.5 {
            seen[i] = true;
            refc[i] = col(i);
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        let (x, y) = (i % w, i / w);
        let r = refc[i];
        let mut nb = [usize::MAX; 4];
        if x > 0 {
            nb[0] = i - 1;
        }
        if x + 1 < w {
            nb[1] = i + 1;
        }
        if y > 0 {
            nb[2] = i - w;
        }
        if y + 1 < h {
            nb[3] = i + w;
        }
        for j in nb {
            if j == usize::MAX || seen[j] {
                continue;
            }
            let c = col(j);
            // Pas de saut brusque, et pas trop loin du fond rencontré jusque-là.
            if dist(c, r) <= thr * 0.6 {
                seen[j] = true;
                refc[j] = [r[0] * 0.8 + c[0] * 0.2, r[1] * 0.8 + c[1] * 0.2, r[2] * 0.8 + c[2] * 0.2];
                queue.push_back(j);
            }
        }
    }
    seen
}

/// Ouverture du masque (érosion puis dilatation d'un pixel) : enlève le liseré du fond
/// resté accroché au contour et les fils d'un pixel.
fn open_mask(fg: &mut [bool], w: usize, h: usize) {
    let at = |m: &[bool], x: i64, y: i64| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && m[y as usize * w + x as usize];
    let src = fg.to_vec();
    let mut er = vec![false; w * h];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            er[y as usize * w + x as usize] = at(&src, x, y) && at(&src, x - 1, y) && at(&src, x + 1, y) && at(&src, x, y - 1) && at(&src, x, y + 1);
        }
    }
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            fg[y as usize * w + x as usize] = at(&er, x, y) || at(&er, x - 1, y) || at(&er, x + 1, y) || at(&er, x, y - 1) || at(&er, x, y + 1);
        }
    }
}

/// Retire les taches (ombres, poussières) bien plus petites que les morceaux principaux.
fn remove_small(fg: &mut [bool], w: usize, h: usize) {
    let mut label = vec![u32::MAX; w * h];
    let mut areas: Vec<usize> = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if !fg[start] || label[start] != u32::MAX {
            continue;
        }
        let id = areas.len() as u32;
        let mut area = 0;
        label[start] = id;
        stack.push(start);
        while let Some(i) = stack.pop() {
            area += 1;
            let (x, y) = (i % w, i / w);
            let mut nb = [usize::MAX; 4];
            if x > 0 {
                nb[0] = i - 1;
            }
            if x + 1 < w {
                nb[1] = i + 1;
            }
            if y > 0 {
                nb[2] = i - w;
            }
            if y + 1 < h {
                nb[3] = i + w;
            }
            for j in nb {
                if j != usize::MAX && fg[j] && label[j] == u32::MAX {
                    label[j] = id;
                    stack.push(j);
                }
            }
        }
        areas.push(area);
    }
    let largest = areas.iter().copied().max().unwrap_or(0);
    let min_area = (largest / 100).max(30);
    for i in 0..w * h {
        if fg[i] && areas[label[i] as usize] < min_area {
            fg[i] = false;
        }
    }
}

/// Découpe une planche en vues : groupes de colonnes contenant l'objet, séparés par du vide.
pub fn find_views(mask: &[bool], w: usize, h: usize) -> Vec<[u32; 4]> {
    if w == 0 || h == 0 {
        return vec![];
    }
    let cols: Vec<bool> = (0..w).map(|x| (0..h).any(|y| mask[y * w + x])).collect();
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut x = 0;
    while x < w {
        if cols[x] {
            let s = x;
            while x < w && cols[x] {
                x += 1;
            }
            runs.push((s, x));
        } else {
            x += 1;
        }
    }
    // Fusionne les écarts minuscules, ignore les bandes trop étroites.
    let gap = ((w as f32) * 0.012) as usize;
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for r in runs {
        match merged.last_mut() {
            Some(last) if r.0 - last.1 <= gap => last.1 = r.1,
            _ => merged.push(r),
        }
    }
    let widest = merged.iter().map(|r| r.1 - r.0).max().unwrap_or(0);
    merged
        .into_iter()
        .filter(|r| r.1 - r.0 >= (widest / 8).max(4))
        .filter_map(|(x0, x1)| {
            let rows: Vec<usize> = (0..h).filter(|&y| (x0..x1).any(|x| mask[y * w + x])).collect();
            Some([x0 as u32, *rows.first()? as u32, x1 as u32, *rows.last()? as u32 + 1])
        })
        .collect()
}

/// Prépare les vues : une seule image = une planche découpée ; plusieurs images = une vue chacune.
/// `angles[i]` force le côté de la i-ème vue (Auto = deviné).
pub fn prepare(images: Vec<(String, RgbaImage)>, angles: &[ViewAngle], threshold: f32) -> Result<Vec<SourceView>, String> {
    let single = images.len() == 1;
    let mut views = Vec::new();
    for (name, img) in images {
        let (w, h) = (img.width() as usize, img.height() as usize);
        let mask = crate::cutout::refined_mask(&img, threshold, &[]);
        let boxes = find_views(&mask, w, h);
        if boxes.is_empty() {
            return Err(format!("{name} : aucun objet détecté (fond trop proche de l'objet ? baissez le seuil)"));
        }
        let img = Arc::new(img);
        let mask = Arc::new(mask);
        if single && boxes.len() > 1 {
            for (k, b) in boxes.into_iter().enumerate() {
                views.push(SourceView { name: format!("{name} - vue {}", k + 1), img: img.clone(), mask: mask.clone(), bbox: b, angle: ViewAngle::Auto });
            }
        } else {
            let mut u = boxes[0];
            for b in &boxes[1..] {
                u = [u[0].min(b[0]), u[1].min(b[1]), u[2].max(b[2]), u[3].max(b[3])];
            }
            views.push(SourceView { name, img, mask, bbox: u, angle: ViewAngle::Auto });
        }
    }
    for (i, v) in views.iter_mut().enumerate() {
        v.angle = angles.get(i).copied().unwrap_or(ViewAngle::Auto);
    }
    Ok(views)
}

/// Devine le côté des vues « Auto » d'après la forme de leur silhouette.
/// Renvoie, pour chaque vue, si elle a été devinée.
pub fn classify(views: &mut [SourceView]) -> Vec<bool> {
    let guessed: Vec<bool> = views.iter().map(|v| v.angle == ViewAngle::Auto).collect();
    let has = |vs: &[SourceView], a: ViewAngle| vs.iter().any(|v| v.angle == a);
    // Référence : la face donnée, sinon la première vue automatique devient la face.
    if !has(views, ViewAngle::Front) {
        if let Some(v) = views.iter_mut().find(|v| v.angle == ViewAngle::Auto) {
            v.angle = ViewAngle::Front;
        }
    }
    let Some(front) = views.iter().find(|v| v.angle == ViewAngle::Front) else { return guessed };
    let r0 = front.w() / front.h().max(1.0);
    for i in 0..views.len() {
        if views[i].angle != ViewAngle::Auto {
            continue;
        }
        let r = (views[i].w() / views[i].h().max(1.0)) / r0.max(1e-3);
        let a = if !(0.62..=1.6).contains(&r) {
            if !has(views, ViewAngle::SideRight) {
                ViewAngle::SideRight
            } else if !has(views, ViewAngle::SideLeft) {
                ViewAngle::SideLeft
            } else {
                ViewAngle::Ignore
            }
        } else if (0.85..=1.18).contains(&r) && !has(views, ViewAngle::Back) {
            ViewAngle::Back
        } else {
            // Trois-quarts ou angle inconnu : on ne s'en sert pas pour sculpter.
            ViewAngle::Ignore
        };
        views[i].angle = a;
    }
    guessed
}

/// Comment une vue projette un point de l'objet sur son image.
#[derive(Clone, Copy)]
struct Proj {
    view: usize,
    top: bool,
    right: [f32; 3],
    /// Direction de l'objet vers l'appareil photo.
    cam: [f32; 3],
    s: f32,
    cx: f32,
    base: f32,
}

impl Proj {
    fn project(&self, p: [f32; 3]) -> (f32, f32) {
        if self.top {
            (self.cx + p[0] * self.s, self.base + p[2] * self.s)
        } else {
            let u = p[0] * self.right[0] + p[2] * self.right[2];
            (self.cx + u * self.s, self.base - p[1] * self.s)
        }
    }
}

fn side_proj(view: usize, v: &SourceView, theta_deg: f32) -> Proj {
    let t = theta_deg.to_radians();
    let (st, ct) = t.sin_cos();
    Proj {
        view,
        top: false,
        right: [ct, 0.0, st],
        cam: [-st, 0.0, ct],
        s: v.h().max(1.0),
        cx: (v.bbox[0] + v.bbox[2]) as f32 * 0.5,
        base: v.bbox[3] as f32,
    }
}

pub struct Reconstruction {
    pub mesh: Mesh,
    pub texture: RgbaImage,
    pub views: Vec<ViewInfo>,
}

pub fn view_infos(views: &[SourceView], guessed: &[bool]) -> Vec<ViewInfo> {
    views
        .iter()
        .enumerate()
        .map(|(i, v)| ViewInfo {
            source: v.name.clone(),
            angle: v.angle.id().to_string(),
            guessed: guessed.get(i).copied().unwrap_or(false),
            width: v.bbox[2] - v.bbox[0],
            height: v.bbox[3] - v.bbox[1],
            generated: false,
        })
        .collect()
}

/// Reconstruit l'objet à partir d'images (une planche, ou une image par côté).
pub fn from_images(images: Vec<(String, RgbaImage)>, angles: &[ViewAngle], s: &Settings) -> Result<Reconstruction, String> {
    let mut views = prepare(images, angles, s.threshold)?;
    let guessed = classify(&mut views);
    let mut infos = view_infos(&views, &guessed);
    // Une personne sans photo de dos : on fabrique un dos (sinon le visage apparaît derrière).
    if s.shape == Shape::Person && !views.iter().any(|v| v.angle == ViewAngle::Back) {
        if let Some(front) = views.iter().find(|v| v.angle == ViewAngle::Front) {
            let back = synth_person_back(front);
            infos.push(ViewInfo {
                source: "dos créé automatiquement (personne)".into(),
                angle: "back".into(),
                guessed: true,
                width: back.bbox[2],
                height: back.bbox[3],
                generated: true,
            });
            views.push(back);
        }
    }
    let (mesh, texture) = reconstruct(&views, s)?;
    Ok(Reconstruction { mesh, texture, views: infos })
}

/// Fabrique la vue de dos d'une personne à partir de sa vue de face : image retournée,
/// tête couverte de la couleur des cheveux (pas de visage), vêtements adoucis (sans boutons,
/// poches ni cravate), mains et bras gardant leur couleur.
pub fn synth_person_back(front: &SourceView) -> SourceView {
    let [x0, y0, x1, y1] = front.bbox;
    let (cw, ch) = (x1 - x0, y1 - y0);
    let iw = front.img.width() as usize;
    let src_in = |x: u32, y: u32| front.mask[y as usize * iw + x as usize];
    // Image retournée gauche-droite (le dos montre la gauche de la personne à droite).
    let mut img = RgbaImage::new(cw, ch);
    let mut mask = vec![false; (cw * ch) as usize];
    for y in 0..ch {
        for x in 0..cw {
            let (sx, sy) = (x1 - 1 - x, y0 + y);
            img.put_pixel(x, y, *front.img.get_pixel(sx, sy));
            mask[(y * cw + x) as usize] = src_in(sx, sy);
        }
    }
    let w = cw as usize;
    let at = |x: usize, y: usize| mask[y * w + x];
    // Largeur de la silhouette à chaque ligne : la tête finit au cou (ligne la plus étroite
    // entre 8 % et 22 % de la hauteur).
    let width_at = |y: usize| (0..w).filter(|&x| at(x, y)).count();
    let first = (0..ch as usize).find(|&y| width_at(y) > 0).unwrap_or(0);
    let (a, b) = (first + (ch as f32 * 0.08) as usize, first + (ch as f32 * 0.22) as usize);
    let neck = (a..b.min(ch as usize)).min_by_key(|&y| width_at(y)).unwrap_or(first + (ch as f32 * 0.13) as usize);
    // Couleur des cheveux : le haut de la tête.
    let top_rows = first..(first + ((neck - first) / 4).max(1));
    let mut sum = [0u64; 3];
    let mut cnt = 0u64;
    for y in top_rows {
        for x in 0..w {
            if at(x, y) {
                let p = img.get_pixel(x as u32, y as u32).0;
                for k in 0..3 {
                    sum[k] += p[k] as u64;
                }
                cnt += 1;
            }
        }
    }
    let hair = if cnt > 0 { [(sum[0] / cnt) as u8, (sum[1] / cnt) as u8, (sum[2] / cnt) as u8] } else { [40, 30, 25] };
    // Vêtements adoucis : moyenne locale à l'intérieur de la silhouette.
    let r = ((ch as f32) * 0.012).max(2.0) as i64;
    let src = img.clone();
    let soft = |cx: usize, cy: usize| -> [u8; 3] {
        let mut s = [0u32; 3];
        let mut n = 0u32;
        let mut y = cy as i64 - r;
        while y <= cy as i64 + r {
            let mut x = cx as i64 - r;
            while x <= cx as i64 + r {
                if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < ch as usize && at(x as usize, y as usize) {
                    let p = src.get_pixel(x as u32, y as u32).0;
                    for k in 0..3 {
                        s[k] += p[k] as u32;
                    }
                    n += 1;
                }
                x += 2;
            }
            y += 2;
        }
        if n == 0 { [0, 0, 0] } else { [(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8] }
    };
    for y in 0..ch as usize {
        for x in 0..w {
            if !at(x, y) {
                continue;
            }
            let c = if y < neck {
                // Arrière de la tête : cheveux, avec un léger relief de la photo.
                let p = src.get_pixel(x as u32, y as u32).0;
                let l = (p[0] as f32 + p[1] as f32 + p[2] as f32) / 765.0;
                let k = 0.85 + 0.3 * l;
                [(hair[0] as f32 * k).min(255.0) as u8, (hair[1] as f32 * k).min(255.0) as u8, (hair[2] as f32 * k).min(255.0) as u8]
            } else {
                soft(x, y)
            };
            img.put_pixel(x as u32, y as u32, Rgba([c[0], c[1], c[2], 255]));
        }
    }
    SourceView {
        name: "dos créé automatiquement".into(),
        img: Arc::new(img),
        mask: Arc::new(mask),
        bbox: [0, 0, cw, ch],
        angle: ViewAngle::Back,
    }
}

pub fn reconstruct(views: &[SourceView], s: &Settings) -> Result<(Mesh, RgbaImage), String> {
    // --- Projections des vues ---------------------------------------------------------------
    let find = |a: ViewAngle| views.iter().position(|v| v.angle == a);
    let mut projs: Vec<Proj> = Vec::new();
    for (i, v) in views.iter().enumerate() {
        let theta = match v.angle {
            ViewAngle::Front => 0.0,
            ViewAngle::SideRight => 90.0,
            ViewAngle::Back => 180.0,
            ViewAngle::SideLeft => 270.0,
            _ => continue,
        };
        projs.push(side_proj(i, v, theta));
    }
    if projs.is_empty() {
        return Err("il faut au moins une vue de face, de profil ou de dos".into());
    }
    let front_like = find(ViewAngle::Front).or(find(ViewAngle::Back));
    let side_like = find(ViewAngle::SideRight).or(find(ViewAngle::SideLeft));
    let ratio = |i: usize| views[i].w() / views[i].h().max(1.0);
    if let Some(t) = find(ViewAngle::Top) {
        let v = &views[t];
        let st = if let Some(f) = front_like {
            Some(v.w() / ratio(f).max(1e-3))
        } else {
            side_like.map(|sd| v.h() / ratio(sd).max(1e-3))
        };
        if let Some(st) = st {
            projs.push(Proj {
                view: t,
                top: true,
                right: [1.0, 0.0, 0.0],
                cam: [0.0, 1.0, 0.0],
                s: st,
                cx: (v.bbox[0] + v.bbox[2]) as f32 * 0.5,
                base: (v.bbox[1] + v.bbox[3]) as f32 * 0.5,
            });
        }
    }
    let top_proj = projs.iter().find(|p| p.top).copied();

    // --- Dimensions (hauteur = 1) ------------------------------------------------------------
    let mut xh: f32 = 0.0;
    let mut zh: f32 = 0.0;
    for p in &projs {
        let v = &views[p.view];
        if p.top {
            xh = xh.max(v.w() / p.s * 0.5);
            zh = zh.max(v.h() / p.s * 0.5);
        } else if p.right[0].abs() > 0.5 {
            xh = xh.max(v.w() / p.s * 0.5);
        } else {
            zh = zh.max(v.w() / p.s * 0.5);
        }
    }
    let guess = |other: f32| match s.shape {
        Shape::Organic | Shape::Person => other * 0.55,
        Shape::Vehicle => other * 0.5,
        Shape::Building => other,
    };
    if xh <= 0.0 {
        xh = guess(zh).max(0.05);
    }
    if zh <= 0.0 {
        zh = guess(xh).max(0.05);
    }
    let known_z = side_like.is_some() || top_proj.is_some();

    // --- Grille de voxels --------------------------------------------------------------------
    let res = s.resolution.clamp(24, 384) as f32;
    let margin = 0.03 * 1f32.max(2.0 * xh).max(2.0 * zh);
    let pad = 2usize;
    let mut vox = (1f32.max(2.0 * xh).max(2.0 * zh) + 2.0 * margin) / res;
    let dims = |vox: f32| {
        (
            ((2.0 * (xh + margin)) / vox).ceil() as usize + 2 * pad,
            ((1.0 + 2.0 * margin) / vox).ceil() as usize + 2 * pad,
            ((2.0 * (zh + margin)) / vox).ceil() as usize + 2 * pad,
        )
    };
    let (mut nx, mut ny, mut nz) = dims(vox);
    while nx * ny * nz > 40_000_000 {
        vox *= 1.15;
        (nx, ny, nz) = dims(vox);
    }
    let wx = |i: usize| -xh - margin + (i as f32 - pad as f32 + 0.5) * vox;
    let wy = |j: usize| -margin + (j as f32 - pad as f32 + 0.5) * vox;
    let wz = |k: usize| -zh - margin + (k as f32 - pad as f32 + 0.5) * vox;
    let idx = |i: usize, j: usize, k: usize| (k * ny + j) * nx + i;

    // Intervalles occupés par ligne dans la vue de face (x) et de profil (z), pour arrondir.
    let intervals = |n: usize, test: &dyn Fn(usize) -> bool| -> (Vec<(u16, u16)>, usize) {
        let mut out = vec![(u16::MAX, 0u16); n];
        let mut count = 0;
        let mut a = pad;
        while a < n - pad {
            if test(a) {
                let mut b = a;
                while b + 1 < n - pad && test(b + 1) {
                    b += 1;
                }
                for c in a..=b {
                    out[c] = (a as u16, b as u16);
                }
                count += 1;
                a = b + 1;
            } else {
                a += 1;
            }
        }
        (out, count)
    };
    let fproj = front_like.and_then(|f| projs.iter().find(|p| p.view == f).copied());
    let sproj = side_like.and_then(|f| projs.iter().find(|p| p.view == f).copied());
    let rows: Vec<((Vec<(u16, u16)>, usize), Option<(Vec<(u16, u16)>, usize)>)> = (0..ny)
        .into_par_iter()
        .map(|j| {
            let y = wy(j);
            let xi = match fproj {
                Some(p) => intervals(nx, &|i| views[p.view].inside_p(p.project([wx(i), y, 0.0]))),
                None => intervals(nx, &|_| true),
            };
            let zi = sproj.map(|p| intervals(nz, &|k| views[p.view].inside_p(p.project([0.0, y, wz(k)]))));
            (xi, zi)
        })
        .collect();

    let power = match s.shape {
        Shape::Organic | Shape::Person => 2.0f32,
        Shape::Vehicle => 4.0,
        Shape::Building => 0.0,
    };
    let zfull = (pad as f32, (nz - pad - 1) as f32);

    // --- Sculpture ---------------------------------------------------------------------------
    let mut occ = vec![0f32; nx * ny * nz];
    occ.par_chunks_mut(nx * ny).enumerate().for_each(|(k, slab)| {
        if k < pad || k >= nz - pad {
            return;
        }
        let z = wz(k);
        for j in pad..ny - pad {
            let ((xi, xcount), zi) = &rows[j];
            let y = wy(j);
            for i in pad..nx - pad {
                let (xa, xb) = xi[i];
                if xa == u16::MAX {
                    continue;
                }
                let (za, zb) = match zi {
                    Some((zv, _)) => {
                        let r = zv[k];
                        if r.0 == u16::MAX {
                            continue;
                        }
                        (r.0 as f32, r.1 as f32)
                    }
                    None => zfull,
                };
                let p = [wx(i), y, z];
                if !projs.iter().all(|pr| views[pr.view].inside_p(pr.project(p))) {
                    continue;
                }
                // Forme arrondie entre les vues.
                let hx = (xb as f32 - xa as f32) * 0.5 + 0.5;
                let xc = (xa as f32 + xb as f32) * 0.5;
                let mut hz = (zb - za) * 0.5 + 0.5;
                let mut zc = (za + zb) * 0.5;
                if !known_z {
                    hz = hz.min(hx * if s.shape.is_organic() { 0.9 } else { 1.5 });
                    zc = (nz as f32 - 1.0) * 0.5;
                } else if s.shape.is_organic() && *xcount > 1 {
                    // Bras, jambes : pas plus épais que larges.
                    hz = hz.min(hx * 1.6);
                }
                let mut v = 1.0f32;
                if power > 0.0 || !known_z {
                    let pw = if power > 0.0 { power } else { 6.0 };
                    let e = ((i as f32 - xc).abs() / hx).powf(pw) + ((k as f32 - zc).abs() / hz).powf(pw);
                    // Valeur douce : distance approximative au bord (en voxels), pour une surface lisse.
                    let r = e.powf(1.0 / pw);
                    v = (0.5 + (1.0 - r) * hx.min(hz)).clamp(0.0, 1.0);
                    if v <= 0.0 {
                        continue;
                    }
                }
                slab[j * nx + i] = v;
            }
        }
    });
    if !occ.iter().any(|&v| v > 0.5) {
        return Err("les vues ne se recoupent pas : vérifiez le côté de chaque vue".into());
    }
    let solid: Vec<bool> = occ.iter().map(|&v| v > 0.5).collect();

    // Adoucit le volume (bords des voxels).
    for _ in 0..3 {
        blur_axis(&mut occ, nx, ny, nz, 0);
        blur_axis(&mut occ, nx, ny, nz, 1);
        blur_axis(&mut occ, nx, ny, nz, 2);
    }

    // --- Surface -----------------------------------------------------------------------------
    let mut m = surface_nets(&occ, nx, ny, nz, 0.5);
    if m.indices.is_empty() {
        return Err("objet trop fin pour ce niveau de détail : augmentez le détail".into());
    }
    for p in m.positions.iter_mut() {
        *p = [-xh - margin + (p[0] - pad as f32 + 0.5) * vox, -margin + (p[1] - pad as f32 + 0.5) * vox, -zh - margin + (p[2] - pad as f32 + 0.5) * vox];
    }
    taubin(&mut m, 10);
    m.compute_normals();

    // --- Texture : quelle photo pour chaque triangle ? ------------------------------------------
    let mut tex_views: Vec<Proj> = projs.clone();
    // Un seul profil : l'autre côté reprend la même photo (miroir).
    for (have, miss) in [(ViewAngle::SideRight, ViewAngle::SideLeft), (ViewAngle::SideLeft, ViewAngle::SideRight)] {
        if find(miss).is_none() {
            if let Some(p) = projs.iter().find(|p| views[p.view].angle == have) {
                let mut q = *p;
                q.cam = [-q.cam[0], -q.cam[1], -q.cam[2]];
                tex_views.push(q);
            }
        }
    }
    let nv = tex_views.len();
    let occupied = |p: [f32; 3]| -> bool {
        let fi = (p[0] + xh + margin) / vox + pad as f32 - 0.5;
        let fj = (p[1] + margin) / vox + pad as f32 - 0.5;
        let fk = (p[2] + zh + margin) / vox + pad as f32 - 0.5;
        let (i, j, k) = (fi.round(), fj.round(), fk.round());
        if i < 0.0 || j < 0.0 || k < 0.0 || i >= nx as f32 || j >= ny as f32 || k >= nz as f32 {
            return false;
        }
        solid[idx(i as usize, j as usize, k as usize)]
    };
    let max_t = (nx.max(ny).max(nz) as f32) * vox * 1.5;
    let visible: Vec<u32> = m
        .positions
        .par_iter()
        .map(|&p| {
            let mut bits = 0u32;
            for (vi, tv) in tex_views.iter().enumerate() {
                let c = tv.cam;
                let mut t = 2.5 * vox;
                let mut hidden = false;
                while t < max_t {
                    if occupied([p[0] + c[0] * t, p[1] + c[1] * t, p[2] + c[2] * t]) {
                        hidden = true;
                        break;
                    }
                    t += 0.8 * vox;
                }
                if !hidden {
                    bits |= 1 << vi;
                }
            }
            bits
        })
        .collect();

    // Planche de texture : les photos utilisées, côte à côte, débordées sur le fond.
    let mut used_views: Vec<usize> = tex_views.iter().map(|t| t.view).collect();
    used_views.sort_unstable();
    used_views.dedup();
    let border = 4u32;
    let mut crops: Vec<(usize, [u32; 4], u32)> = Vec::new(); // vue, rect, décalage x dans l'atlas
    let mut aw = 0u32;
    let mut ah = 0u32;
    for &v in &used_views {
        let sv = &views[v];
        let (iw, ih) = sv.img.dimensions();
        let r = [sv.bbox[0].saturating_sub(border), sv.bbox[1].saturating_sub(border), (sv.bbox[2] + border).min(iw), (sv.bbox[3] + border).min(ih)];
        crops.push((v, r, aw));
        aw += r[2] - r[0] + 2;
        ah = ah.max(r[3] - r[1]);
    }
    let mut atlas = RgbaImage::from_pixel(aw.max(1), ah.max(1), Rgba([128, 128, 128, 255]));
    let mut filled = vec![false; (aw.max(1) * ah.max(1)) as usize];
    for &(v, r, ox) in &crops {
        let sv = &views[v];
        let iw = sv.img.width();
        for y in r[1]..r[3] {
            for x in r[0]..r[2] {
                let ax = ox + x - r[0];
                let ay = y - r[1];
                let mut px = *sv.img.get_pixel(x, y);
                px.0[3] = 255;
                atlas.put_pixel(ax, ay, px);
                filled[(ay * aw + ax) as usize] = sv.mask[(y * iw + x) as usize];
            }
        }
    }
    bleed(&mut atlas, &mut filled, 40);

    let uv_of = |vi: usize, p: [f32; 3]| -> [f32; 2] {
        let tv = &tex_views[vi];
        let (px, py) = tv.project(p);
        let (_, r, ox) = crops.iter().find(|c| c.0 == tv.view).copied().unwrap();
        let px = px.clamp(r[0] as f32, r[2] as f32 - 1.0);
        let py = py.clamp(r[1] as f32, r[3] as f32 - 1.0);
        [(ox as f32 + px - r[0] as f32 + 0.5) / aw as f32, (py - r[1] as f32 + 0.5) / ah as f32]
    };

    let ntri = m.indices.len() / 3;
    let normals_t: Vec<[f32; 3]> = m
        .indices
        .chunks_exact(3)
        .map(|t| {
            let n = crate::mesh::face_normal(m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]);
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-12);
            [n[0] / l, n[1] / l, n[2] / l]
        })
        .collect();
    let score = |t: usize, vi: usize| {
        let n = normals_t[t];
        n[0] * tex_views[vi].cam[0] + n[1] * tex_views[vi].cam[1] + n[2] * tex_views[vi].cam[2]
    };
    let mut choice: Vec<usize> = (0..ntri)
        .map(|t| {
            let tri = &m.indices[t * 3..t * 3 + 3];
            let vis = visible[tri[0] as usize] & visible[tri[1] as usize] & visible[tri[2] as usize];
            let mut best: Option<(usize, f32)> = None;
            for vi in 0..nv {
                if vis & (1 << vi) != 0 {
                    let sc = score(t, vi);
                    if sc > 0.05 && best.map(|b| sc > b.1).unwrap_or(true) {
                        best = Some((vi, sc));
                    }
                }
            }
            match best {
                Some((vi, _)) => vi,
                None => (0..nv).max_by(|&x, &y| score(t, x).partial_cmp(&score(t, y)).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0),
            }
        })
        .collect();
    // Retire les petites taches : un triangle isolé prend la photo de ses voisins.
    for _ in 0..2 {
        let mut votes = vec![[0u16; 8]; m.positions.len()];
        for t in 0..ntri {
            for &v in &m.indices[t * 3..t * 3 + 3] {
                let c = &mut votes[v as usize][choice[t].min(7)];
                *c = c.saturating_add(1);
            }
        }
        for t in 0..ntri {
            let mut tally = [0u32; 8];
            for &v in &m.indices[t * 3..t * 3 + 3] {
                for (k, c) in votes[v as usize].iter().enumerate() {
                    tally[k] += *c as u32;
                }
            }
            let (best, cnt) = tally.iter().enumerate().max_by_key(|x| *x.1).map(|(k, c)| (k, *c)).unwrap();
            let own = tally[choice[t].min(7)];
            if best < nv && best != choice[t] && cnt > own * 2 && score(t, best) > 0.0 {
                choice[t] = best;
            }
        }
    }

    let mut out = Mesh::default();
    let mut remap = vec![u32::MAX; m.positions.len() * nv];
    for (t, tri) in m.indices.chunks_exact(3).enumerate() {
        let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        let vi = choice[t];
        for &v in &[a, b, c] {
            let key = v * nv + vi;
            if remap[key] == u32::MAX {
                remap[key] = out.push(m.positions[v], uv_of(vi, m.positions[v]));
                let last = out.normals.len() - 1;
                out.normals[last] = m.normals[v];
            }
            out.indices.push(remap[key]);
        }
    }

    // --- Taille finale : posé au sol, centré -------------------------------------------------
    let (min, max) = out.bounds();
    let ext = (max[0] - min[0]).max(max[1] - min[1]).max(max[2] - min[2]).max(1e-6);
    let k = s.size.max(0.001) / ext;
    let cx = (min[0] + max[0]) * 0.5;
    let cz = (min[2] + max[2]) * 0.5;
    for p in out.positions.iter_mut() {
        *p = [(p[0] - cx) * k, (p[1] - min[1]) * k, (p[2] - cz) * k];
    }
    Ok((out, atlas))
}

impl SourceView {
    fn inside_p(&self, p: (f32, f32)) -> bool {
        self.inside(p.0, p.1)
    }
}

/// Flou 1-2-1 le long d'un axe.
fn blur_axis(f: &mut [f32], nx: usize, ny: usize, nz: usize, axis: usize) {
    let src = f.to_vec();
    let (stride, len) = match axis {
        0 => (1, nx),
        1 => (nx, ny),
        _ => (nx * ny, nz),
    };
    f.par_iter_mut().enumerate().for_each(|(id, v)| {
        let i = id % nx;
        let j = (id / nx) % ny;
        let k = id / (nx * ny);
        let c = [i, j, k][axis];
        let a = if c > 0 { src[id - stride] } else { 0.0 };
        let b = if c + 1 < len { src[id + stride] } else { 0.0 };
        *v = 0.25 * a + 0.5 * src[id] + 0.25 * b;
    });
}

/// Extraction de surface « surface nets » : un sommet par cellule traversée, des quadrilatères
/// entre cellules voisines. Positions en unités de grille.
fn surface_nets(f: &[f32], nx: usize, ny: usize, nz: usize, iso: f32) -> Mesh {
    let idx = |i: usize, j: usize, k: usize| (k * ny + j) * nx + i;
    let (cx, cy) = (nx - 1, ny - 1);
    let cidx = |i: usize, j: usize, k: usize| (k * cy + j) * cx + i;
    const C: [[usize; 3]; 8] = [[0, 0, 0], [1, 0, 0], [0, 1, 0], [1, 1, 0], [0, 0, 1], [1, 0, 1], [0, 1, 1], [1, 1, 1]];
    const E: [(usize, usize); 12] = [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];
    let mut m = Mesh::default();
    let mut cell_v = vec![u32::MAX; cx * cy * (nz - 1)];
    for k in 0..nz - 1 {
        for j in 0..ny - 1 {
            for i in 0..nx - 1 {
                let mut vals = [0f32; 8];
                let mut inside = 0u8;
                for (c, o) in C.iter().enumerate() {
                    let v = f[idx(i + o[0], j + o[1], k + o[2])];
                    vals[c] = v;
                    if v > iso {
                        inside |= 1 << c;
                    }
                }
                if inside == 0 || inside == 255 {
                    continue;
                }
                let mut sum = [0f32; 3];
                let mut cnt = 0.0;
                for &(a, b) in &E {
                    let (va, vb) = (vals[a], vals[b]);
                    if (va > iso) != (vb > iso) {
                        let t = (iso - va) / (vb - va);
                        for d in 0..3 {
                            sum[d] += C[a][d] as f32 + t * (C[b][d] as f32 - C[a][d] as f32);
                        }
                        cnt += 1.0;
                    }
                }
                cell_v[cidx(i, j, k)] = m.push([i as f32 + sum[0] / cnt, j as f32 + sum[1] / cnt, k as f32 + sum[2] / cnt], [0.0, 0.0]);
            }
        }
    }
    let quad = |m: &mut Mesh, q: [u32; 4], out: [f32; 3]| {
        if q.iter().all(|&v| v != u32::MAX) {
            quad_facing(m, q, out);
        }
    };
    for k in 1..nz - 1 {
        for j in 1..ny - 1 {
            for i in 0..nx - 1 {
                let a = f[idx(i, j, k)] > iso;
                if a != (f[idx(i + 1, j, k)] > iso) {
                    let q = [cell_v[cidx(i, j - 1, k - 1)], cell_v[cidx(i, j, k - 1)], cell_v[cidx(i, j, k)], cell_v[cidx(i, j - 1, k)]];
                    quad(&mut m, q, if a { [1.0, 0.0, 0.0] } else { [-1.0, 0.0, 0.0] });
                }
            }
        }
    }
    for k in 1..nz - 1 {
        for j in 0..ny - 1 {
            for i in 1..nx - 1 {
                let a = f[idx(i, j, k)] > iso;
                if a != (f[idx(i, j + 1, k)] > iso) {
                    let q = [cell_v[cidx(i - 1, j, k - 1)], cell_v[cidx(i, j, k - 1)], cell_v[cidx(i, j, k)], cell_v[cidx(i - 1, j, k)]];
                    quad(&mut m, q, if a { [0.0, 1.0, 0.0] } else { [0.0, -1.0, 0.0] });
                }
            }
        }
    }
    for k in 0..nz - 1 {
        for j in 1..ny - 1 {
            for i in 1..nx - 1 {
                let a = f[idx(i, j, k)] > iso;
                if a != (f[idx(i, j, k + 1)] > iso) {
                    let q = [cell_v[cidx(i - 1, j - 1, k)], cell_v[cidx(i, j - 1, k)], cell_v[cidx(i, j, k)], cell_v[cidx(i - 1, j, k)]];
                    quad(&mut m, q, if a { [0.0, 0.0, 1.0] } else { [0.0, 0.0, -1.0] });
                }
            }
        }
    }
    m
}

/// Lissage de Taubin (adoucit sans faire maigrir l'objet).
fn taubin(m: &mut Mesh, iterations: usize) {
    let n = m.positions.len();
    let mut nb: Vec<Vec<u32>> = vec![Vec::new(); n];
    for t in m.indices.chunks_exact(3) {
        for e in 0..3 {
            let (a, b) = (t[e], t[(e + 1) % 3]);
            nb[a as usize].push(b);
            nb[b as usize].push(a);
        }
    }
    for l in nb.iter_mut() {
        l.sort_unstable();
        l.dedup();
    }
    for _ in 0..iterations {
        for factor in [0.5f32, -0.53] {
            let src = m.positions.clone();
            m.positions.par_iter_mut().enumerate().for_each(|(i, p)| {
                if nb[i].is_empty() {
                    return;
                }
                let mut avg = [0f32; 3];
                for &j in &nb[i] {
                    for d in 0..3 {
                        avg[d] += src[j as usize][d];
                    }
                }
                let c = nb[i].len() as f32;
                for d in 0..3 {
                    p[d] = src[i][d] + factor * (avg[d] / c - src[i][d]);
                }
            });
        }
    }
}

/// Étend les couleurs de l'objet sur le fond voisin : pas de liseré blanc sur les bords du modèle.
fn bleed(img: &mut RgbaImage, filled: &mut [bool], passes: usize) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    for _ in 0..passes {
        let prev = filled.to_vec();
        let src = img.clone();
        let mut changed = false;
        for y in 0..h {
            for x in 0..w {
                if prev[y * w + x] {
                    continue;
                }
                let mut sum = [0u32; 3];
                let mut cnt = 0;
                for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                    let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                    if xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h && prev[yy as usize * w + xx as usize] {
                        let p = src.get_pixel(xx as u32, yy as u32).0;
                        for d in 0..3 {
                            sum[d] += p[d] as u32;
                        }
                        cnt += 1;
                    }
                }
                if cnt > 0 {
                    img.put_pixel(x as u32, y as u32, Rgba([(sum[0] / cnt) as u8, (sum[1] / cnt) as u8, (sum[2] / cnt) as u8, 255]));
                    filled[y * w + x] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Planche de test : une « personne » simplifiée vue de face, de profil et de dos.
    fn sheet() -> RgbaImage {
        RgbaImage::from_fn(600, 300, |x, y| {
            let (x, y) = (x as f32, y as f32);
            let body = |cx: f32, half_w: f32| {
                let head = (x - cx).powi(2) + (y - 50.0).powi(2) < 400.0;
                let torso = (x - cx).abs() < half_w && (70.0..280.0).contains(&y);
                head || torso
            };
            if body(100.0, 40.0) || body(300.0, 18.0) || body(500.0, 40.0) {
                Rgba([150, 90, 60, 255])
            } else {
                Rgba([255, 255, 255, 255])
            }
        })
    }

    #[test]
    fn sheet_splits_and_rebuilds() {
        let s = Settings { resolution: 64, ..Settings::default() };
        let r = from_images(vec![("planche".into(), sheet())], &[], &s).unwrap();
        let angles: Vec<&str> = r.views.iter().map(|v| v.angle.as_str()).collect();
        assert_eq!(angles, vec!["front", "side_right", "back"]);
        assert!(r.mesh.triangle_count() > 500);
        let (min, max) = r.mesh.bounds();
        // Plus large que profond (40 vs 18 de demi-largeur).
        assert!(max[0] - min[0] > (max[2] - min[2]) * 1.5, "{min:?} {max:?}");
        assert!((max[1] - min[1] - s.size).abs() < 0.01);
    }
}
