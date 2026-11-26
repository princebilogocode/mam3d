//! Détourage intelligent : un premier détourage automatique, corrigé par des repères
//! (« broches ») posés par la personne ou un agent.
//!
//! * Repère vert (garder) : la zone de couleur semblable autour du point fait partie de l'objet.
//! * Repère rouge (retirer) : la zone de couleur semblable autour du point est du fond.
//!
//! Le contour final est placé sur les vrais bords de l'image : une bande incertaine autour du
//! détourage automatique est partagée entre objet et fond par une « ligne de partage des eaux »
//! (chaque pixel rejoint le côté dont il est le plus proche en couleur, de proche en proche).

use image::{Rgba, RgbaImage};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pin {
    /// Position en pixels de l'image d'origine.
    pub x: f32,
    pub y: f32,
    /// true = garder (objet), false = retirer (fond).
    pub keep: bool,
    /// Rayon du pinceau en pixels (la zone autour du point est forcée).
    #[serde(default = "default_radius")]
    pub radius: f32,
}

fn default_radius() -> f32 {
    6.0
}

const UNKNOWN: u8 = 0;
const FG: u8 = 1;
const BG: u8 = 2;

fn color(raw: &[u8], i: usize) -> [f32; 3] {
    [raw[i * 4] as f32, raw[i * 4 + 1] as f32, raw[i * 4 + 2] as f32]
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn neighbours(i: usize, w: usize, h: usize) -> [usize; 4] {
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
    nb
}

/// Zone de couleur semblable autour d'un point (la référence suit les dégradés doux).
fn grow(raw: &[u8], w: usize, h: usize, start: usize, thr: f32, blocked: &[bool]) -> Vec<usize> {
    let mut seen = vec![false; w * h];
    let mut out = vec![start];
    let mut refc = vec![[0f32; 3]; 0];
    refc.resize(w * h, [0.0; 3]);
    seen[start] = true;
    refc[start] = color(raw, start);
    let mut q = VecDeque::from([start]);
    // Pas plus d'un quart de l'image par repère : un clic ne doit pas tout avaler.
    let limit = (w * h) / 4;
    while let Some(i) = q.pop_front() {
        if out.len() > limit {
            break;
        }
        let r = refc[i];
        let seed = refc[start];
        for j in neighbours(i, w, h) {
            if j == usize::MAX || seen[j] || blocked[j] {
                continue;
            }
            let c = color(raw, j);
            // Proche de la couleur cliquée, et sans saut entre voisins : la zone ne déborde pas
            // sur une partie voisine de teinte proche.
            if dist(c, seed) <= thr && dist(c, r) <= thr * 0.5 {
                seen[j] = true;
                refc[j] = [r[0] * 0.85 + c[0] * 0.15, r[1] * 0.85 + c[1] * 0.15, r[2] * 0.85 + c[2] * 0.15];
                out.push(j);
                q.push_back(j);
            }
        }
    }
    out
}

fn erode(m: &[bool], w: usize, h: usize, times: usize) -> Vec<bool> {
    let mut cur = m.to_vec();
    for _ in 0..times {
        let src = cur.clone();
        for i in 0..w * h {
            if src[i] && neighbours(i, w, h).iter().any(|&j| j == usize::MAX || !src[j]) {
                cur[i] = false;
            }
        }
    }
    cur
}

/// Masque de l'objet : détourage automatique, repères, puis contour calé sur les bords.
pub fn refined_mask(img: &RgbaImage, threshold: f32, pins: &[Pin]) -> Vec<bool> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let n = w * h;
    if n == 0 {
        return vec![];
    }
    let raw = img.as_raw();
    let auto = crate::multiview::object_mask(img, threshold);

    // 1. Graines sûres : l'intérieur de l'objet et l'intérieur du fond (bande de 3 px incertaine).
    let band = 3;
    let fg_core = erode(&auto, w, h, band);
    let inv: Vec<bool> = auto.iter().map(|&v| !v).collect();
    let bg_core = erode(&inv, w, h, band);
    let mut label = vec![UNKNOWN; n];
    for i in 0..n {
        if fg_core[i] {
            label[i] = FG;
        } else if bg_core[i] {
            label[i] = BG;
        }
    }

    // 2. Repères : la zone semblable autour de chaque point change de côté.
    let thr = threshold.max(1.0) * 0.7;
    let disk = |p: &Pin| -> Vec<usize> {
        let r = p.radius.max(1.0);
        let mut v = Vec::new();
        let (x0, x1) = ((p.x - r).floor().max(0.0) as usize, ((p.x + r).ceil() as usize).min(w.saturating_sub(1)));
        let (y0, y1) = ((p.y - r).floor().max(0.0) as usize, ((p.y + r).ceil() as usize).min(h.saturating_sub(1)));
        for y in y0..=y1 {
            for x in x0..=x1 {
                if (x as f32 - p.x).powi(2) + (y as f32 - p.y).powi(2) <= r * r {
                    v.push(y * w + x);
                }
            }
        }
        v
    };
    // Les zones d'un repère s'arrêtent aux pinceaux de l'autre couleur.
    let mut keep_brush = vec![false; n];
    let mut remove_brush = vec![false; n];
    for p in pins.iter().filter(|p| p.x >= 0.0 && p.y >= 0.0 && (p.x as usize) < w && (p.y as usize) < h) {
        for i in disk(p) {
            if p.keep {
                keep_brush[i] = true;
            } else {
                remove_brush[i] = true;
            }
        }
    }
    for p in pins.iter().filter(|p| p.x >= 0.0 && p.y >= 0.0 && (p.x as usize) < w && (p.y as usize) < h) {
        let start = p.y as usize * w + p.x as usize;
        let blocked = if p.keep { &remove_brush } else { &keep_brush };
        let lab = if p.keep { FG } else { BG };
        for i in grow(raw, w, h, start, thr, blocked) {
            label[i] = lab;
        }
    }
    // Le pinceau lui-même passe avant tout ; puis on rouvre une bande incertaine autour des
    // zones changées pour que leur bord se cale lui aussi sur l'image.
    for i in 0..n {
        if keep_brush[i] {
            label[i] = FG;
        } else if remove_brush[i] {
            label[i] = BG;
        }
    }
    let fg_now: Vec<bool> = label.iter().map(|&l| l == FG).collect();
    let bg_now: Vec<bool> = label.iter().map(|&l| l == BG).collect();
    let fg_in = erode(&fg_now, w, h, 2);
    let bg_in = erode(&bg_now, w, h, 2);
    for i in 0..n {
        if !(keep_brush[i] || remove_brush[i]) && !fg_in[i] && !bg_in[i] && label[i] != UNKNOWN {
            // Bord d'une zone : on le laisse décider par l'image.
            let mixed = neighbours(i, w, h).iter().any(|&j| j != usize::MAX && label[j] != label[i]);
            if mixed {
                label[i] = UNKNOWN;
            }
        }
    }

    // 3. Partage des eaux : chaque pixel incertain rejoint le côté le plus proche en couleur.
    let mut heap: BinaryHeap<Reverse<(u32, usize, u8)>> = BinaryHeap::new();
    for i in 0..n {
        if label[i] == UNKNOWN {
            continue;
        }
        for j in neighbours(i, w, h) {
            if j != usize::MAX && label[j] == UNKNOWN {
                heap.push(Reverse(((dist(color(raw, i), color(raw, j)) * 4.0) as u32, j, label[i])));
            }
        }
    }
    while let Some(Reverse((_, i, l))) = heap.pop() {
        if label[i] != UNKNOWN {
            continue;
        }
        label[i] = l;
        let c = color(raw, i);
        for j in neighbours(i, w, h) {
            if j != usize::MAX && label[j] == UNKNOWN {
                heap.push(Reverse(((dist(c, color(raw, j)) * 4.0) as u32, j, l)));
            }
        }
    }
    label.iter().map(|&l| l == FG).collect()
}

/// Image détourée : le fond devient transparent, le contour est adouci d'un pixel.
pub fn apply(img: &RgbaImage, mask: &[bool]) -> RgbaImage {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let at = |x: i64, y: i64| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && mask[y as usize * w + x as usize];
    RgbaImage::from_fn(img.width(), img.height(), |x, y| {
        let p = *img.get_pixel(x, y);
        let (xi, yi) = (x as i64, y as i64);
        if !at(xi, yi) {
            return Rgba([0, 0, 0, 0]);
        }
        let inner = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().filter(|(dx, dy)| at(xi + dx, yi + dy)).count();
        Rgba([p.0[0], p.0[1], p.0[2], if inner == 4 { 255 } else { 200 }])
    })
}

/// Détourage complet d'une image avec des repères éventuels.
pub fn cutout(img: &RgbaImage, threshold: f32, pins: &[Pin]) -> RgbaImage {
    let mask = refined_mask(img, threshold, pins);
    apply(img, &mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fond dégradé, disque rouge, et une tache grise (ombre) collée au disque.
    fn scene() -> RgbaImage {
        RgbaImage::from_fn(200, 200, |x, y| {
            let (fx, fy) = (x as f32 - 100.0, y as f32 - 90.0);
            if fx * fx + fy * fy < 50.0 * 50.0 {
                Rgba([200, 40, 40, 255])
            } else if (60..140).contains(&x) && (140..170).contains(&y) {
                Rgba([90, 90, 95, 255])
            } else {
                let g = (170.0 + (x as f32) * 0.2) as u8;
                Rgba([g, g, g, 255])
            }
        })
    }

    #[test]
    fn pins_remove_a_region() {
        let img = scene();
        let auto = refined_mask(&img, 40.0, &[]);
        assert!(auto[150 * 200 + 100], "l'ombre fait partie du détourage automatique");
        assert!(auto[90 * 200 + 100]);
        assert!(!auto[5 * 200 + 5]);
        let fixed = refined_mask(&img, 40.0, &[Pin { x: 100.0, y: 160.0, keep: false, radius: 3.0 }]);
        assert!(!fixed[150 * 200 + 100], "le repère rouge retire l'ombre");
        assert!(fixed[90 * 200 + 100], "le disque reste");
    }
}
