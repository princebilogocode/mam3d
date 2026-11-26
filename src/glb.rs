//! Lecture d'un fichier GLB (glTF binaire) : maillages (avec les transformations des nœuds)
//! et texture de couleur. Sert à afficher, redimensionner et convertir les modèles reçus de
//! l'IA en ligne.

use crate::mesh::Mesh;
use image::RgbaImage;
use serde_json::Value;

type Mat = [f32; 16];

const ID: Mat = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// Produit de matrices colonne-major (comme glTF).
fn mul(a: &Mat, b: &Mat) -> Mat {
    let mut r = [0f32; 16];
    for c in 0..4 {
        for row in 0..4 {
            r[c * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[c * 4 + k]).sum();
        }
    }
    r
}

fn node_matrix(n: &Value) -> Mat {
    if let Some(m) = n.get("matrix").and_then(|v| v.as_array()) {
        if m.len() == 16 {
            let mut r = [0f32; 16];
            for (i, v) in m.iter().enumerate() {
                r[i] = v.as_f64().unwrap_or(0.0) as f32;
            }
            return r;
        }
    }
    let arr = |k: &str, d: &[f32]| -> Vec<f32> {
        n.get(k).and_then(|v| v.as_array()).map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect()).unwrap_or_else(|| d.to_vec())
    };
    let t = arr("translation", &[0.0, 0.0, 0.0]);
    let q = arr("rotation", &[0.0, 0.0, 0.0, 1.0]);
    let s = arr("scale", &[1.0, 1.0, 1.0]);
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let r = [
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y + z * w),
        2.0 * (x * z - y * w),
        0.0,
        2.0 * (x * y - z * w),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z + x * w),
        0.0,
        2.0 * (x * z + y * w),
        2.0 * (y * z - x * w),
        1.0 - 2.0 * (x * x + y * y),
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ];
    let sm = [s[0], 0.0, 0.0, 0.0, 0.0, s[1], 0.0, 0.0, 0.0, 0.0, s[2], 0.0, 0.0, 0.0, 0.0, 1.0];
    let tm = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, t[0], t[1], t[2], 1.0];
    mul(&tm, &mul(&r, &sm))
}

struct Gltf<'a> {
    json: Value,
    bin: &'a [u8],
}

impl Gltf<'_> {
    fn view(&self, i: usize) -> Result<(&[u8], usize), String> {
        let v = &self.json["bufferViews"][i];
        let off = v["byteOffset"].as_u64().unwrap_or(0) as usize;
        let len = v["byteLength"].as_u64().ok_or("bufferView sans longueur")? as usize;
        let stride = v["byteStride"].as_u64().unwrap_or(0) as usize;
        let data = self.bin.get(off..off + len).ok_or("bufferView hors du fichier")?;
        Ok((data, stride))
    }

    /// Lit un accesseur en f32 (composantes par élément), entiers normalisés acceptés.
    fn floats(&self, i: usize) -> Result<(Vec<f32>, usize), String> {
        let a = &self.json["accessors"][i];
        let count = a["count"].as_u64().ok_or("accesseur sans nombre")? as usize;
        let comps = match a["type"].as_str().unwrap_or("") {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            t => return Err(format!("type d'accesseur non géré : {t}")),
        };
        let ct = a["componentType"].as_u64().unwrap_or(5126);
        let size = match ct {
            5126 | 5125 => 4,
            5123 | 5122 => 2,
            _ => 1,
        };
        let vi = a["bufferView"].as_u64().ok_or("accesseur sans bufferView (compressé ?)")? as usize;
        let (data, stride) = self.view(vi)?;
        let base = a["byteOffset"].as_u64().unwrap_or(0) as usize;
        let stride = if stride == 0 { comps * size } else { stride };
        let normalized = a["normalized"].as_bool().unwrap_or(false);
        let mut out = Vec::with_capacity(count * comps);
        for e in 0..count {
            for c in 0..comps {
                let o = base + e * stride + c * size;
                let b = data.get(o..o + size).ok_or("accesseur hors des données")?;
                let v = match ct {
                    5126 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                    5125 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32,
                    5123 => {
                        let v = u16::from_le_bytes([b[0], b[1]]) as f32;
                        if normalized { v / 65535.0 } else { v }
                    }
                    5122 => {
                        let v = i16::from_le_bytes([b[0], b[1]]) as f32;
                        if normalized { (v / 32767.0).max(-1.0) } else { v }
                    }
                    5121 => {
                        let v = b[0] as f32;
                        if normalized { v / 255.0 } else { v }
                    }
                    _ => {
                        let v = b[0] as i8 as f32;
                        if normalized { (v / 127.0).max(-1.0) } else { v }
                    }
                };
                out.push(v);
            }
        }
        Ok((out, comps))
    }

    fn indices(&self, i: usize) -> Result<Vec<u32>, String> {
        let a = &self.json["accessors"][i];
        let count = a["count"].as_u64().unwrap_or(0) as usize;
        let ct = a["componentType"].as_u64().unwrap_or(5125);
        let size = match ct {
            5125 => 4,
            5123 => 2,
            _ => 1,
        };
        let vi = a["bufferView"].as_u64().ok_or("indices sans bufferView")? as usize;
        let (data, stride) = self.view(vi)?;
        let base = a["byteOffset"].as_u64().unwrap_or(0) as usize;
        let stride = if stride == 0 { size } else { stride };
        (0..count)
            .map(|e| {
                let o = base + e * stride;
                let b = data.get(o..o + size).ok_or_else(|| "indices hors des données".to_string())?;
                Ok(match size {
                    4 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                    2 => u16::from_le_bytes([b[0], b[1]]) as u32,
                    _ => b[0] as u32,
                })
            })
            .collect()
    }

    fn image(&self, texture: usize) -> Option<RgbaImage> {
        let src = self.json["textures"][texture]["source"].as_u64()? as usize;
        let img = &self.json["images"][src];
        let vi = img["bufferView"].as_u64()? as usize;
        let (data, _) = self.view(vi).ok()?;
        image::load_from_memory(data).ok().map(|i| i.to_rgba8())
    }
}

/// Lit un GLB : tous les triangles de la scène (transformations appliquées) et la texture de
/// couleur du premier matériau texturé.
pub fn read(bytes: &[u8]) -> Result<(Mesh, Option<RgbaImage>), String> {
    if bytes.len() < 20 || &bytes[0..4] != b"glTF" {
        return Err("ce n'est pas un fichier GLB".into());
    }
    let jlen = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
    let json: Value = serde_json::from_slice(bytes.get(20..20 + jlen).ok_or("GLB tronqué")?).map_err(|e| format!("GLB : {e}"))?;
    let mut bin: &[u8] = &[];
    let boff = 20 + jlen;
    if bytes.len() >= boff + 8 {
        let blen = u32::from_le_bytes([bytes[boff], bytes[boff + 1], bytes[boff + 2], bytes[boff + 3]]) as usize;
        bin = bytes.get(boff + 8..boff + 8 + blen).unwrap_or(&[]);
    }
    if let Some(req) = json["extensionsRequired"].as_array() {
        if let Some(e) = req.iter().filter_map(|v| v.as_str()).find(|e| e.contains("draco") || e.contains("meshopt")) {
            return Err(format!("maillage compressé ({e}) non lisible"));
        }
    }
    let g = Gltf { json, bin };
    let mut mesh = Mesh::default();
    let mut texture: Option<RgbaImage> = None;

    // Parcours des nœuds de la scène avec leurs transformations.
    let scene = g.json["scene"].as_u64().unwrap_or(0) as usize;
    let roots: Vec<usize> = g.json["scenes"][scene]["nodes"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_u64()).map(|v| v as usize).collect())
        .unwrap_or_else(|| (0..g.json["nodes"].as_array().map(|a| a.len()).unwrap_or(0)).collect());
    let mut stack: Vec<(usize, Mat)> = roots.into_iter().map(|n| (n, ID)).collect();
    let mut guard = 0;
    while let Some((ni, parent)) = stack.pop() {
        guard += 1;
        if guard > 100_000 {
            break;
        }
        let node = &g.json["nodes"][ni];
        let m = mul(&parent, &node_matrix(node));
        if let Some(children) = node["children"].as_array() {
            for c in children.iter().filter_map(|v| v.as_u64()) {
                stack.push((c as usize, m));
            }
        }
        let Some(mi) = node["mesh"].as_u64() else { continue };
        let prims = g.json["meshes"][mi as usize]["primitives"].as_array().cloned().unwrap_or_default();
        for p in prims {
            if p["mode"].as_u64().unwrap_or(4) != 4 {
                continue;
            }
            let Some(pa) = p["attributes"]["POSITION"].as_u64() else { continue };
            let (pos, _) = g.floats(pa as usize)?;
            let n = pos.len() / 3;
            let uvs = match p["attributes"]["TEXCOORD_0"].as_u64() {
                Some(a) => g.floats(a as usize)?.0,
                None => vec![0.0; n * 2],
            };
            if texture.is_none() {
                if let Some(mat) = p["material"].as_u64() {
                    if let Some(t) = g.json["materials"][mat as usize]["pbrMetallicRoughness"]["baseColorTexture"]["index"].as_u64() {
                        texture = g.image(t as usize);
                    }
                }
            }
            let start = mesh.positions.len() as u32;
            for v in 0..n {
                let (x, y, z) = (pos[v * 3], pos[v * 3 + 1], pos[v * 3 + 2]);
                let w = [m[0] * x + m[4] * y + m[8] * z + m[12], m[1] * x + m[5] * y + m[9] * z + m[13], m[2] * x + m[6] * y + m[10] * z + m[14]];
                mesh.push(w, [uvs.get(v * 2).copied().unwrap_or(0.0), uvs.get(v * 2 + 1).copied().unwrap_or(0.0)]);
            }
            let idx = match p["indices"].as_u64() {
                Some(a) => g.indices(a as usize)?,
                None => (0..n as u32).collect(),
            };
            // Une transformation miroir inverse le sens des faces.
            let det = m[0] * (m[5] * m[10] - m[9] * m[6]) - m[4] * (m[1] * m[10] - m[9] * m[2]) + m[8] * (m[1] * m[6] - m[5] * m[2]);
            for t in idx.chunks_exact(3) {
                if det < 0.0 {
                    mesh.indices.extend_from_slice(&[start + t[0], start + t[2], start + t[1]]);
                } else {
                    mesh.indices.extend_from_slice(&[start + t[0], start + t[1], start + t[2]]);
                }
            }
        }
    }
    if mesh.indices.is_empty() {
        return Err("aucun triangle dans le fichier".into());
    }
    mesh.compute_normals();
    Ok((mesh, texture))
}

/// Met le modèle à la taille voulue (plus grande dimension), posé au sol, centré.
pub fn fit(mesh: &mut Mesh, size: f32) {
    let (min, max) = mesh.bounds();
    let ext = (max[0] - min[0]).max(max[1] - min[1]).max(max[2] - min[2]).max(1e-6);
    let k = size.max(0.001) / ext;
    let cx = (min[0] + max[0]) * 0.5;
    let cz = (min[2] + max[2]) * 0.5;
    for p in mesh.positions.iter_mut() {
        *p = [(p[0] - cx) * k, (p[1] - min[1]) * k, (p[2] - cz) * k];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_back_our_own_glb() {
        let img = RgbaImage::from_fn(40, 40, |x, y| {
            if (x as i32 - 20).pow(2) + (y as i32 - 20).pow(2) < 225 { image::Rgba([200, 30, 30, 255]) } else { image::Rgba([0, 0, 0, 0]) }
        });
        let s = crate::settings::Settings::default();
        let m = crate::mesh::build(&img, &s).unwrap();
        let dir = std::env::temp_dir().join("mam3d_test_glb");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.glb");
        let tex = crate::export::texture_png(&img, 64).unwrap();
        crate::export::write_glb(&p, &m, "t", Some(&tex)).unwrap();
        let (r, t) = read(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(r.triangle_count(), m.triangle_count());
        assert!(t.is_some());
    }
}
