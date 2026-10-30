//! Web share: the LocalSend browser download page on a separate plain-HTTP port,
//! so the encrypted server keeps receiving from LocalSend apps meanwhile.

use crate::files::collect;
use crate::{lan_ipv4s, Inner, WebShare, DEFAULT_PORT, FALLBACK_PORTS};
use anyhow::{anyhow, Result};
use localsend::http::server::web::{
    WebConfig, WebDownloadConfig, WebDownloadEvent, WebI18n, WebMode,
};
use localsend::http::server::{start_with_port, ServerHandle};
use localsend::model::transfer::{FileContent, FileDto, FileMetadata};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

pub(crate) struct WebSlot {
    handle: ServerHandle,
    port: u16,
    stop: oneshot::Sender<()>,
    done: CancellationToken,
}

fn i18n_vi() -> WebI18n {
    WebI18n {
        waiting: "Đang chờ máy chia sẻ…".into(),
        enter_pin: "Nhập mã PIN".into(),
        invalid_pin: "Sai mã PIN".into(),
        too_many_attempts: "Thử sai quá nhiều lần".into(),
        rejected: "Bị từ chối".into(),
        upload_rejected: "Máy nhận đã từ chối.".into(),
        busy: "Máy kia đang bận.".into(),
        files: "File".into(),
        file_name: "Tên file".into(),
        size: "Dung lượng".into(),
        drop_hint: "Thả file vào đây.".into(),
    }
}

pub(crate) async fn start(inner: &Arc<Inner>, items: Vec<PathBuf>) -> Result<WebShare> {
    if inner.stopped.is_cancelled() {
        return Err(anyhow!("LocalSend đã tắt"));
    }
    stop(inner).await;
    let files = tokio::task::spawn_blocking(move || collect(&items))
        .await
        .unwrap_or_default();
    if files.is_empty() {
        return Err(anyhow!(
            "Không có file nào để chia sẻ (folder trống hoặc không đọc được)"
        ));
    }
    let mut dtos = HashMap::new();
    let mut paths = HashMap::new();
    for f in files {
        let id = uuid::Uuid::new_v4().to_string();
        dtos.insert(
            id.clone(),
            FileDto {
                id: id.clone(),
                file_name: f.name.clone(),
                size: f.size,
                file_type: crate::send::mime_of(&f.name).into(),
                sha256: None,
                preview: None,
                metadata: FileMetadata::from_path(&f.path),
            },
        );
        paths.insert(id, f.path);
    }
    let main = inner.port.load(Ordering::Relaxed);
    let mut ports: Vec<u16> = std::iter::once(DEFAULT_PORT)
        .chain(FALLBACK_PORTS)
        .filter(|p| *p != main)
        .collect();
    ports.push(0);
    let (tx, rx) = mpsc::channel(64);
    let mut started = None;
    let mut last_err = None;
    for p in ports {
        let (stop_tx, stop_rx) = oneshot::channel();
        let cfg = WebConfig {
            mode: WebMode::Download(WebDownloadConfig {
                files: dtos.clone(),
                pin: None,
                event_tx: tx.clone(),
            }),
            i18n: i18n_vi(),
            ..WebConfig::default()
        };
        // No TLS (browsers cannot verify the self-signed cert), no v2 upload API here.
        match start_with_port(p, None, inner.client_info(), None, None, cfg, stop_rx).await {
            Ok(h) => {
                started = Some((h, stop_tx));
                break;
            }
            Err(e) => last_err = Some(e),
        }
    }
    let Some((handle, stop_tx)) = started else {
        return Err(anyhow!(
            "Không mở được cổng chia sẻ: {}",
            last_err.map(|e| format!("{e:#}")).unwrap_or_default()
        ));
    };
    let port = handle.port();
    let done = inner.stopped.child_token();
    inner.rt.spawn(event_loop(rx, paths, done.clone()));
    let urls = lan_ipv4s()
        .into_iter()
        .map(|ip| format!("http://{ip}:{port}"))
        .collect();
    *inner.web.lock().await = Some(WebSlot {
        handle,
        port,
        stop: stop_tx,
        done,
    });
    Ok(WebShare { urls })
}

async fn event_loop(
    mut rx: mpsc::Receiver<WebDownloadEvent>,
    paths: HashMap<String, PathBuf>,
    done: CancellationToken,
) {
    loop {
        let ev = tokio::select! {
            ev = rx.recv() => ev,
            _ = done.cancelled() => return,
        };
        match ev {
            None => return,
            Some(WebDownloadEvent::PrepareDownload { decision_tx, .. }) => {
                // Web share auto-accepts every browser on the LAN.
                let _ = decision_tx.send(true);
            }
            Some(WebDownloadEvent::FileDownload {
                file_id,
                content_tx,
                ..
            }) => {
                if let Some(p) = paths.get(&file_id) {
                    let _ = content_tx.send(FileContent::Path(p.clone()));
                }
            }
        }
    }
}

pub(crate) async fn stop(inner: &Arc<Inner>) {
    let slot = inner.web.lock().await.take();
    if let Some(s) = slot {
        s.done.cancel();
        let _ = s.stop.send(());
        let _ = tokio::time::timeout(Duration::from_secs(3), s.handle.wait_stopped()).await;
    }
}

pub(crate) async fn port(inner: &Arc<Inner>) -> Option<u16> {
    inner.web.lock().await.as_ref().map(|s| s.port)
}
