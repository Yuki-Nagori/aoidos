use super::*;

#[test]
fn completed_items_expose_outcome_and_corrupt_summary_never_returns_a_looping_cursor() {
    let mut session = create();
    session
        .append(Body::Narration {
            text: "完成正文".into(),
            turn_id: uuid::Uuid::new_v4().to_string(),
            terminal: crate::ports::Terminal::Cancelled,
        })
        .unwrap();
    assert_eq!(
        session.page(None, None).unwrap().items[0].outcome,
        Some(Outcome::Cancelled)
    );
    session.index.get_mut(&1).unwrap().created_at = "x".repeat(PAGE_BYTES);
    assert_eq!(session.page(None, None).unwrap_err().code, "store.corrupt");
    let dir = session.path.parent().unwrap().to_owned();
    drop(session);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn body_references_are_signed_bounded_and_follow_utf8_boundaries() {
    let mut session = create();
    let text = "中".repeat(format::MAX_TEXT / 3);
    session.append(player(&text)).unwrap();
    let page = session.page(None, None).unwrap();
    let reference = page.items[0].body_ref.as_ref().unwrap();
    let mut cursor = None;
    let mut actual = String::new();
    loop {
        let body = session.body(reference, cursor.as_deref()).unwrap();
        assert!(body.text.len() <= BODY_BYTES);
        actual.push_str(&body.text);
        cursor = body.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(actual, text);
    assert!(session.body("bad", None).is_err());
    assert!(session.body(&"x".repeat(4097), None).is_err());
    for invalid in ["x.y", "aa.zz", "a.bb", "ff.00"] {
        assert!(session.body(invalid, None).is_err());
    }
    let missing_ref = encode(
        &BodyRef {
            version: 1,
            session: session.header.session_id.clone(),
            epoch: session.view_epoch.clone(),
            seq: 999,
            hash: hash(b"missing"),
        },
        &session.reference_key,
    );
    assert_eq!(
        session.body(&missing_ref, None).unwrap_err().code,
        "app.not-found"
    );
    let mut token: BodyRef = decode(reference, &session.reference_key).unwrap();
    token.hash = hash(b"wrong");
    assert!(
        session
            .body(&encode(&token, &session.reference_key), None)
            .is_err()
    );
    token.seq = 0;
    assert!(
        session
            .body(&encode(&token, &session.reference_key), None)
            .is_err()
    );
    for offset in [1, text.len() + 1] {
        let cursor = encode(
            &BodyCursor {
                reference: reference.clone(),
                offset,
            },
            &session.reference_key,
        );
        assert!(session.body(reference, Some(&cursor)).is_err());
    }
    let cursor = encode(
        &BodyCursor {
            reference: "other".into(),
            offset: 0,
        },
        &session.reference_key,
    );
    assert!(session.body(reference, Some(&cursor)).is_err());
    let data = b"invalid json";
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(session.reference_key.as_bytes()).unwrap();
    mac.update(data);
    assert!(
        session
            .body(
                &format!("{}.{}", hex(data), hex(&mac.finalize().into_bytes())),
                None
            )
            .is_err()
    );
    assert!(unhex("é").is_err());
    assert!(unhex("aéa").is_err());
    assert!(unhex("a").is_err());
    session.view_epoch = uuid::Uuid::new_v4().to_string();
    assert!(session.body(reference, None).is_err());
    let dir = session.path.parent().unwrap().to_owned();
    drop(session);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn bounded_page_payload_makes_progress_and_unknown_body_keeps_raw_bytes() {
    let mut session = create();
    for _ in 0..3 {
        session.append(player(&"a".repeat(200_000))).unwrap();
    }
    let first = session.page(None, Some(200)).unwrap();
    assert_eq!(first.items.len(), 2);
    assert_eq!(
        session
            .page(first.next_cursor.as_deref(), None)
            .unwrap()
            .items
            .len(),
        1
    );
    session.close().unwrap();
    let path = session.path.clone();
    let dir = path.parent().unwrap().to_owned();
    let events = Arc::new(Events(AtomicU64::new(0)));
    drop(session);
    let raw=format::line(&serde_json::json!({"seq":4,"createdAt":now(),"kind":"future","payload":"中".repeat(699_000)})).unwrap();
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(raw.as_bytes()).unwrap();
    drop(file);
    let session = Session::open(path, events).unwrap();
    let page = session.page(None, Some(1)).unwrap();
    assert!(page.items[0].body.is_none());
    let reference = page.items[0].body_ref.as_ref().unwrap();
    let mut cursor = None;
    let mut text = String::new();
    let mut count = 0;
    loop {
        let part = session.body(reference, cursor.as_deref()).unwrap();
        count += 1;
        text.push_str(&part.text);
        cursor = part.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(text, raw);
    assert_eq!(count, 65);
    drop(session);
    std::fs::remove_dir_all(dir).unwrap();
}
use crate::record::{
    format::{Body, Header, hash, now},
    session::RecordEvents,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
struct Events(AtomicU64);
impl RecordEvents for Events {
    fn prepare(&self, _: &super::super::session::Appended) -> Result<u64, Fault> {
        Ok(self.0.fetch_add(1, Ordering::SeqCst) + 1)
    }
    fn deliver(&self, _: u64, _: super::super::session::Appended) -> Result<(), Fault> {
        Ok(())
    }
}
fn create() -> Session {
    let id = uuid::Uuid::new_v4().to_string();
    let path: PathBuf = std::env::temp_dir()
        .join(format!("aoidos-page-{id}"))
        .join(format!("{id}.jsonl"));
    let prefix = "[AOIDOS:STATIC]\nx\n[/AOIDOS:STATIC]\n".to_owned();
    Session::create(
        path,
        Header {
            kind: "header".into(),
            format_version: 1,
            grammar_version: 1,
            projection_version: 1,
            script_id: "demo".into(),
            session_id: id,
            created_at: now(),
            static_prefix_hash: hash(prefix.as_bytes()),
            static_prefix: prefix,
            script_revision: hash(b"x"),
        },
        Arc::new(Events(AtomicU64::new(0))),
    )
    .unwrap()
}
fn player(text: &str) -> Body {
    Body::PlayerSpeech {
        player_id: "p".into(),
        text: text.into(),
        mode: None,
        content_range: None,
    }
}
#[test]
fn pagination_boundary_and_epoch_are_independent_of_later_appends() {
    let mut session = create();
    for i in 0..4 {
        session.append(player(&format!("行{i}"))).unwrap();
    }
    let first = session.page(None, Some(2)).unwrap();
    assert_eq!(
        first.items.iter().map(|i| i.record_seq).collect::<Vec<_>>(),
        vec![4, 3]
    );
    session.append(player("新行")).unwrap();
    let second = session.page(first.next_cursor.as_deref(), Some(2)).unwrap();
    assert_eq!(
        second
            .items
            .iter()
            .map(|i| i.record_seq)
            .collect::<Vec<_>>(),
        vec![2, 1]
    );
    assert_eq!(second.last_record_seq, 4);
    assert!(session.page(None, Some(0)).is_err());
    assert!(session.page(Some("bad"), None).is_err());
    session.view_epoch = uuid::Uuid::new_v4().to_string();
    assert!(session.page(first.next_cursor.as_deref(), Some(2)).is_err());
    assert_eq!(session.view(None).unwrap().page.last_record_seq, 5);
    let dir = session.path.parent().unwrap().to_owned();
    drop(session);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn reopening_the_root_branch_keeps_later_records_visible_in_pages_and_projection() {
    struct Root;
    impl crate::record::history::HistoryPort for Root {
        fn verify_target(&self, _: &Session, _: u64) -> Result<(), Fault> {
            Ok(())
        }
        fn applied(&self, _: &str, _: &str) -> Result<bool, Fault> {
            Ok(false)
        }
        fn rebuild(&mut self, _: &Session, _: &[u64], _: &str, _: &str) -> Result<(), Fault> {
            Ok(())
        }
    }
    let mut session = create();
    session.append(player("重开前")).unwrap();
    let path = session.path.clone();
    drop(session);
    let mut session =
        Session::open(path.clone(), std::sync::Arc::new(Events(AtomicU64::new(0)))).unwrap();
    session.recover_history(&mut Root).unwrap();
    session.append(player("重开后的新输入")).unwrap();
    let page = session.page(None, None).unwrap();
    assert_eq!(
        page.items
            .iter()
            .map(|item| item.record_seq)
            .collect::<Vec<_>>(),
        vec![2, 1]
    );
    assert_eq!(
        page.items[0].body.as_ref().unwrap()["text"],
        "重开后的新输入"
    );
    let working = session.projection_working_set().unwrap();
    assert!(working.records().iter().any(|record| record.seq == 2));
    drop(session);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
