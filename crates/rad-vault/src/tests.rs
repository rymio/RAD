use super::*;
use rad_api::*;

fn tmp() -> (tempfile::TempDir, std::path::PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("test.radvault");
    (d, p)
}

#[test]
fn create_open_wrong_key_recovery_rekey() {
    let (_d, p) = tmp();
    let (v, recovery) = Vault::create(&p, "correct horse battery").unwrap();
    assert!(recovery.starts_with("RAD1-"));
    {
        let c = v.conn();
        repo::create_project(&c, &ProjectInput { name: Some("Taxes".into()), ..Default::default() }).unwrap();
    }
    assert!(v.verify_passphrase("correct horse battery"));
    assert!(!v.verify_passphrase("wrong horse battery"));
    v.close();

    assert!(matches!(Vault::open(&p, "wrong passphrase!"), Err(VaultError::WrongKey)));
    let v = Vault::open(&p, "correct horse battery").unwrap();
    assert_eq!(repo::projects(&v.conn()).unwrap().len(), 1);
    v.close();

    let v = Vault::open_with_recovery(&p, &recovery.to_lowercase()).unwrap();
    let new_recovery = v.rekey("a brand new passphrase").unwrap();
    assert_ne!(new_recovery, recovery);
    assert_eq!(repo::projects(&v.conn()).unwrap().len(), 1);
    v.close();

    assert!(Vault::open(&p, "correct horse battery").is_err());
    assert!(Vault::open_with_recovery(&p, &recovery).is_err());
    let v = Vault::open(&p, "a brand new passphrase").unwrap();
    assert_eq!(repo::projects(&v.conn()).unwrap()[0].name, "Taxes");
    v.close();
    let v = Vault::open_with_recovery(&p, &new_recovery).unwrap();
    v.close();
}

#[test]
fn bad_recovery_key_checksum() {
    let k = [7u8; 32];
    let mut s = encode_recovery_key(&k);
    assert_eq!(&*decode_recovery_key(&s).unwrap(), &k);
    s.replace_range(8..9, if &s[8..9] == "A" { "B" } else { "A" });
    assert!(decode_recovery_key(&s).is_err());
}

#[test]
fn blobs_dedup_range_and_no_plaintext() {
    let (_d, p) = tmp();
    let (v, _) = Vault::create(&p, "correct horse battery").unwrap();
    let marker = b"SUPER-SECRET-MARKER-0451";
    let mut data = Vec::new();
    for i in 0..(3 * blobs::CHUNK / 24 + 7) {
        data.extend_from_slice(marker);
        data.push((i % 251) as u8);
    }
    let c = v.conn();
    let (m1, dup1) = blobs::put(&c, &data, "text/plain").unwrap();
    let (m2, dup2) = blobs::put(&c, &data, "text/plain").unwrap();
    assert!(!dup1 && dup2);
    assert_eq!(m1.id, m2.id);
    assert_eq!(blobs::read_all(&c, m1.id).unwrap(), data);
    let r = blobs::read_range(&c, m1.id, blobs::CHUNK as u64 - 10, blobs::CHUNK as u64 + 10).unwrap();
    assert_eq!(&r[..], &data[blobs::CHUNK - 10..blobs::CHUNK + 10]);
    drop(c);
    v.close();
    let raw = std::fs::read(&p).unwrap();
    assert!(!raw.windows(marker.len()).any(|w| w == marker), "plaintext leaked into vault file");
}

#[test]
fn fts_and_vectors_with_partitions() {
    let (_d, p) = tmp();
    let (v, _) = Vault::create(&p, "correct horse battery").unwrap();
    let c = v.conn();
    let p1 = repo::create_project(&c, &ProjectInput { name: Some("House".into()), ..Default::default() }).unwrap();
    let p2 = repo::create_project(&c, &ProjectInput { name: Some("Work".into()), ..Default::default() }).unwrap();
    let mk = |pid: Id, text: &str| {
        let (b, _) = blobs::put(&c, text.as_bytes(), "text/plain").unwrap();
        let d = repo::create_document(&c, &repo::NewDocument {
            project_id: pid, blob_id: b.id, title: "t", original_name: "t.txt", mime: "text/plain",
            size: text.len() as i64, source_kind: "upload", tags: &[],
        }).unwrap();
        let ids = repo::replace_chunks(&c, d.id, &[repo::NewChunk {
            text: text.into(), context: String::new(), anchor: Anchor::default(), kind: "text".into(),
        }]).unwrap();
        (d.id, ids[0])
    };
    let (_, roof) = mk(p1.id, "The roofing invoice was 12400 dollars");
    let (_, budget) = mk(p2.id, "Quarterly budget review for the roofing division");
    let hits = search::fts_search(&c, "how much was the roof invoice?", &[], 10).unwrap();
    assert_eq!(hits[0].0, roof);
    let hits = search::fts_search(&c, "roofing", &[p2.id], 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, budget);

    let s = search::ensure_space(&c, "test:model", 3).unwrap();
    search::insert_vectors(&c, s, &[(roof, p1.id, vec![1.0, 0.0, 0.0]), (budget, p2.id, vec![0.0, 1.0, 0.0])]).unwrap();
    let nn = search::knn(&c, s, &[0.9, 0.1, 0.0], 5, &[]).unwrap();
    assert_eq!(nn[0].0, roof);
    let nn = search::knn(&c, s, &[0.9, 0.1, 0.0], 5, &[p2.id]).unwrap();
    assert_eq!(nn.len(), 1);
    assert_eq!(nn[0].0, budget);
    assert_eq!(search::vectors(&c, s, &[roof]).unwrap()[&roof], vec![1.0, 0.0, 0.0]);
}

#[test]
fn backup_is_openable() {
    let (d, p) = tmp();
    let (v, _) = Vault::create(&p, "correct horse battery").unwrap();
    repo::create_project(&v.conn(), &ProjectInput { name: Some("X".into()), ..Default::default() }).unwrap();
    let b = d.path().join("backup.radvault");
    v.backup_to(&b).unwrap();
    let bv = Vault::open(&b, "correct horse battery").unwrap();
    assert_eq!(repo::projects(&bv.conn()).unwrap().len(), 1);
    assert!(v.integrity_check().unwrap().iter().any(|m| m == "ok"));
}

#[test]
fn jobs_queue() {
    let (_d, p) = tmp();
    let (v, _) = Vault::create(&p, "correct horse battery").unwrap();
    let c = v.conn();
    let a = repo::enqueue_job(&c, "ingest", Some(1), serde_json::json!({})).unwrap();
    let b = repo::enqueue_job(&c, "ingest", Some(1), serde_json::json!({})).unwrap();
    assert_eq!(a, b, "duplicate pending jobs collapse");
    let j = repo::claim_job(&c).unwrap().unwrap();
    assert!(repo::claim_job(&c).unwrap().is_none());
    repo::retry_job(&c, j.id, 0, "x").unwrap();
    assert!(repo::claim_job(&c).unwrap().is_some());
    assert_eq!(repo::requeue_running_jobs(&c).unwrap(), 1);
}
