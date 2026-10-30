//! Knowing the owner's face, for Gilgamesh's sight (lib/gilgamesh/sight.svelte.ts).
//!
//! The camera and face tracking run in the webview (MediaPipe). For a face to recognise, it
//! sends a small crop around the face and five points (eyes, nose tip, mouth corners). Here the
//! face is aligned to the standard 112×112 template and run through **SFace** (OpenCV Zoo,
//! Apache-2.0; a MobileFaceNet), giving 128 numbers on the unit sphere. Faces of the same person
//! are close (cosine similarity above ~0.36). The webview keeps the owner's numbers in the vault
//! and compares; no picture is kept anywhere.
//!
//! The model (39 MB) is downloaded once, checksum-verified, and run with tract (pure Rust, so it
//! doesn't meet the onnxruntime linked into sherpa-onnx).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::AsyncWriteExt;
use tract_onnx::prelude::*;

use crate::client::{CmdError, CmdResult};

const MODEL_URL: &str = "https://media.githubusercontent.com/media/opencv/opencv_zoo/ba91a3b91d00d76e86540d4013f944bd6b514e39/models/face_recognition_sface/face_recognition_sface_2021dec.onnx";
const MODEL_FILE: &str = "face_recognition_sface_2021dec.onnx";
const MODEL_SHA256: &str = "0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79";
const MODEL_BYTES: u64 = 38_696_353;
const SIZE: usize = 112;
/// Where the five points sit in an aligned face (ArcFace's template, used by SFace).
const TEMPLATE: [[f32; 2]; 5] = [[38.2946, 51.6963], [73.5318, 51.5014], [56.0252, 71.7366], [41.5493, 92.3655], [70.7299, 92.2041]];

type Model = Arc<TypedRunnableModel>;

#[derive(Default)]
pub struct FaceId {
    downloading: Mutex<Option<Value>>,
    model: Mutex<Option<Model>>,
}

fn dir(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().map(|d| d.join("models").join("faceid")).unwrap_or_else(|_| "models/faceid".into())
}

fn installed(app: &AppHandle) -> bool {
    std::fs::metadata(dir(app).join(MODEL_FILE)).is_ok_and(|m| m.len() == MODEL_BYTES)
}

fn load(path: &std::path::Path) -> TractResult<Model> {
    tract_onnx::onnx().model_for_path(path)?.with_input_fact(0, f32::fact([1, 3, SIZE, SIZE]).into())?.into_optimized()?.into_runnable()
}

fn model(app: &AppHandle, f: &FaceId) -> CmdResult<Model> {
    let mut m = f.model.lock().unwrap();
    if let Some(m) = &*m {
        return Ok(m.clone());
    }
    if !installed(app) {
        return Err(CmdError::new("faceid_missing", "Face recognition isn't set up yet"));
    }
    let loaded = load(&dir(app).join(MODEL_FILE)).map_err(|e| CmdError::new("faceid", format!("Couldn't load face recognition: {e}")))?;
    *m = Some(loaded.clone());
    Ok(loaded)
}

// ------------------------------------------------------------------ the maths

/// The similarity (scale, rotation, shift) that best maps `from` onto `to` (least squares):
/// x' = a·x − b·y + tx, y' = b·x + a·y + ty, as [a, b, tx, ty].
fn similarity(from: &[[f32; 2]; 5], to: &[[f32; 2]; 5]) -> Option<[f32; 4]> {
    let n = from.len() as f32;
    let mean = |p: &[[f32; 2]; 5]| [p.iter().map(|q| q[0]).sum::<f32>() / n, p.iter().map(|q| q[1]).sum::<f32>() / n];
    let (mf, mt) = (mean(from), mean(to));
    let (mut num_a, mut num_b, mut den) = (0f32, 0f32, 0f32);
    for (p, q) in from.iter().zip(to) {
        let (px, py) = (p[0] - mf[0], p[1] - mf[1]);
        let (qx, qy) = (q[0] - mt[0], q[1] - mt[1]);
        num_a += px * qx + py * qy;
        num_b += px * qy - py * qx;
        den += px * px + py * py;
    }
    if den < 1e-6 {
        return None;
    }
    let (a, b) = (num_a / den, num_b / den);
    Some([a, b, mt[0] - (a * mf[0] - b * mf[1]), mt[1] - (b * mf[0] + a * mf[1])])
}

/// The face, aligned: 112×112, RGB, channels first, 0–255 (what SFace expects).
fn align(rgba: &[u8], w: usize, h: usize, points: &[[f32; 2]; 5]) -> Option<Vec<f32>> {
    let [a, b, tx, ty] = similarity(points, &TEMPLATE)?;
    // Back from the template to the crop: the inverse of [a −b; b a].
    let d = a * a + b * b;
    let (ia, ib) = (a / d, b / d);
    let mut out = vec![0f32; 3 * SIZE * SIZE];
    let px = |x: i64, y: i64, c: usize| -> f32 {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            0.0
        } else {
            rgba[(y as usize * w + x as usize) * 4 + c] as f32
        }
    };
    for v in 0..SIZE {
        for u in 0..SIZE {
            let (qx, qy) = (u as f32 - tx, v as f32 - ty);
            let sx = ia * qx + ib * qy;
            let sy = -ib * qx + ia * qy;
            let (x0, y0) = (sx.floor(), sy.floor());
            let (fx, fy) = (sx - x0, sy - y0);
            let (x0, y0) = (x0 as i64, y0 as i64);
            for c in 0..3 {
                let top = px(x0, y0, c) * (1.0 - fx) + px(x0 + 1, y0, c) * fx;
                let bot = px(x0, y0 + 1, c) * (1.0 - fx) + px(x0 + 1, y0 + 1, c) * fx;
                out[c * SIZE * SIZE + v * SIZE + u] = top * (1.0 - fy) + bot * fy;
            }
        }
    }
    Some(out)
}

fn embed(model: &Model, input: Vec<f32>) -> CmdResult<Vec<f32>> {
    let t: Tensor = tract_ndarray::Array4::from_shape_vec((1, 3, SIZE, SIZE), input).map_err(|e| CmdError::new("faceid", e.to_string()))?.into();
    let out = model.run(tvec!(t.into())).map_err(|e| CmdError::new("faceid", e.to_string()))?;
    let v: Vec<f32> = out[0].to_plain_array_view::<f32>().map_err(|e| CmdError::new("faceid", e.to_string()))?.iter().copied().collect();
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    Ok(v.into_iter().map(|x| x / norm).collect())
}

/// The request: width and height (u32, little-endian), the five points (10 × f32, in the crop's
/// pixels: right eye, left eye, nose tip, right and left mouth corner, as seen in the picture),
/// then the crop's RGBA pixels.
fn parse(body: &[u8]) -> CmdResult<(usize, usize, [[f32; 2]; 5], &[u8])> {
    let bad = || CmdError::new("bad_request", "bad face picture");
    if body.len() < 48 {
        return Err(bad());
    }
    let u = |i: usize| u32::from_le_bytes(body[i..i + 4].try_into().unwrap()) as usize;
    let f = |i: usize| f32::from_le_bytes(body[i..i + 4].try_into().unwrap());
    let (w, h) = (u(0), u(4));
    if w == 0 || h == 0 || w > 1024 || h > 1024 || body.len() != 48 + w * h * 4 {
        return Err(bad());
    }
    let mut p = [[0f32; 2]; 5];
    for (i, q) in p.iter_mut().enumerate() {
        *q = [f(8 + i * 8), f(12 + i * 8)];
        if !q[0].is_finite() || !q[1].is_finite() {
            return Err(bad());
        }
    }
    Ok((w, h, p, &body[48..]))
}

// ------------------------------------------------------------------ commands

#[tauri::command]
pub fn faceid_status(app: AppHandle, f: State<'_, FaceId>) -> Value {
    json!({
        "installed": installed(&app),
        "bytes": MODEL_BYTES,
        "downloading": f.downloading.lock().unwrap().clone(),
    })
}

#[tauri::command]
pub async fn faceid_download(app: AppHandle, f: State<'_, FaceId>) -> CmdResult<()> {
    {
        let mut d = f.downloading.lock().unwrap();
        if d.is_some() {
            return Err(CmdError::new("busy", "A download is already running"));
        }
        *d = Some(json!({"done": 0, "total": MODEL_BYTES}));
    }
    let r = download(&app, &f).await;
    *f.downloading.lock().unwrap() = None;
    let _ = app.emit("faceid-progress", json!({"done": true, "error": r.as_ref().err().map(|e| e.message.clone())}));
    r
}

async fn download(app: &AppHandle, f: &FaceId) -> CmdResult<()> {
    let d = dir(app);
    tokio::fs::create_dir_all(&d).await.map_err(|e| CmdError::new("io", e.to_string()))?;
    let http = reqwest::Client::builder().proxy(rad_tunnel::route::reqwest_proxy()).connect_timeout(Duration::from_secs(15)).build().map_err(|e| CmdError::new("faceid", e.to_string()))?;
    let resp = http.get(MODEL_URL).send().await.map_err(|e| CmdError::new("offline", format!("Download failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(CmdError::new("faceid", format!("Download failed: HTTP {}", resp.status())));
    }
    let part = d.join(format!("{MODEL_FILE}.part"));
    let mut file = tokio::fs::File::create(&part).await.map_err(|e| CmdError::new("io", e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut stream = resp.bytes_stream();
    let (mut done, mut last) = (0u64, 0u64);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| CmdError::new("offline", format!("Download interrupted: {e}")))?;
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(|e| CmdError::new("io", e.to_string()))?;
        done += chunk.len() as u64;
        if done - last > 1_000_000 {
            last = done;
            let p = json!({"done": done, "total": MODEL_BYTES});
            *f.downloading.lock().unwrap() = Some(p.clone());
            let _ = app.emit("faceid-progress", p);
        }
    }
    file.flush().await.map_err(|e| CmdError::new("io", e.to_string()))?;
    drop(file);
    let hash: String = hasher.finalize().iter().map(|x| format!("{x:02x}")).collect();
    if hash != MODEL_SHA256 {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(CmdError::new("faceid", "The face recognition model didn't match its checksum — try again"));
    }
    tokio::fs::rename(&part, d.join(MODEL_FILE)).await.map_err(|e| CmdError::new("io", e.to_string()))
}

#[tauri::command]
pub async fn faceid_remove(app: AppHandle, f: State<'_, FaceId>) -> CmdResult<()> {
    *f.model.lock().unwrap() = None;
    let _ = tokio::fs::remove_file(dir(&app).join(MODEL_FILE)).await;
    Ok(())
}

/// A face's 128 numbers (see `parse` for the request).
#[tauri::command]
pub async fn faceid_embed(app: AppHandle, f: State<'_, FaceId>, request: tauri::ipc::Request<'_>) -> CmdResult<Vec<f32>> {
    let tauri::ipc::InvokeBody::Raw(body) = request.body() else {
        return Err(CmdError::new("bad_request", "send the face as bytes"));
    };
    let (w, h, points, rgba) = parse(body)?;
    let input = align(rgba, w, h, &points).ok_or_else(|| CmdError::new("bad_request", "the face's points are all in one place"))?;
    let m = model(&app, &f)?;
    tokio::task::spawn_blocking(move || embed(&m, input)).await.map_err(|e| CmdError::new("faceid", e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_finds_scale_rotation_and_shift() {
        // The template moved: scaled by 2.5, turned 20°, shifted.
        let (s, r) = (2.5f32, 20f32.to_radians());
        let (a, b) = (s * r.cos(), s * r.sin());
        let moved = TEMPLATE.map(|p| [a * p[0] - b * p[1] + 40.0, b * p[0] + a * p[1] - 15.0]);
        // Mapping the moved points back gives the template.
        let [ma, mb, tx, ty] = similarity(&moved, &TEMPLATE).unwrap();
        for (p, q) in moved.iter().zip(&TEMPLATE) {
            let (x, y) = (ma * p[0] - mb * p[1] + tx, mb * p[0] + ma * p[1] + ty);
            assert!((x - q[0]).abs() < 1e-3 && (y - q[1]).abs() < 1e-3, "{x},{y} vs {q:?}");
        }
        assert!(similarity(&[[1.0, 1.0]; 5], &TEMPLATE).is_none());
    }

    #[test]
    fn aligning_an_aligned_face_changes_nothing() {
        // A 112×112 picture whose points are already the template comes out the same.
        let rgba: Vec<u8> = (0..SIZE * SIZE).flat_map(|i| [(i % 251) as u8, (i % 13 * 19) as u8, (i / SIZE) as u8, 255]).collect();
        let out = align(&rgba, SIZE, SIZE, &TEMPLATE).unwrap();
        for i in 0..SIZE * SIZE {
            for c in 0..3 {
                assert!((out[c * SIZE * SIZE + i] - rgba[i * 4 + c] as f32).abs() < 0.01);
            }
        }
    }

    #[test]
    fn parses_the_request() {
        let mut b = Vec::new();
        b.extend(2u32.to_le_bytes());
        b.extend(1u32.to_le_bytes());
        for i in 0..10 {
            b.extend((i as f32).to_le_bytes());
        }
        b.extend([7u8; 8]);
        let (w, h, p, px) = parse(&b).unwrap();
        assert_eq!((w, h, p[4], px.len()), (2, 1, [8.0, 9.0], 8));
        b.pop();
        assert!(parse(&b).is_err());
    }

    /// The real model: a face and the same face scaled, turned and moved within a bigger picture
    /// give (nearly) the same numbers; a different picture doesn't.
    /// RAD_SFACE=/path/to/face_recognition_sface_2021dec.onnx cargo test -p rad-app --lib faceid -- --ignored
    #[test]
    #[ignore]
    fn same_face_same_numbers() {
        let m = load(std::path::Path::new(&std::env::var("RAD_SFACE").unwrap())).unwrap();
        let face: Vec<u8> = (0..SIZE * SIZE).flat_map(|i| {
            let (x, y) = ((i % SIZE) as f32, (i / SIZE) as f32);
            let v = (((x - 56.0).powi(2) + (y - 60.0).powi(2)).sqrt() * 3.0) as u8;
            [v, v.wrapping_mul(2), 255 - v, 255]
        }).collect();
        // The same face drawn at 1.6× scale, turned 10°, at (30, 20) in a 260×240 picture.
        let (s, r) = (1.6f32, 10f32.to_radians());
        let (a, b) = (s * r.cos(), s * r.sin());
        let (w, h) = (260usize, 240usize);
        let mut big = vec![0u8; w * h * 4];
        let d = a * a + b * b;
        for y in 0..h {
            for x in 0..w {
                let (qx, qy) = (x as f32 - 30.0, y as f32 - 20.0);
                let (sx, sy) = ((a * qx + b * qy) / d, (-b * qx + a * qy) / d);
                if sx >= 0.0 && sy >= 0.0 && (sx as usize) < SIZE && (sy as usize) < SIZE {
                    let i = (sy as usize * SIZE + sx as usize) * 4;
                    big[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&face[i..i + 4]);
                }
            }
        }
        let moved = TEMPLATE.map(|p| [a * p[0] - b * p[1] + 30.0, b * p[0] + a * p[1] + 20.0]);
        let e1 = embed(&m, align(&face, SIZE, SIZE, &TEMPLATE).unwrap()).unwrap();
        let e2 = embed(&m, align(&big, w, h, &moved).unwrap()).unwrap();
        let other: Vec<u8> = face.chunks(4).flat_map(|p| [p[2], p[0], p[1], 255]).collect();
        let e3 = embed(&m, align(&other, SIZE, SIZE, &TEMPLATE).unwrap()).unwrap();
        let cos = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        println!("same {:.3}, different {:.3}", cos(&e1, &e2), cos(&e1, &e3));
        assert_eq!(e1.len(), 128);
        assert!(cos(&e1, &e2) > 0.9);
        assert!(cos(&e1, &e3) < cos(&e1, &e2));
    }
}
