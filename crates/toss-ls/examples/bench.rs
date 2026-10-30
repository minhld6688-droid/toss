//! LocalSend-path throughput between two in-process bridges (HTTPS, loopback).
//! `cargo run --release -p toss-ls --example bench -- <MB> <files>`
use toss_ls::{LsBridge, LsConfig, LsEvent, LsOptions};

#[tokio::main]
async fn main() {
    let mb: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2048);
    let files: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let src = tempfile::tempdir().unwrap();
    let job = src.path().join("job");
    std::fs::create_dir_all(&job).unwrap();
    for i in 0..files {
        std::fs::write(
            job.join(format!("f{i}.bin")),
            vec![9u8; mb * 1024 * 1024 / files],
        )
        .unwrap();
    }
    let opts = LsOptions {
        multicast: false,
        subnet_scan: false,
        ..LsOptions::default()
    };
    let (d1, d2, i1, i2) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let cfg = |alias: &str, d: &tempfile::TempDir, i: &tempfile::TempDir| LsConfig {
        alias: alias.into(),
        data_dir: d.path().into(),
        dest_dir: i.path().into(),
        port: 0,
    };
    let (rx, mut rx_ev) = LsBridge::start_with(cfg("rx", &d1, &i1), opts.clone())
        .await
        .unwrap();
    let (tx, mut tx_ev) = LsBridge::start_with(cfg("tx", &d2, &i2), opts)
        .await
        .unwrap();
    tx.discover_addr("127.0.0.1", rx.port())
        .await
        .expect("discover");
    let rx2 = rx.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx_ev.recv().await {
            if let LsEvent::Incoming { req_id, .. } = ev {
                rx2.answer(&req_id, true);
            }
        }
    });
    let t = std::time::Instant::now();
    tx.send(&rx.fingerprint(), vec![job]).await.unwrap();
    while let Some(ev) = tx_ev.recv().await {
        match ev {
            LsEvent::Done { .. } => break,
            LsEvent::Error { msg, .. } => panic!("{msg}"),
            _ => {}
        }
    }
    let s = t.elapsed().as_secs_f64();
    println!(
        "{{\"localsend_https\":true,\"mb\":{mb},\"files\":{files},\"secs\":{s:.2},\"MBps\":{:.0}}}",
        mb as f64 / s
    );
}
