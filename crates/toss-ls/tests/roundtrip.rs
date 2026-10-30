#[tokio::test]
async fn folder_roundtrip() {
    let src = tempfile::tempdir().unwrap();
    let job = src.path().join("Lop 12A1");
    std::fs::create_dir_all(job.join("RAW/cam1")).unwrap();
    std::fs::write(job.join("RAW/cam1/IMG_0001.CR3"), vec![7u8; 3_000_000]).unwrap();
    std::fs::write(job.join("note.txt"), b"hello").unwrap();
    let dst = tempfile::tempdir().unwrap();
    let r = toss_ls::receive("May tong", 0, dst.path().to_path_buf()).await.unwrap();
    let n = toss_ls::send_folder("Editor", "127.0.0.1", r.handle.port(), &job).await.unwrap();
    assert_eq!(n, 2);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(std::fs::read(dst.path().join("Lop 12A1/note.txt")).unwrap(), b"hello");
    assert_eq!(std::fs::metadata(dst.path().join("Lop 12A1/RAW/cam1/IMG_0001.CR3")).unwrap().len(), 3_000_000);
}
