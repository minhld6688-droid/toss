#[tokio::main]
async fn main() {
    let mb: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(2048);
    let files: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(1);
    let src = tempfile::tempdir().unwrap();
    let job = src.path().join("job");
    std::fs::create_dir_all(&job).unwrap();
    for i in 0..files { std::fs::write(job.join(format!("f{i}.bin")), vec![9u8; mb * 1024 * 1024 / files]).unwrap(); }
    let dst = tempfile::tempdir().unwrap();
    let r = toss_ls::receive("rx", 0, dst.path().to_path_buf()).await.unwrap();
    let t = std::time::Instant::now();
    toss_ls::send_folder("tx", "127.0.0.1", r.handle.port(), &job).await.unwrap();
    let s = t.elapsed().as_secs_f64();
    println!("{{\"localsend_http\":true,\"mb\":{mb},\"files\":{files},\"secs\":{s:.2},\"MBps\":{:.0}}}", mb as f64 / s);
}
