//! Toss <-> LocalSend bridge (spike). Receive: LocalSend-compatible v2 server that saves
//! into a folder through Toss's path sandbox. Send: push a whole folder to any LocalSend peer.
pub use localsend;

use anyhow::{anyhow, Context, Result};
use localsend::http::client::LsHttpClientV2;
use localsend::http::dto_v2::{PrepareUploadRequestDtoV2, RegisterDtoV2};
use localsend::http::server::common::save::FileUploadTarget;
use localsend::http::server::v2::{PrepareUploadDecisionV2, ServerEventV2};
use localsend::http::server::web::WebConfig;
use localsend::http::server::{start_with_port, ServerConfigV2, ServerHandle};
use localsend::http::state::ClientInfo;
use localsend::model::discovery::{DeviceType, ProtocolType, PROTOCOL_VERSION_V2};
use localsend::model::transfer::FileDto;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

pub struct Receiver {
    pub handle: ServerHandle,
    _stop: oneshot::Sender<()>,
}

/// Start a LocalSend-compatible receiver (plain HTTP) saving into `dest`.
pub async fn receive(alias: &str, port: u16, dest: PathBuf) -> Result<Receiver> {
    let (event_tx, mut event_rx) = mpsc::channel::<ServerEventV2>(32);
    let (stop_tx, stop_rx) = oneshot::channel();
    let info = ClientInfo {
        alias: alias.into(),
        version: PROTOCOL_VERSION_V2.into(),
        device_model: Some("Toss".into()),
        device_type: Some(DeviceType::Desktop),
        token: format!("toss-{}", std::process::id()),
    };
    let cfg = ServerConfigV2 { pin: None, verify_checksums: true, event_tx };
    let handle = start_with_port(port, None, info, None, Some(cfg), WebConfig::default(), stop_rx).await?;
    tokio::spawn(async move {
        while let Some(ev) = event_rx.recv().await {
            match ev {
                ServerEventV2::PrepareUpload { files, decision_tx, .. } => {
                    let ok = files.iter().filter(|(_, f)| toss_core::paths::sanitize_rel(&f.file_name).is_ok());
                    let _ = decision_tx.send(PrepareUploadDecisionV2::Accept(ok.map(|(k, _)| k.clone()).collect()));
                }
                ServerEventV2::FileUpload { file, target_tx, .. } => {
                    let Ok(rel) = toss_core::paths::sanitize_rel(&file.file_name) else { continue };
                    let path = dest.join(rel.to_path());
                    if let Some(p) = path.parent() {
                        let _ = std::fs::create_dir_all(p);
                    }
                    let (result_tx, result_rx) = oneshot::channel();
                    let _ = target_tx.send(FileUploadTarget::Path { path, result_tx, progress_tx: None });
                    tokio::spawn(async move {
                        if let Ok(Err(e)) = result_rx.await {
                            eprintln!("save failed: {e}");
                        }
                    });
                }
                _ => {}
            }
        }
    });
    Ok(Receiver { handle, _stop: stop_tx })
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            walk(root, &p, out)?;
        } else {
            let rel = p.strip_prefix(root)?.to_string_lossy().replace('\\', "/");
            out.push((rel, p));
        }
    }
    Ok(())
}

/// Send `folder` (keeps its name as the top directory) to a LocalSend peer.
pub async fn send_folder(alias: &str, host: &str, port: u16, folder: &Path) -> Result<usize> {
    let base = folder.parent().unwrap_or(folder);
    let mut list = Vec::new();
    walk(base, folder, &mut list)?;
    let mut files = HashMap::new();
    let mut paths = HashMap::new();
    for (i, (rel, p)) in list.into_iter().enumerate() {
        let id = format!("f{i}");
        let size = std::fs::metadata(&p)?.len();
        files.insert(id.clone(), FileDto {
            id: id.clone(), file_name: rel, size,
            file_type: "application/octet-stream".into(), sha256: None, preview: None, metadata: None,
        });
        paths.insert(id, p);
    }
    let payload = PrepareUploadRequestDtoV2 {
        info: RegisterDtoV2 {
            alias: alias.into(), version: PROTOCOL_VERSION_V2.into(), device_model: Some("Toss".into()),
            device_type: Some(DeviceType::Desktop), fingerprint: format!("toss-{}", std::process::id()),
            port, protocol: ProtocolType::Http, download: false,
        },
        files,
    };
    let client = LsHttpClientV2::try_new_without_cert()?;
    let prep = client
        .prepare_upload(ProtocolType::Http, host, port, None, payload, None, CancellationToken::new())
        .await
        .map_err(|e| anyhow!("prepare: {e}"))?;
    let resp = prep.response.ok_or_else(|| anyhow!("peer accepted nothing ({})", prep.status_code))?;
    let mut n = 0;
    for (id, token) in &resp.files {
        let f = tokio::fs::File::open(&paths[id]).await.context("open")?;
        let body = localsend::reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(f));
        client
            .upload(ProtocolType::Http, host, port, None, &resp.session_id, id, token, body, CancellationToken::new())
            .await
            .map_err(|e| anyhow!("upload {id}: {e}"))?;
        n += 1;
    }
    Ok(n)
}
