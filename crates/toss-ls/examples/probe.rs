//! Real-device interop probe: `cargo run --release -p toss-ls --example probe -- <folder-to-send> [seconds]`
//! Lists LocalSend devices on the LAN, sends <folder> to every non-Toss device found, auto-accepts
//! anything sent back (saved under ./probe-inbox), prints every event as JSON.
use std::path::PathBuf;
use std::time::Duration;
use toss_ls::{LsBridge, LsConfig, LsEvent};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let folder = std::env::args().nth(1).map(PathBuf::from);
    let secs: u64 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(120);
    let cwd = std::env::current_dir()?;
    let (br, mut rx) = LsBridge::start(LsConfig {
        alias: "Toss thử".into(),
        data_dir: cwd.join("probe-data"),
        dest_dir: cwd.join("probe-inbox"),
        port: 53317,
    })
    .await?;
    println!("listening on {} fp {}", br.port(), br.fingerprint());
    br.rescan().await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    let mut sent = std::collections::HashSet::new();
    let mut tick = tokio::time::interval(Duration::from_secs(3));
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            _ = tick.tick() => {
                for d in br.devices() {
                    println!("device {}", serde_json::to_string(&d)?);
                    if let Some(f) = &folder {
                        if !d.is_toss && sent.insert(d.fingerprint.clone()) {
                            match br.send(&d.fingerprint, vec![f.clone()]).await {
                                Ok(id) => println!("send started {id} -> {}", d.alias),
                                Err(e) => println!("send error -> {}: {e:#}", d.alias),
                            }
                            let _ = br.send_text(&d.fingerprint, "Xin chào từ Toss 0.8 👋").await;
                        }
                    }
                }
            }
            Some(ev) = rx.recv() => {
                if let LsEvent::Incoming { req_id, .. } = &ev { br.answer(req_id, true); }
                println!("event {}", serde_json::to_string(&ev)?);
            }
        }
    }
    br.stop().await;
    Ok(())
}
