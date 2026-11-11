//! Studio pipeline end to end, two nodes over loopback: an editor backs a job
//! folder up to the studio through a put-only link and frees the local copy on
//! the receipt; a download into a full disk stops before any byte moves.

use std::{
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

use iroh::{EndpointAddr, TransportAddr};
use toss_core::{transfer::Status, HostConfig, Node, NodeConfig, Root, Ticket, TicketSpec};

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "toss_core=info".into()),
        )
        .with_test_writer()
        .try_init();
}

async fn node(dir: &Path, name: &str) -> Node {
    Node::start(NodeConfig {
        data_dir: dir.join(format!("{name}-data")),
        device_name: name.to_string(),
        app_version: "test".into(),
        net: Default::default(),
    })
    .await
    .expect("start node")
}

fn loopback_addr(node: &Node) -> EndpointAddr {
    let addrs = node
        .endpoint()
        .bound_sockets()
        .into_iter()
        .filter(|s| s.is_ipv4())
        .map(|s| TransportAddr::Ip(SocketAddr::from((Ipv4Addr::LOCALHOST, s.port()))));
    EndpointAddr::from_parts(node.endpoint_id(), addrs)
}

fn write(p: &Path, len: usize, seed: u8) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect::<Vec<_>>()).unwrap();
}

struct Studio {
    _tmp: tempfile::TempDir,
    host: Node,
    editor: Node,
    jobs: PathBuf,
    backup: PathBuf,
    work: PathBuf,
}

/// The studio: `Jobs` served read-only, backups land in `Backup/<editor>`.
async fn studio() -> Studio {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path().to_path_buf();
    let (jobs, backup, work) = (t.join("Jobs"), t.join("Backup"), t.join("work"));
    for d in [&jobs, &work] {
        std::fs::create_dir_all(d).unwrap();
    }
    let host = node(&t, "studio").await;
    let editor = node(&t, "editor").await;
    let mut cfg = HostConfig::new(vec![Root { alias: "Jobs".into(), path: jobs.clone() }], backup.clone());
    cfg.max_parallel_jobs = 2;
    host.set_host(Some(cfg)).await.unwrap();
    editor.add_peer_addr(loopback_addr(&host));
    Studio { _tmp: tmp, host, editor, jobs, backup, work }
}

fn backup_link(s: &Studio, editor: &str) -> Ticket {
    let t = s
        .host
        .create_ticket(TicketSpec {
            job: format!("Backup · {editor}"),
            root: "Jobs".into(),
            path: String::new(),
            ret: editor.into(),
            get: false,
            put: true,
            days: 0,
        })
        .unwrap();
    t.to_string().parse().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backup_writes_receipt_and_frees_only_unchanged_files() {
    let s = studio().await;
    let job = s.work.join("03.11.26 12A1 Doan Ket HBT");
    write(&job.join("1_RAW/Tuan Future/DSC_0001.NEF"), 300_000, 1);
    write(&job.join("1_RAW/Tuan Future/DSC_0002.NEF"), 200_000, 2);
    write(&job.join("Color/DSC_0001.jpg"), 50_000, 3);
    let ticket = backup_link(&s, "Editor An");

    let t = s.editor.put(ticket.clone(), vec![job.clone()]).await.unwrap();
    let st = t.wait().await;
    assert_eq!(st.status, Status::Done, "{st:?}");

    // On the studio, in the editor's own area, byte for byte.
    let landed = s.backup.join("Editor An/03.11.26 12A1 Doan Ket HBT");
    for rel in ["1_RAW/Tuan Future/DSC_0001.NEF", "1_RAW/Tuan Future/DSC_0002.NEF", "Color/DSC_0001.jpg"] {
        assert_eq!(std::fs::read(landed.join(rel)).unwrap(), std::fs::read(job.join(rel)).unwrap(), "{rel}");
    }
    assert!(!s.jobs.join("03.11.26 12A1 Doan Ket HBT").exists(), "backups never land in Jobs");

    // The receipt lists the three files.
    let receipts = s.editor.receipts();
    assert_eq!(receipts.len(), 1);
    assert_eq!((receipts[0].id.as_str(), receipts[0].files, receipts[0].bytes), (t.id.as_str(), 3, 550_000));

    // Backing the same folder up again moves nothing new and leaves no "(2)" copies.
    let t2 = s.editor.put(ticket, vec![job.clone()]).await.unwrap();
    assert_eq!(t2.wait().await.status, Status::Done);
    let names: Vec<_> = std::fs::read_dir(landed.join("1_RAW/Tuan Future")).unwrap().flatten().map(|e| e.file_name()).collect();
    assert_eq!(names.len(), 2, "identical files are skipped, not duplicated: {names:?}");

    // Edit one JPG after the backup: freeing keeps it, deletes the rest.
    std::thread::sleep(std::time::Duration::from_millis(20));
    write(&job.join("Color/DSC_0001.jpg"), 60_000, 9);
    let freed = s.editor.free_up(&t.id).await.unwrap();
    assert_eq!((freed.files, freed.bytes, freed.kept.len()), (2, 500_000, 1));
    assert!(!job.join("1_RAW").exists(), "emptied folders go too");
    assert!(job.join("Color/DSC_0001.jpg").exists(), "a file changed after the backup is never deleted");
    assert_eq!(s.editor.receipts().iter().find(|r| r.id == t.id).unwrap().freed.as_ref().unwrap().files, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn download_into_a_full_disk_stops_with_a_clear_message() {
    let s = studio().await;
    write(&s.jobs.join("2026/11/Job/1_RAW/Minhfatz/MFZ_0001.NEF"), 2_000_000, 4);
    let ticket = s
        .host
        .create_ticket(TicketSpec {
            job: "Job - Minhfatz".into(),
            root: "Jobs".into(),
            path: "2026/11/Job/1_RAW/Minhfatz".into(),
            ret: String::new(),
            get: true,
            put: false,
            days: 30,
        })
        .unwrap();
    let ticket: Ticket = ticket.to_string().parse().unwrap();
    let dest = s.work.join("ssd");

    toss_core::fsutil::FAKE_FREE_SPACE.store(1_000_000, Ordering::SeqCst);
    let t = s.editor.get(ticket.clone(), dest.clone()).await.unwrap();
    let st = t.wait().await;
    toss_core::fsutil::FAKE_FREE_SPACE.store(0, Ordering::SeqCst);
    assert_eq!(st.status, Status::Error, "{st:?}");
    let err = st.error.unwrap_or_default();
    assert!(err.contains("thiếu chỗ"), "{err}");
    assert!(!dest.join("Job - Minhfatz/MFZ_0001.NEF").exists());

    // With room again, the same link downloads normally.
    let t = s.editor.get(ticket, dest.clone()).await.unwrap();
    assert_eq!(t.wait().await.status, Status::Done);
    assert_eq!(
        std::fs::read(dest.join("Job - Minhfatz/MFZ_0001.NEF")).unwrap().len(),
        2_000_000
    );
}
