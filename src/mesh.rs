//! Transforme une image 2D en maillage 3D (relief, extrusion ou gonflage).
//!
//! Le maillage est construit dans le repère glTF (Y vers le haut, l'image fait face à +Z).
//! Blender le convertit tout seul : l'image apparaît debout, face à la vue de face.

use crate::settings::{Background, Mode, Settings};
use image::{RgbaImage, imageops::FilterType};

#[derive(Default, Clone)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for p in &self.positions {
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        if self.positions.is_empty() {
            return ([0.0; 3], [0.0; 3]);
        }
        (min, max)
    }

    pub(crate) fn push(&mut self, p: [f32; 3], uv: [f32; 2]) -> u32 {
        self.positions.push(p);
        self.uvs.push(uv);
        self.normals.push([0.0; 3]);
        (self.positions.len() - 1) as u32
    }

    pub(crate) fn compute_normals(&mut self) {
        for n in self.normals.iter_mut() {
            *n = [0.0; 3];
        }
        for t in self.indices.chunks_exact(3) {
            let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
            let n = face_normal(self.positions[a], self.positions[b], self.positions[c]);
            for &v in &[a, b, c] {
                for k in 0..3 {
                    self.normals[v][k] += n[k];
                }
            }
        }
        for n in self.normals.iter_mut() {
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if l > 1e-12 {
                n[0] /= l;
                n[1] /= l;
                n[2] /= l;
            } else {
                *n = [0.0, 0.0, 1.0];
            }
        }
    }
}

/// Normale non normalisée (proportionnelle à l'aire).
pub(crate) fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]]
}

/// Ajoute un quadrilatère (a,b,c,d) orienté vers `outward`.
pub(crate) fn quad_facing(m: &mut Mesh, q: [u32; 4], outward: [f32; 3]) {
    let p = |i: u32| m.positions[i as usize];
    // Normale du quadrilatère entier (produit des diagonales) : fiable même s'il est plié.
    let (a, b, c, d) = (p(q[0]), p(q[1]), p(q[2]), p(q[3]));
    let u = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let v = [d[0] - b[0], d[1] - b[1], d[2] - b[2]];
    let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let dot = n[0] * outward[0] + n[1] * outward[1] + n[2] * outward[2];
    if dot >= 0.0 {
        m.indices.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
    } else {
        m.indices.extend_from_slice(&[q[0], q[2], q[1], q[0], q[3], q[2]]);
    }
}

/// Grille de travail tirée de l'image : couleurs réduites, masque de l'objet, luminosité.
pub struct Grid {
    pub w: usize,
    pub h: usize,
    pub mask: Vec<bool>,
    pub lum: Vec<f32>,
}

pub fn grid_size(img_w: u32, img_h: u32, resolution: u32) -> (usize, usize) {
    let res = resolution.clamp(8, 2048) as f32;
    let longest = img_w.max(img_h).max(1) as f32;
    let scale = (res / longest).min(1.0);
    let gw = ((img_w as f32 * scale).round() as usize).max(2);
    let gh = ((img_h as f32 * scale).round() as usize).max(2);
    (gw, gh)
}

pub fn has_transparency(img: &RgbaImage) -> bool {
    // Une vraie transparence (fond détouré), pas un léger fondu sur les bords de la photo.
    let clear = img.pixels().filter(|p| p.0[3] < 128).count();
    clear * 100 > (img.width() as usize * img.height() as usize).max(1)
}

pub fn make_grid(img: &RgbaImage, s: &Settings) -> Grid {
    let (gw, gh) = grid_size(img.width(), img.height(), s.resolution);
    let small = if gw as u32 == img.width() && gh as u32 == img.height() {
        img.clone()
    } else {
        image::imageops::resize(img, gw as u32, gh as u32, FilterType::Triangle)
    };
    let transparent = has_transparency(img);

    let background = match s.background {
        Background::Auto => {
            if transparent {
                Background::Alpha
            } else if s.mode == Mode::Relief {
                Background::None
            } else {
                Background::Color
            }
        }
        b => b,
    };

    let px = |i: usize, j: usize| small.get_pixel(i as u32, j as u32).0;
    let mut mask = vec![true; gw * gh];
    match background {
        Background::None | Background::Auto => {}
        Background::Alpha => {
            for j in 0..gh {
                for i in 0..gw {
                    mask[j * gw + i] = px(i, j)[3] >= 128;
                }
            }
        }
        Background::Color => {
            // Couleur du fond : médiane des pixels du bord.
            let mut border: Vec<[u8; 4]> = Vec::new();
            for i in 0..gw {
                border.push(px(i, 0));
                border.push(px(i, gh - 1));
            }
            for j in 0..gh {
                border.push(px(0, j));
                border.push(px(gw - 1, j));
            }
            let mut bg = [0f32; 3];
            for k in 0..3 {
                let mut ch: Vec<u8> = border.iter().map(|p| p[k]).collect();
                ch.sort_unstable();
                bg[k] = ch[ch.len() / 2] as f32;
            }
            let thr = s.threshold.max(1.0);
            for j in 0..gh {
                for i in 0..gw {
                    let p = px(i, j);
                    let d = ((p[0] as f32 - bg[0]).powi(2)
                        + (p[1] as f32 - bg[1]).powi(2)
                        + (p[2] as f32 - bg[2]).powi(2))
                    .sqrt();
                    mask[j * gw + i] = d > thr && p[3] >= 128;
                }
            }
            // Fond dégradé (studio, vignettage) relié au bord : retiré aussi.
            let flood = crate::multiview::background_flood(&small, s.threshold);
            for (m, b) in mask.iter_mut().zip(flood) {
                if b {
                    *m = false;
                }
            }
        }
    }
    // Retire les points isolés (bruit).
    if background != Background::None {
        let copy = mask.clone();
        for j in 0..gh {
            for i in 0..gw {
                if !copy[j * gw + i] {
                    continue;
                }
                let mut n = 0;
                for (di, dj) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                    let (x, y) = (i as i32 + di, j as i32 + dj);
                    if x >= 0 && y >= 0 && (x as usize) < gw && (y as usize) < gh && copy[y as usize * gw + x as usize] {
                        n += 1;
                    }
                }
                if n == 0 {
                    mask[j * gw + i] = false;
                }
            }
        }
    }

    let mut lum: Vec<f32> = (0..gw * gh)
        .map(|k| {
            let p = px(k % gw, k / gw);
            let l = (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0;
            if s.invert { 1.0 - l } else { l }
        })
        .collect();
    for _ in 0..s.smooth.min(20) {
        let src = lum.clone();
        for j in 0..gh {
            for i in 0..gw {
                let mut sum = 0.0;
                let mut n = 0.0;
                for dj in -1i32..=1 {
                    for di in -1i32..=1 {
                        let (x, y) = (i as i32 + di, j as i32 + dj);
                        if x >= 0 && y >= 0 && (x as usize) < gw && (y as usize) < gh {
                            sum += src[y as usize * gw + x as usize];
                            n += 1.0;
                        }
                    }
                }
                lum[j * gw + i] = sum / n;
            }
        }
    }
    Grid { w: gw, h: gh, mask, lum }
}

/// Distance (en cellules) de chaque cellule de l'objet jusqu'au bord de l'objet.
fn distance_field(g: &Grid) -> Vec<f32> {
    let (w, h) = (g.w, g.h);
    let big = 1e9f32;
    let mut d: Vec<f32> = g.mask.iter().map(|&m| if m { big } else { 0.0 }).collect();
    let inside = |i: i32, j: i32| i >= 0 && j >= 0 && (i as usize) < w && (j as usize) < h;
    let get = |d: &Vec<f32>, i: i32, j: i32| if inside(i, j) { d[j as usize * w + i as usize] } else { 0.0 };
    let diag = std::f32::consts::SQRT_2;
    for j in 0..h as i32 {
        for i in 0..w as i32 {
            let k = j as usize * w + i as usize;
            if d[k] == 0.0 {
                continue;
            }
            let v = (get(&d, i - 1, j) + 1.0)
                .min(get(&d, i, j - 1) + 1.0)
                .min(get(&d, i - 1, j - 1) + diag)
                .min(get(&d, i + 1, j - 1) + diag);
            d[k] = d[k].min(v);
        }
    }
    for j in (0..h as i32).rev() {
        for i in (0..w as i32).rev() {
            let k = j as usize * w + i as usize;
            if d[k] == 0.0 {
                continue;
            }
            let v = (get(&d, i + 1, j) + 1.0)
                .min(get(&d, i, j + 1) + 1.0)
                .min(get(&d, i + 1, j + 1) + diag)
                .min(get(&d, i - 1, j + 1) + diag);
            d[k] = d[k].min(v);
        }
    }
    // Adoucit les arêtes du squelette pour une surface bien ronde.
    for _ in 0..4 {
        let src = d.clone();
        for j in 0..h as i32 {
            for i in 0..w as i32 {
                let k = j as usize * w + i as usize;
                if !g.mask[k] {
                    continue;
                }
                let mut sum = 0.0;
                for dj in -1..=1 {
                    for di in -1..=1 {
                        sum += get(&src, i + di, j + dj);
                    }
                }
                d[k] = (sum / 9.0).max(0.5);
            }
        }
    }
    d
}

pub fn build(img: &RgbaImage, s: &Settings) -> Result<Mesh, String> {
    if s.mode == Mode::Multiview {
        return crate::multiview::from_images(vec![("image".into(), img.clone())], &[], s).map(|r| r.mesh);
    }
    let g = make_grid(img, s);
    build_from_grid(&g, img.width(), img.height(), s)
}

pub fn build_from_grid(g: &Grid, img_w: u32, img_h: u32, s: &Settings) -> Result<Mesh, String> {
    let (gw, gh) = (g.w, g.h);
    if !g.mask.iter().any(|&m| m) {
        return Err("aucun objet détecté dans l'image (essayez fond = aucun ou un seuil plus bas)".into());
    }
    let size = s.size.max(0.001);
    let (sx, sy) = if img_w >= img_h {
        (size, size * img_h as f32 / img_w.max(1) as f32)
    } else {
        (size * img_w as f32 / img_h.max(1) as f32, size)
    };
    let depth = s.depth.max(0.0);
    let cw = gw + 1;
    let cell_in = |i: i32, j: i32| -> bool {
        i >= 0 && j >= 0 && (i as usize) < gw && (j as usize) < gh && g.mask[j as usize * gw + i as usize]
    };

    let dist = if s.mode == Mode::Inflate { distance_field(g) } else { Vec::new() };
    let max_dist = dist.iter().cloned().fold(0.0f32, f32::max).max(1.0);

    // Hauteurs avant/arrière de chaque coin de cellule.
    let n_corners = cw * (gh + 1);
    let mut active = vec![false; n_corners];
    let mut boundary = vec![false; n_corners];
    let mut front = vec![0f32; n_corners];
    let mut back = vec![0f32; n_corners];
    for j in 0..=gh {
        for i in 0..=gw {
            let c = j * cw + i;
            let adj = [(i as i32 - 1, j as i32 - 1), (i as i32, j as i32 - 1), (i as i32 - 1, j as i32), (i as i32, j as i32)];
            let mut n_in = 0;
            let mut lum = 0.0;
            let mut dsum = 0.0;
            for &(x, y) in &adj {
                if cell_in(x, y) {
                    n_in += 1;
                    let k = y as usize * gw + x as usize;
                    lum += g.lum[k];
                    if !dist.is_empty() {
                        dsum += dist[k];
                    }
                }
            }
            if n_in == 0 {
                continue;
            }
            active[c] = true;
            boundary[c] = n_in < 4;
            let lum = lum / n_in as f32;
            match s.mode {
                Mode::Relief => {
                    front[c] = s.base + lum * depth;
                    back[c] = 0.0;
                }
                Mode::Extrude => {
                    front[c] = depth * 0.5;
                    back[c] = -depth * 0.5;
                }
                Mode::Inflate | Mode::Multiview => {
                    // Le bord garde une fine épaisseur : avant et arrière ne se touchent jamais.
                    let d = if boundary[c] { 0.3 } else { (dsum / 4.0 - 0.5).max(0.3) };
                    let t = (d / max_dist).clamp(0.0, 1.0);
                    // Profil arrondi (coussin) ; la luminosité ajoute un léger détail.
                    let h = depth * 0.5 * t.sqrt() * (1.0 + s.detail * (lum - 0.5));
                    front[c] = h;
                    back[c] = -h * s.back_scale;
                }
            }
        }
    }

    // Position de chaque coin dans la grille ; le contour est adouci pour effacer les marches d'escalier.
    let mut xy: Vec<[f32; 2]> = (0..n_corners).map(|c| [(c % cw) as f32, (c / cw) as f32]).collect();
    if g.mask.iter().any(|&m| !m) {
        let mut nb: Vec<Vec<usize>> = vec![Vec::new(); n_corners];
        for j in 0..=gh {
            for i in 0..=gw {
                let c = j * cw + i;
                if i < gw && cell_in(i as i32, j as i32 - 1) != cell_in(i as i32, j as i32) {
                    nb[c].push(c + 1);
                    nb[c + 1].push(c);
                }
                if j < gh && cell_in(i as i32 - 1, j as i32) != cell_in(i as i32, j as i32) {
                    nb[c].push(c + cw);
                    nb[c + cw].push(c);
                }
            }
        }
        for _ in 0..4 {
            let prev = xy.clone();
            for c in 0..n_corners {
                if nb[c].len() == 2 {
                    let (a, b) = (prev[nb[c][0]], prev[nb[c][1]]);
                    xy[c] = [0.5 * prev[c][0] + 0.25 * (a[0] + b[0]), 0.5 * prev[c][1] + 0.25 * (a[1] + b[1])];
                }
            }
        }
    }

    let pos = |c: usize, z: f32| -> [f32; 3] {
        [(xy[c][0] / gw as f32 - 0.5) * sx, (0.5 - xy[c][1] / gh as f32) * sy, z]
    };
    let uv = |c: usize| -> [f32; 2] { [xy[c][0] / gw as f32, xy[c][1] / gh as f32] };

    let mut m = Mesh::default();
    let mut fidx = vec![u32::MAX; n_corners];
    let mut bidx = vec![u32::MAX; n_corners];
    for c in 0..n_corners {
        if active[c] {
            fidx[c] = m.push(pos(c, front[c]), uv(c));
            bidx[c] = m.push(pos(c, back[c]), uv(c));
        }
    }

    for j in 0..gh {
        for i in 0..gw {
            if !g.mask[j * gw + i] {
                continue;
            }
            let tl = j * cw + i;
            let tr = tl + 1;
            let bl = tl + cw;
            let br = bl + 1;
            // Face avant (vers +Z) puis face arrière (vers -Z).
            m.indices.extend_from_slice(&[fidx[tl], fidx[bl], fidx[br], fidx[tl], fidx[br], fidx[tr]]);
            m.indices.extend_from_slice(&[bidx[tl], bidx[br], bidx[bl], bidx[tl], bidx[tr], bidx[br]]);

            // Murs sur les bords de l'objet.
            let sides: [((i32, i32), (usize, usize), (usize, usize), [f32; 3]); 4] = [
                ((-1, 0), (i, j), (i, j + 1), [-1.0, 0.0, 0.0]),
                ((1, 0), (i + 1, j), (i + 1, j + 1), [1.0, 0.0, 0.0]),
                ((0, -1), (i, j), (i + 1, j), [0.0, 1.0, 0.0]),
                ((0, 1), (i, j + 1), (i + 1, j + 1), [0.0, -1.0, 0.0]),
            ];
            for ((di, dj), a, b, out) in sides {
                if cell_in(i as i32 + di, j as i32 + dj) {
                    continue;
                }
                let ca = a.1 * cw + a.0;
                let cb = b.1 * cw + b.0;
                let af = m.push(pos(ca, front[ca]), uv(ca));
                let bf = m.push(pos(cb, front[cb]), uv(cb));
                let bb = m.push(pos(cb, back[cb]), uv(cb));
                let ab = m.push(pos(ca, back[ca]), uv(ca));
                quad_facing(&mut m, [af, bf, bb, ab], out);
            }
        }
    }

    m.compute_normals();
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn disc() -> RgbaImage {
        RgbaImage::from_fn(64, 48, |x, y| {
            let (dx, dy) = (x as f32 - 32.0, y as f32 - 24.0);
            if dx * dx + dy * dy < 400.0 { Rgba([200, 60, 40, 255]) } else { Rgba([255, 255, 255, 255]) }
        })
    }

    /// Chaque arête doit être partagée par exactement deux triangles (maillage fermé).
    fn is_watertight(m: &Mesh) -> bool {
        use std::collections::HashMap;
        let key = |p: [f32; 3]| (p[0].to_bits(), p[1].to_bits(), p[2].to_bits());
        let mut edges: HashMap<((u32, u32, u32), (u32, u32, u32)), i32> = HashMap::new();
        for t in m.indices.chunks_exact(3) {
            for e in 0..3 {
                let a = key(m.positions[t[e] as usize]);
                let b = key(m.positions[t[(e + 1) % 3] as usize]);
                let k = if a < b { (a, b) } else { (b, a) };
                *edges.entry(k).or_insert(0) += 1;
            }
        }
        edges.values().all(|&n| n == 2)
    }

    #[test]
    fn all_modes_closed() {
        for mode in [Mode::Relief, Mode::Extrude, Mode::Inflate] {
            let s = Settings { mode, ..Settings::default() };
            let m = build(&disc(), &s).unwrap();
            assert!(m.triangle_count() > 100, "{mode:?}");
            assert!(is_watertight(&m), "{mode:?} pas fermé");
        }
    }
}
