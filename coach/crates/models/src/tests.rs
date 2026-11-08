use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::*;

/// A tiny HTTP/1.1 server that serves `body`, honouring `Range: bytes=N-`.
fn serve(body: Vec<u8>, requests: usize) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let ranged = Arc::new(AtomicUsize::new(0));
    let ranged2 = ranged.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().take(requests) {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut start = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                    start = v.trim().trim_end_matches('-').parse().unwrap();
                    ranged2.fetch_add(1, Ordering::SeqCst);
                }
            }
            let slice = &body[start..];
            let head = if start > 0 {
                format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                    slice.len(), start, body.len() - 1, body.len()
                )
            } else {
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", slice.len())
            };
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(slice).unwrap();
        }
    });
    (format!("http://{addr}/model.bin"), ranged)
}

fn manifest(url: &str, sha: &str) -> Manifest {
    Manifest::parse(&format!(
        "version = 1\n[[model]]\nid = \"m\"\nname = \"Test model\"\nrole = \"coach\"\nurl = \"{url}\"\nfile = \"m.bin\"\nsha256 = \"{sha}\"\n"
    ))
    .unwrap()
}

fn tempdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("aac-models-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    d
}

fn body() -> Vec<u8> {
    (0..300_000u32).map(|i| (i % 251) as u8).collect()
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn downloads_and_verifies_pinned_hash() {
    let b = body();
    let (url, _) = serve(b.clone(), 1);
    let dir = tempdir("pinned");
    let store = ModelStore::new(&dir, manifest(&url, &sha(&b))).unwrap();
    let entry = store.manifest().get("m").unwrap().clone();
    assert_eq!(store.status(&entry), Status::Missing);
    let path = store.download("m", |_| true).unwrap();
    assert_eq!(fs::read(path).unwrap(), b);
    assert_eq!(store.status(&entry), Status::Ready { bytes: b.len() as u64, verified: true });
    fs::remove_dir_all(dir).ok();
}

#[test]
fn rejects_wrong_hash_and_cleans_up() {
    let (url, _) = serve(body(), 1);
    let dir = tempdir("wrong");
    let store = ModelStore::new(&dir, manifest(&url, &"0".repeat(64))).unwrap();
    assert!(matches!(store.download("m", |_| true), Err(ModelError::Checksum { .. })));
    let entry = store.manifest().get("m").unwrap().clone();
    assert_eq!(store.status(&entry), Status::Missing);
    fs::remove_dir_all(dir).ok();
}

#[test]
fn resumes_after_cancel_and_records_tofu_hash() {
    let b = body();
    let (url, ranged) = serve(b.clone(), 2);
    let dir = tempdir("resume");
    let store = ModelStore::new(&dir, manifest(&url, "")).unwrap();
    let cancelled = store.download("m", |p| p.downloaded < 100_000);
    assert!(matches!(cancelled, Err(ModelError::Cancelled)));
    let entry = store.manifest().get("m").unwrap().clone();
    assert!(matches!(store.status(&entry), Status::Partial { .. }));

    let path = store.download("m", |_| true).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b);
    assert_eq!(ranged.load(Ordering::SeqCst), 1, "second request should be a range request");
    assert_eq!(fs::read_to_string(dir.join("m.bin.sha256")).unwrap(), sha(&b));
    store.remove("m").unwrap();
    assert_eq!(store.status(&entry), Status::Missing);
    fs::remove_dir_all(dir).ok();
}

#[test]
fn manifest_validation() {
    assert!(Manifest::parse("version = 1\n[[model]]\nid=\"a\"\nname=\"a\"\nrole=\"asr\"\nurl=\"u\"\nfile=\"../x\"\n").is_err());
    assert!(Manifest::parse("version = 1\n[[model]]\nid=\"a\"\nname=\"a\"\nrole=\"asr\"\nurl=\"u\"\nfile=\"x\"\nsha256=\"abc\"\n").is_err());
}

#[test]
fn repository_manifest_parses() {
    let text = include_str!("../../../models.toml");
    let m = Manifest::parse(text).unwrap();
    assert!(m.default_for(&Role::Coach).is_some());
    assert!(m.default_for(&Role::Asr).is_some());
    assert!(m.default_for(&Role::Phonemes).unwrap().url.is_empty());
}

#[test]
fn script_installed_models_explain_themselves() {
    let m = Manifest::parse(
        "version = 1\n[[model]]\nid = \"p\"\nname = \"P\"\nrole = \"phonemes\"\nurl = \"\"\nfile = \"p.onnx\"\nnote = \"run the script\"\n",
    )
    .unwrap();
    let dir = tempdir("script");
    let store = ModelStore::new(&dir, m).unwrap();
    match store.download("p", |_| true) {
        Err(ModelError::Http(msg)) => assert_eq!(msg, "run the script"),
        other => panic!("{other:?}"),
    }
    fs::remove_dir_all(dir).ok();
}
