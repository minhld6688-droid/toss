#![allow(dead_code)]
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;
use toss_ls::{LsBridge, LsConfig, LsEvent, LsOptions};

pub struct Node {
    pub b: LsBridge,
    pub rx: mpsc::Receiver<LsEvent>,
    pub alias: String,
    pub data: TempDir,
    pub inbox: TempDir,
}

impl Node {
    pub fn inbox(&self) -> PathBuf {
        self.inbox.path().to_path_buf()
    }

    /// Next event matching `pred` (others are skipped). Panics after `secs`.
    pub async fn wait<F: FnMut(&LsEvent) -> bool>(&mut self, secs: u64, mut pred: F) -> LsEvent {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            match tokio::time::timeout_at(deadline, self.rx.recv()).await {
                Ok(Some(ev)) if pred(&ev) => return ev,
                Ok(Some(_)) => continue,
                Ok(None) => panic!("{}: event channel closed", self.alias),
                Err(_) => panic!("{}: timed out waiting for event", self.alias),
            }
        }
    }

    /// Waits for the outcome (Done/Error) of transfer `id`.
    pub async fn outcome(&mut self, id: &str, secs: u64) -> LsEvent {
        self.wait(
            secs,
            |e| matches!(e, LsEvent::Done { id: i, .. } | LsEvent::Error { id: i, .. } if i == id),
        )
        .await
    }

    pub async fn incoming(&mut self, secs: u64) -> LsEvent {
        self.wait(secs, |e| matches!(e, LsEvent::Incoming { .. }))
            .await
    }
}

pub fn quiet() -> LsOptions {
    LsOptions {
        multicast: false,
        subnet_scan: false,
        ..LsOptions::default()
    }
}

pub async fn node(alias: &str) -> Node {
    node_with(alias, 0, quiet()).await
}

pub async fn node_with(alias: &str, port: u16, opts: LsOptions) -> Node {
    let data = tempfile::tempdir().unwrap();
    let inbox = tempfile::tempdir().unwrap();
    let (b, rx) = LsBridge::start_with(
        LsConfig {
            alias: alias.into(),
            data_dir: data.path().into(),
            dest_dir: inbox.path().into(),
            port,
        },
        opts,
    )
    .await
    .expect("start bridge");
    Node {
        b,
        rx,
        alias: alias.into(),
        data,
        inbox,
    }
}

/// Makes `a` and `b` know each other over loopback (the known-address path).
pub async fn link(a: &Node, b: &Node) {
    let d =
        a.b.discover_addr("127.0.0.1", b.b.port())
            .await
            .expect("a finds b");
    assert_eq!(d.fingerprint, b.b.fingerprint());
    // b learned a from a's register request.
    for _ in 0..50 {
        if b.b
            .devices()
            .iter()
            .any(|d| d.fingerprint == a.b.fingerprint())
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("b did not learn a from its register request");
}

pub fn write(p: &Path, data: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, data).unwrap();
}

/// All files under `root`, as sorted `/`-relative paths.
pub fn tree(root: &Path) -> Vec<String> {
    fn go(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                out.push(format!(
                    "{}/",
                    p.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/")
                ));
                go(root, &p, out);
            } else {
                out.push(
                    p.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let mut out = Vec::new();
    if root.exists() {
        go(root, root, &mut out);
    }
    out.sort();
    out
}
