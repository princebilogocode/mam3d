//! Écriture des fichiers 3D : GLB (glTF binaire), OBJ + MTL + PNG, STL binaire.

use crate::mesh::Mesh;
use image::{RgbaImage, imageops::FilterType};
use std::fs::File;
use std::io::{BufWriter, Cursor, Write};
use std::path::{Path, PathBuf};

pub fn texture_png(img: &RgbaImage, max_side: u32) -> Result<Vec<u8>, String> {
    let max_side = max_side.clamp(16, 8192);
    let (w, h) = img.dimensions();
    let scaled;
    let tex = if w.max(h) > max_side {
        let s = max_side as f32 / w.max(h) as f32;
        scaled = image::imageops::resize(
            img,
            ((w as f32 * s).round() as u32).max(1),
            ((h as f32 * s).round() as u32).max(1),
            FilterType::Triangle,
        );
        &scaled
    } else {
        img
    };
    let mut out = Vec::new();
    tex.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|e| format!("encodage de la texture : {e}"))?;
    Ok(out)
}

fn pad4(v: &mut Vec<u8>, byte: u8) {
    while v.len() % 4 != 0 {
        v.push(byte);
    }
}

/// glTF 2.0 binaire, avec la texture intégrée. Blender : Fichier > Importer > glTF 2.0.
pub fn write_glb(path: &Path, mesh: &Mesh, name: &str, texture: Option<&[u8]>) -> Result<(), String> {
    let mut bin: Vec<u8> = Vec::with_capacity(mesh.positions.len() * 32 + mesh.indices.len() * 4);
    let mut views = Vec::new();
    let mut add_view = |bin: &mut Vec<u8>, bytes: &[u8], target: Option<u32>| -> usize {
        pad4(bin, 0);
        let offset = bin.len();
        bin.extend_from_slice(bytes);
        let mut v = serde_json::json!({"buffer": 0, "byteOffset": offset, "byteLength": bytes.len()});
        if let Some(t) = target {
            v["target"] = serde_json::json!(t);
        }
        views.push(v);
        views.len() - 1
    };
    let f32s = |data: &mut dyn Iterator<Item = f32>| -> Vec<u8> { data.flat_map(|f| f.to_le_bytes()).collect() };

    let pos_bytes = f32s(&mut mesh.positions.iter().flat_map(|p| p.iter().copied()));
    let nrm_bytes = f32s(&mut mesh.normals.iter().flat_map(|p| p.iter().copied()));
    let uv_bytes = f32s(&mut mesh.uvs.iter().flat_map(|p| p.iter().copied()));
    let idx_bytes: Vec<u8> = mesh.indices.iter().flat_map(|i| i.to_le_bytes()).collect();

    let v_pos = add_view(&mut bin, &pos_bytes, Some(34962));
    let v_nrm = add_view(&mut bin, &nrm_bytes, Some(34962));
    let v_uv = add_view(&mut bin, &uv_bytes, Some(34962));
    let v_idx = add_view(&mut bin, &idx_bytes, Some(34963));
    let v_img = texture.map(|t| add_view(&mut bin, t, None));
    pad4(&mut bin, 0);

    let (min, max) = mesh.bounds();
    let count = mesh.positions.len();
    let mut material = serde_json::json!({
        "name": format!("{name}_matiere"),
        "pbrMetallicRoughness": {"metallicFactor": 0.0, "roughnessFactor": 0.8},
        "doubleSided": false
    });
    let mut root = serde_json::json!({
        "asset": {"version": "2.0", "generator": "mam3d"},
        "scene": 0,
        "scenes": [{"name": name, "nodes": [0]}],
        "nodes": [{"name": name, "mesh": 0}],
        "meshes": [{"name": name, "primitives": [{
            "attributes": {"POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2},
            "indices": 3, "material": 0, "mode": 4
        }]}],
        "accessors": [
            {"bufferView": v_pos, "componentType": 5126, "count": count, "type": "VEC3", "min": min, "max": max},
            {"bufferView": v_nrm, "componentType": 5126, "count": count, "type": "VEC3"},
            {"bufferView": v_uv, "componentType": 5126, "count": count, "type": "VEC2"},
            {"bufferView": v_idx, "componentType": 5125, "count": mesh.indices.len(), "type": "SCALAR"}
        ],
        "bufferViews": views,
        "buffers": [{"byteLength": bin.len()}]
    });
    if let Some(vi) = v_img {
        material["pbrMetallicRoughness"]["baseColorTexture"] = serde_json::json!({"index": 0});
        root["images"] = serde_json::json!([{"name": format!("{name}_texture"), "bufferView": vi, "mimeType": "image/png"}]);
        root["samplers"] = serde_json::json!([{"magFilter": 9729, "minFilter": 9987, "wrapS": 33071, "wrapT": 33071}]);
        root["textures"] = serde_json::json!([{"source": 0, "sampler": 0}]);
    }
    root["materials"] = serde_json::json!([material]);

    let mut json = serde_json::to_vec(&root).map_err(|e| e.to_string())?;
    pad4(&mut json, b' ');
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut w = BufWriter::new(File::create(path).map_err(|e| format!("{}: {e}", path.display()))?);
    let io = |e: std::io::Error| e.to_string();
    w.write_all(b"glTF").map_err(io)?;
    w.write_all(&2u32.to_le_bytes()).map_err(io)?;
    w.write_all(&(total as u32).to_le_bytes()).map_err(io)?;
    w.write_all(&(json.len() as u32).to_le_bytes()).map_err(io)?;
    w.write_all(b"JSON").map_err(io)?;
    w.write_all(&json).map_err(io)?;
    w.write_all(&(bin.len() as u32).to_le_bytes()).map_err(io)?;
    w.write_all(b"BIN\0").map_err(io)?;
    w.write_all(&bin).map_err(io)?;
    w.flush().map_err(io)?;
    Ok(())
}

/// OBJ + MTL + texture PNG. Renvoie les fichiers écrits.
pub fn write_obj(path: &Path, mesh: &Mesh, name: &str, texture: Option<&[u8]>) -> Result<Vec<PathBuf>, String> {
    let io = |e: std::io::Error| e.to_string();
    let mut written = vec![path.to_path_buf()];
    let mtl_path = path.with_extension("mtl");
    let tex_name = format!("{name}_texture.png");
    let mtl_name = mtl_path.file_name().unwrap().to_string_lossy().to_string();
    {
        let mut m = BufWriter::new(File::create(&mtl_path).map_err(io)?);
        writeln!(m, "# mam3d\nnewmtl {name}_matiere\nKa 1 1 1\nKd 1 1 1\nKs 0 0 0\nd 1\nillum 1").map_err(io)?;
        if texture.is_some() {
            writeln!(m, "map_Kd {tex_name}").map_err(io)?;
        }
        m.flush().map_err(io)?;
    }
    written.push(mtl_path.clone());
    if let Some(t) = texture {
        let tp = path.with_file_name(&tex_name);
        std::fs::write(&tp, t).map_err(io)?;
        written.push(tp);
    }
    let mut w = BufWriter::with_capacity(1 << 20, File::create(path).map_err(io)?);
    writeln!(w, "# mam3d - {} sommets, {} triangles", mesh.positions.len(), mesh.triangle_count()).map_err(io)?;
    writeln!(w, "mtllib {mtl_name}\no {name}").map_err(io)?;
    for p in &mesh.positions {
        writeln!(w, "v {:.5} {:.5} {:.5}", p[0], p[1], p[2]).map_err(io)?;
    }
    for t in &mesh.uvs {
        writeln!(w, "vt {:.5} {:.5}", t[0], 1.0 - t[1]).map_err(io)?;
    }
    for n in &mesh.normals {
        writeln!(w, "vn {:.4} {:.4} {:.4}", n[0], n[1], n[2]).map_err(io)?;
    }
    writeln!(w, "usemtl {name}_matiere\ns 1").map_err(io)?;
    for t in mesh.indices.chunks_exact(3) {
        let (a, b, c) = (t[0] + 1, t[1] + 1, t[2] + 1);
        writeln!(w, "f {a}/{a}/{a} {b}/{b}/{b} {c}/{c}/{c}").map_err(io)?;
    }
    w.flush().map_err(io)?;
    Ok(written)
}

/// STL binaire, converti en Z vers le haut (convention de Blender pour le STL).
pub fn write_stl(path: &Path, mesh: &Mesh) -> Result<(), String> {
    let io = |e: std::io::Error| e.to_string();
    let mut w = BufWriter::with_capacity(1 << 20, File::create(path).map_err(io)?);
    let mut header = [0u8; 80];
    let tag = b"mam3d binary STL";
    header[..tag.len()].copy_from_slice(tag);
    w.write_all(&header).map_err(io)?;
    w.write_all(&(mesh.triangle_count() as u32).to_le_bytes()).map_err(io)?;
    let zup = |p: [f32; 3]| [p[0], -p[2], p[1]];
    for t in mesh.indices.chunks_exact(3) {
        let a = zup(mesh.positions[t[0] as usize]);
        let b = zup(mesh.positions[t[1] as usize]);
        let c = zup(mesh.positions[t[2] as usize]);
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let mut n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-20);
        n.iter_mut().for_each(|x| *x /= l);
        for vtx in [n, a, b, c] {
            for f in vtx {
                w.write_all(&f.to_le_bytes()).map_err(io)?;
            }
        }
        w.write_all(&[0, 0]).map_err(io)?;
    }
    w.flush().map_err(io)?;
    Ok(())
}
