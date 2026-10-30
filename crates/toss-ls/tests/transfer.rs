mod common;

use common::*;
use std::collections::HashMap;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use toss_ls::localsend::crypto::cert::generate_self_signed;
use toss_ls::localsend::http::client::LsHttpClientV2;
use toss_ls::localsend::http::dto_v2::{PrepareUploadRequestDtoV2, RegisterDtoV2};
use toss_ls::localsend::model::discovery::{DeviceType, ProtocolType};
use toss_ls::localsend::model::transfer::FileDto;
use toss_ls::LsEvent;

/// Auto-answers every Incoming of `n` with `accept`, forwarding all events.
fn auto_answer(mut n: Node, accept: bool) -> (Node, tokio::sync::mpsc::Receiver<LsEvent>) {
    let (tx, rx) = tokio::sync::mpsc::channel(512);
    let mut events = std::mem::replace(&mut n.rx, tokio::sync::mpsc::channel(1).1);
    let b = n.b.clone();
    tokio::spawn(async move {
        while let Some(ev) = events.recv().await {
            if let LsEvent::Incoming { req_id, .. } = &ev {
                b.answer(req_id, accept);
            }
            let _ = tx.send(ev).await;
        }
    });
    (n, rx)
}

#[tokio::test]
async fn folder_roundtrip_keeps_structure_and_names() {
    let mut a = node("Máy tổng").await;
    let mut b = node("Editor Hà").await;
    link(&a, &b).await;

    let src = tempfile::tempdir().unwrap();
    let job = src.path().join("Lớp 12A1 — kỷ yếu");
    write(&job.join("RAW/cam 1/IMG 0001.CR3"), &vec![7u8; 3_000_000]);
    write(&job.join("RAW/cam 1/ảnh đẹp.jpg"), b"jpeg");
    write(&job.join("ghi chú.txt"), b"hello");
    std::fs::create_dir_all(job.join("trống/rỗng")).unwrap();
    let loose = src.path().join("lẻ.txt");
    write(&loose, b"loose");

    let id =
        a.b.send(&b.b.fingerprint(), vec![job.clone(), loose.clone()])
            .await
            .unwrap();
    match a.wait(10, |e| matches!(e, LsEvent::Started { .. })).await {
        LsEvent::Started {
            id: i,
            outgoing,
            files,
            bytes,
            ..
        } => {
            assert_eq!(i, id);
            assert!(outgoing);
            assert_eq!(files, 4);
            assert_eq!(bytes, 3_000_000 + 4 + 5 + 5);
        }
        _ => unreachable!(),
    }
    let req = match b.incoming(10).await {
        LsEvent::Incoming {
            req_id,
            fingerprint,
            alias,
            files,
            bytes,
            names,
            text,
            ..
        } => {
            assert_eq!(fingerprint, a.b.fingerprint(), "mTLS-verified fingerprint");
            assert_eq!(alias, "Máy tổng");
            assert_eq!(files, 4);
            assert_eq!(bytes, 3_000_014);
            assert!(names.len() <= 5);
            assert!(text.is_none());
            req_id
        }
        _ => unreachable!(),
    };
    b.b.answer(&req, true);
    match b.outcome(&req, 30).await {
        LsEvent::Done {
            dest, files, bytes, ..
        } => {
            assert_eq!(files, 4);
            assert_eq!(bytes, 3_000_014);
            // Two top-level items: dest is the sender folder.
            assert_eq!(dest.unwrap(), b.inbox().join("Máy tổng"));
        }
        e => panic!("{e:?}"),
    }
    match a.outcome(&id, 30).await {
        LsEvent::Done {
            files, bytes, dest, ..
        } => {
            assert_eq!((files, bytes), (4, 3_000_014));
            assert!(dest.is_none());
        }
        e => panic!("{e:?}"),
    }
    let root = b.inbox().join("Máy tổng");
    assert_eq!(
        tree(&root),
        vec![
            "lẻ.txt",
            "Lớp 12A1 — kỷ yếu/",
            "Lớp 12A1 — kỷ yếu/RAW/",
            "Lớp 12A1 — kỷ yếu/RAW/cam 1/",
            "Lớp 12A1 — kỷ yếu/RAW/cam 1/IMG 0001.CR3",
            "Lớp 12A1 — kỷ yếu/RAW/cam 1/ảnh đẹp.jpg",
            "Lớp 12A1 — kỷ yếu/ghi chú.txt",
        ]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>()
        .tap_sort()
    );
    assert_eq!(
        std::fs::read(root.join("Lớp 12A1 — kỷ yếu/ghi chú.txt")).unwrap(),
        b"hello"
    );
    assert_eq!(
        std::fs::read(root.join("Lớp 12A1 — kỷ yếu/RAW/cam 1/IMG 0001.CR3")).unwrap(),
        vec![7u8; 3_000_000]
    );
    a.b.stop().await;
    b.b.stop().await;
}

trait TapSort {
    fn tap_sort(self) -> Self;
}
impl TapSort for Vec<String> {
    fn tap_sort(mut self) -> Self {
        self.sort();
        self
    }
}

#[tokio::test]
async fn no_clobber_on_repeat() {
    let mut a = node("A").await;
    let b = node("B").await;
    link(&a, &b).await;
    let inbox = b.inbox();
    let fp = b.b.fingerprint();
    let (_b, mut bev) = auto_answer(b, true);

    let src = tempfile::tempdir().unwrap();
    let job = src.path().join("job");
    write(&job.join("x.jpg"), b"1");
    let single = src.path().join("a.txt");
    write(&single, b"single");

    for round in 0..2 {
        for item in [job.clone(), single.clone()] {
            let id = a.b.send(&fp, vec![item.clone()]).await.unwrap();
            match a.outcome(&id, 20).await {
                LsEvent::Done { .. } => {}
                e => panic!("round {round}: {e:?}"),
            }
            // The receiver's Done carries the job folder.
            let done = loop {
                match tokio::time::timeout(Duration::from_secs(10), bev.recv())
                    .await
                    .unwrap()
                    .unwrap()
                {
                    LsEvent::Done { dest, .. } => break dest.unwrap(),
                    LsEvent::Error { msg, .. } => panic!("{msg}"),
                    _ => {}
                }
            };
            if item == job {
                let want = if round == 0 { "job" } else { "job (2)" };
                assert_eq!(done, inbox.join("A").join(want));
            }
        }
    }
    assert_eq!(
        tree(&inbox.join("A")),
        vec![
            "a (2).txt",
            "a.txt",
            "job (2)/",
            "job (2)/x.jpg",
            "job/",
            "job/x.jpg"
        ]
    );
}

#[tokio::test]
async fn text_message_is_an_event_not_a_file() {
    let a = node("A").await;
    let mut b = node("B").await;
    link(&a, &b).await;
    a.b.send_text(&b.b.fingerprint(), "Chào anh 👋\nfile xong rồi")
        .await
        .unwrap();
    match b
        .wait(10, |e| {
            matches!(e, LsEvent::Text { .. } | LsEvent::Incoming { .. })
        })
        .await
    {
        LsEvent::Text {
            fingerprint,
            alias,
            text,
        } => {
            assert_eq!(fingerprint, a.b.fingerprint());
            assert_eq!(alias, "A");
            assert_eq!(text, "Chào anh 👋\nfile xong rồi");
        }
        e => panic!("{e:?}"),
    }
    assert!(tree(&b.inbox()).is_empty(), "nothing saved");
    assert!(a
        .b
        .send_text(&b.b.fingerprint(), &"x".repeat(70_000))
        .await
        .is_err());
    assert!(a.b.send_text(&b.b.fingerprint(), "").await.is_err());
}

#[tokio::test]
async fn decline_and_timeout() {
    let mut a = node("A").await;
    let mut b = node("B").await;
    link(&a, &b).await;
    let src = tempfile::tempdir().unwrap();
    let f = src.path().join("f.bin");
    write(&f, b"data");

    let id = a.b.send(&b.b.fingerprint(), vec![f.clone()]).await.unwrap();
    let LsEvent::Incoming { req_id, .. } = b.incoming(10).await else {
        unreachable!()
    };
    b.b.answer(&req_id, false);
    match a.outcome(&id, 10).await {
        LsEvent::Error { msg, .. } => assert_eq!(msg, "Máy kia đã từ chối"),
        e => panic!("{e:?}"),
    }
    assert!(tree(&b.inbox()).is_empty());

    // No answer → declined after the timeout, both sides told.
    let mut c = node_with(
        "C",
        0,
        toss_ls::LsOptions {
            answer_timeout: Duration::from_secs(1),
            ..quiet()
        },
    )
    .await;
    link(&a, &c).await;
    let id = a.b.send(&c.b.fingerprint(), vec![f]).await.unwrap();
    let LsEvent::Incoming { req_id, .. } = c.incoming(10).await else {
        unreachable!()
    };
    match c.outcome(&req_id, 10).await {
        LsEvent::Error { msg, .. } => assert!(msg.contains("Không trả lời"), "{msg}"),
        e => panic!("{e:?}"),
    }
    match a.outcome(&id, 10).await {
        LsEvent::Error { msg, .. } => assert_eq!(msg, "Máy kia đã từ chối"),
        e => panic!("{e:?}"),
    }
}

#[tokio::test]
async fn cancel_pending_and_running() {
    let mut a = node("A").await;
    let mut b = node("B").await;
    link(&a, &b).await;
    let src = tempfile::tempdir().unwrap();
    let small = src.path().join("s.bin");
    write(&small, b"x");

    // 1. Cancel while the receiver is still deciding.
    let id = a.b.send(&b.b.fingerprint(), vec![small]).await.unwrap();
    let LsEvent::Incoming { req_id, .. } = b.incoming(10).await else {
        unreachable!()
    };
    a.b.cancel(&id);
    match a.outcome(&id, 10).await {
        LsEvent::Error { msg, .. } => assert_eq!(msg, "Đã huỷ"),
        e => panic!("{e:?}"),
    }
    match b.outcome(&req_id, 10).await {
        LsEvent::Error { msg, .. } => assert!(msg.contains("huỷ"), "{msg}"),
        e => panic!("{e:?}"),
    }

    // 2. Cancel mid-upload (big sparse file).
    let big = src.path().join("big.bin");
    let f = std::fs::File::create(&big).unwrap();
    f.set_len(2_000_000_000).unwrap();
    drop(f);
    let id = a.b.send(&b.b.fingerprint(), vec![big]).await.unwrap();
    let LsEvent::Incoming { req_id, .. } = b.incoming(10).await else {
        unreachable!()
    };
    b.b.answer(&req_id, true);
    a.wait(
        20,
        |e| matches!(e, LsEvent::Progress { bytes_done, .. } if *bytes_done > 0),
    )
    .await;
    a.b.cancel(&id);
    match a.outcome(&id, 10).await {
        LsEvent::Error { msg, .. } => assert_eq!(msg, "Đã huỷ"),
        e => panic!("{e:?}"),
    }
    match b.outcome(&req_id, 15).await {
        LsEvent::Error { msg, .. } => assert!(msg.contains("huỷ"), "{msg}"),
        e => panic!("{e:?}"),
    }
    // The partial file is removed.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!b.inbox().join("A/big.bin").exists());

    // 3. Receiver-side cancel reaches the sender.
    let big2 = src.path().join("big2.bin");
    std::fs::File::create(&big2)
        .unwrap()
        .set_len(2_000_000_000)
        .unwrap();
    let id = a.b.send(&b.b.fingerprint(), vec![big2]).await.unwrap();
    let LsEvent::Incoming { req_id, .. } = b.incoming(10).await else {
        unreachable!()
    };
    b.b.answer(&req_id, true);
    b.wait(
        20,
        |e| matches!(e, LsEvent::Progress { bytes_done, .. } if *bytes_done > 0),
    )
    .await;
    b.b.cancel(&req_id);
    match a.outcome(&id, 15).await {
        LsEvent::Error { msg, .. } => assert!(msg.contains("Máy kia"), "{msg}"),
        e => panic!("{e:?}"),
    }
}

/// A hostile LocalSend sender: traversal and illegal names never escape the inbox.
#[tokio::test]
async fn evil_names_stay_inside_the_inbox() {
    let mut b = node("B").await;
    let cert = tokio::task::spawn_blocking(|| generate_self_signed().unwrap())
        .await
        .unwrap();
    let client = LsHttpClientV2::try_new(
        &cert.private_key_pem,
        &cert.certificate_pem,
        Some(b.b.fingerprint()),
        None,
    )
    .unwrap();
    let names = [
        "../escape.txt",
        "/abs.txt",
        "C:\\win.txt",
        "ok/../../x.txt",
        "..\\..\\y.txt",
        "good/con.txt",
        "fine.txt",
        "a\\b.txt",
        "q?<>|.txt",
    ];
    let mut files = HashMap::new();
    for (i, n) in names.iter().enumerate() {
        let id = format!("f{i}");
        files.insert(
            id.clone(),
            FileDto {
                id,
                file_name: n.to_string(),
                size: 3,
                file_type: "application/octet-stream".into(),
                sha256: None,
                preview: None,
                metadata: None,
            },
        );
    }
    let info = RegisterDtoV2 {
        alias: "../../Evil\u{1b}[31m".into(),
        version: "2.2".into(),
        device_model: None,
        device_type: Some(DeviceType::Mobile),
        fingerprint: cert.fingerprint.clone(),
        port: 1,
        protocol: ProtocolType::Https,
        download: false,
    };
    let bb = b.b.clone();
    let port = b.b.port();
    let prep = tokio::spawn(async move {
        client
            .prepare_upload(
                ProtocolType::Https,
                "127.0.0.1",
                port,
                None,
                PrepareUploadRequestDtoV2 { info, files },
                None,
                CancellationToken::new(),
            )
            .await
            .map(|r| (r, client))
    });
    let (req, alias) = match b.incoming(10).await {
        LsEvent::Incoming {
            req_id,
            files,
            alias,
            fingerprint,
            ..
        } => {
            assert_eq!(files, 4, "only the safe names are offered");
            assert_eq!(fingerprint, cert.fingerprint);
            (req_id, alias)
        }
        _ => unreachable!(),
    };
    assert!(!alias.contains('\u{1b}'));
    bb.answer(&req, true);
    let (r, client) = prep.await.unwrap().unwrap();
    let resp = r.response.unwrap();
    let mut ids: Vec<_> = resp.files.keys().cloned().collect();
    ids.sort();
    assert_eq!(ids, vec!["f5", "f6", "f7", "f8"]);
    for (fid, tok) in &resp.files {
        let body = toss_ls::localsend::reqwest::Body::from(b"abc".to_vec());
        client
            .upload(
                ProtocolType::Https,
                "127.0.0.1",
                port,
                None,
                &resp.session_id,
                fid,
                tok,
                body,
                CancellationToken::new(),
            )
            .await
            .unwrap();
    }
    match b.outcome(&req, 10).await {
        LsEvent::Done { files, .. } => assert_eq!(files, 4),
        e => panic!("{e:?}"),
    }
    // Everything sits under <inbox>/<sanitized alias>/ ("../../Evil" is one component).
    let tops: Vec<_> = std::fs::read_dir(b.inbox())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(tops.len(), 1, "{tops:?}");
    assert!(tops[0].is_dir());
    assert_eq!(
        tops[0].file_name().unwrap().to_string_lossy(),
        ".._.._Evil[31m"
    );
    assert_eq!(
        tree(&tops[0]),
        vec![
            "a/",
            "a/b.txt",
            "fine.txt",
            "good/",
            "good/_.txt",
            "q____.txt"
        ]
    );
    let parent = b.inbox().parent().unwrap().to_path_buf();
    for n in ["escape.txt", "abs.txt", "x.txt", "y.txt"] {
        assert!(!parent.join(n).exists());
    }
}

#[tokio::test]
async fn send_errors_are_vietnamese() {
    let a = node("A").await;
    let err = a.b.send("NOPE", vec![]).await.unwrap_err().to_string();
    assert!(err.contains("Không thấy máy"), "{err}");
    let b = node("B").await;
    link(&a, &b).await;
    let empty = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(empty.path().join("e/f")).unwrap();
    let err =
        a.b.send(&b.b.fingerprint(), vec![empty.path().join("e")])
            .await
            .unwrap_err()
            .to_string();
    assert!(err.contains("Không có file"), "{err}");
    // Peer gone → unreachable.
    let fp = b.b.fingerprint();
    b.b.stop().await;
    let mut a = a;
    let src = tempfile::tempdir().unwrap();
    write(&src.path().join("x"), b"1");
    let id = a.b.send(&fp, vec![src.path().join("x")]).await.unwrap();
    match a.outcome(&id, 15).await {
        LsEvent::Error { msg, .. } => assert!(msg.starts_with("Không kết nối được"), "{msg}"),
        e => panic!("{e:?}"),
    }
}
