mod common;

use common::*;
use toss_ls::localsend::reqwest;

#[tokio::test]
async fn web_share_download_via_plain_http() {
    let a = node("Máy tổng").await;
    let src = tempfile::tempdir().unwrap();
    let job = src.path().join("Ảnh lớp");
    let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    write(&job.join("sub/ảnh 1.jpg"), &big);
    write(&job.join("ghi chú.txt"), b"xin chao");

    let share = a.b.web_share_start(vec![job]).await.unwrap();
    let port = a.b.web_share_port().await.unwrap();
    assert_ne!(port, a.b.port(), "separate plain-HTTP port");
    for u in &share.urls {
        assert!(
            u.starts_with("http://") && u.ends_with(&format!(":{port}")),
            "{u}"
        );
    }
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let page = http.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(page.status(), 200);
    assert!(page.text().await.unwrap().to_lowercase().contains("<html"));

    let prep: serde_json::Value = http
        .post(format!("{base}/api/localsend/v2/prepare-download"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let sid = prep["sessionId"].as_str().unwrap().to_string();
    assert_eq!(prep["info"]["alias"], "Máy tổng");
    let files = prep["files"].as_object().unwrap();
    assert_eq!(files.len(), 2);
    let mut seen = 0;
    for (fid, f) in files {
        let body = http
            .get(format!(
                "{base}/api/localsend/v2/download?sessionId={sid}&fileId={fid}"
            ))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .bytes()
            .await
            .unwrap();
        match f["fileName"].as_str().unwrap() {
            "Ảnh lớp/sub/ảnh 1.jpg" => assert_eq!(body.as_ref(), big.as_slice()),
            "Ảnh lớp/ghi chú.txt" => assert_eq!(body.as_ref(), b"xin chao"),
            n => panic!("unexpected {n}"),
        }
        seen += 1;
    }
    assert_eq!(seen, 2);

    // Replacing and stopping.
    let other = src.path().join("x.txt");
    write(&other, b"x");
    a.b.web_share_start(vec![other]).await.unwrap();
    let port2 = a.b.web_share_port().await.unwrap();
    a.b.web_share_stop().await;
    assert!(a.b.web_share_port().await.is_none());
    assert!(http
        .get(format!("http://127.0.0.1:{port2}/"))
        .send()
        .await
        .is_err());
    // The encrypted server kept running meanwhile.
    let b = node("B").await;
    assert!(b.b.discover_addr("127.0.0.1", a.b.port()).await.is_some());
}

#[tokio::test]
async fn web_share_rejects_nothing_to_share() {
    let a = node("A").await;
    let d = tempfile::tempdir().unwrap();
    assert!(a.b.web_share_start(vec![d.path().into()]).await.is_err());
}
