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
    let j = repo::claim_job(&c, "me").unwrap().unwrap();
    assert!(repo::claim_job(&c, "me").unwrap().is_none());
    repo::retry_job(&c, j.id, 0, "x").unwrap();
    assert!(repo::claim_job(&c, "me").unwrap().is_some());
    // A job another live computer is running stays theirs; ours (or a gone computer's) requeues.
    assert_eq!(repo::requeue_running_jobs(&c, "other", &["me".into()]).unwrap(), 0);
    assert_eq!(repo::requeue_running_jobs(&c, "other", &[]).unwrap(), 1);
}

fn journal_mode(v: &Vault) -> String {
    v.conn().query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap()
}

#[test]
fn shared_mode_switches_journal_and_persists() {
    let (_d, p) = tmp();
    let (v, _) = Vault::create(&p, "correct horse battery").unwrap();
    assert_eq!(journal_mode(&v), "wal");
    assert!(!v.is_shared());
    v.set_shared(true).unwrap();
    assert!(v.is_shared());
    assert_eq!(journal_mode(&v), "delete");
    v.close();
    assert!(!std::path::Path::new(&format!("{}-wal", p.display())).exists(), "no WAL file next to a shared vault");
    let v = Vault::open(&p, "correct horse battery").unwrap();
    assert!(v.is_shared());
    assert_eq!(journal_mode(&v), "delete");
    v.set_shared(false).unwrap();
    assert_eq!(journal_mode(&v), "wal");
}

/// Two independent openings of one shared vault (as two computers would have) writing at the
/// same time: every write lands, and each sees the other's.
#[test]
fn shared_vault_two_writers() {
    let (_d, p) = tmp();
    {
        let (v, _) = Vault::create(&p, "correct horse battery").unwrap();
        v.set_shared(true).unwrap();
        v.close();
    }
    let a = std::sync::Arc::new(Vault::open(&p, "correct horse battery").unwrap());
    let b = std::sync::Arc::new(Vault::open(&p, "correct horse battery").unwrap());
    let before = repo::last_seq(&a.conn()).unwrap();
    let hs: Vec<_> = [a.clone(), b.clone()]
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            std::thread::spawn(move || {
                for n in 0..40 {
                    let c = v.conn();
                    crate::in_tx(&c, |tx| repo::log_change(tx, "test", (i * 1000 + n) as i64, "write").map(|_| ())).unwrap();
                }
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    let seen_a = repo::changes_since(&a.conn(), before, 1000).unwrap();
    let seen_b = repo::changes_since(&b.conn(), before, 1000).unwrap();
    assert_eq!(seen_a.len(), 80);
    assert_eq!(seen_b.len(), 80);
    let pa = repo::claim_job(&a.conn(), "a").unwrap();
    assert!(pa.is_none());
    let j = repo::enqueue_job(&a.conn(), "ingest", Some(1), serde_json::json!({})).unwrap();
    // The job is claimed by exactly one of them.
    let ca = repo::claim_job(&a.conn(), "a").unwrap().map(|j| j.id);
    let cb = repo::claim_job(&b.conn(), "b").unwrap().map(|j| j.id);
    assert_eq!([ca, cb].iter().flatten().collect::<Vec<_>>(), vec![&j]);
}
