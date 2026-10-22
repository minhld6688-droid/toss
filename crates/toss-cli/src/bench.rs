//! `toss-cli bench`: two nodes in one process over loopback, one get, JSON
//! result (contracts/speed-v0.4.md §4.4). Loopback numbers are CPU-bound: they
//! show the pipeline's overhead, not what a real link does.

use std::{
    io::Write,
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use clap::Args;
use iroh::{EndpointAddr, TransportAddr};
use serde::Serialize;
use toss_core::{
    net::Congestion, transfer::Status, ConnPath, HostConfig, NetConfig, Node, NodeConfig, Root,
    TicketSpec, TransferEvent,
};

/// Arguments of `toss-cli bench`.
#[derive(Args, Debug)]
pub struct BenchArgs {
    /// Size of each file, MiB (fractions allowed, e.g. 0.1).
    #[arg(long, default_value_t = 2048.0)]
    size_mb: f64,
    /// Size of each file in KiB (overrides --size-mb).
    #[arg(long)]
    size_kb: Option<u64>,
    /// Number of files.
    #[arg(long, default_value_t = 1)]
    files: u32,
    /// Simulated RTT. Ignored: delay injection is not possible in-process on
    /// loopback (the output says so).
    #[arg(long)]
    rtt_ms: Option<u32>,
    /// Data streams per transfer (tuning.streams); default from `Tuning`.
    #[arg(long)]
    streams: Option<u32>,
    /// Use Cubic instead of BBR.
    #[arg(long)]
    cubic: bool,
    /// Read/write chunk, KiB (tuning.chunk_kb).
    #[arg(long)]
    chunk_kb: Option<u32>,
    /// Per-stream receive window, MiB (tuning.stream_window_mb).
    #[arg(long)]
    stream_window_mb: Option<u32>,
    /// Split files above this size, MiB (tuning.range_split_mb).
    #[arg(long)]
    range_split_mb: Option<u32>,
    /// Scratch folder (default: system temp dir). Needs 2 × the data size.
    #[arg(long)]
    dir: Option<PathBuf>,
    /// Keep the scratch folder.
    #[arg(long)]
    keep: bool,
    /// Sparse source files (all zeros, no disk space): for a RAM disk
    /// (`--dir /dev/shm`) when the disk is too small. Hashing and transfer
    /// costs are the same; only the host's disk reads become free.
    #[arg(long)]
    sparse: bool,
}

#[derive(Serialize)]
struct BenchResult {
    path: ConnPath,
    rtt_ms: Option<u32>,
    mb_s: f64,
    cpu_pct: Option<f64>,
    streams: u32,
    files: u32,
    bytes: u64,
    elapsed_s: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

/// Runs the benchmark and prints one JSON line.
pub async fn run(args: &BenchArgs) -> Result<()> {
    let size = match args.size_kb {
        Some(kb) => kb * 1024,
        None => (args.size_mb * 1024.0 * 1024.0) as u64,
    };
    if args.files == 0 {
        bail!("--files must be > 0");
    }
    let base = args
        .dir
        .clone()
        .unwrap_or_else(std::env::temp_dir)
        .join(format!(
            "toss-bench-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
    let res = run_in(args, &base, size).await;
    if !args.keep {
        let _ = std::fs::remove_dir_all(&base);
    }
    let r = res?;
    println!("{}", serde_json::to_string(&r)?);
    Ok(())
}

async fn run_in(args: &BenchArgs, base: &Path, size: u64) -> Result<BenchResult> {
    let src = base.join("src/job");
    let dest = base.join("dest");
    std::fs::create_dir_all(&src).with_context(|| format!("create {}", src.display()))?;
    std::fs::create_dir_all(&dest)?;
    eprintln!(
        "bench: writing {} × {} bytes under {}",
        args.files,
        size,
        base.display()
    );
    for i in 0..args.files {
        let p = src.join(format!("IMG_{i:05}.CR3"));
        if args.sparse {
            std::fs::File::create(&p)?.set_len(size)?;
        } else {
            write_file(&p, size, i as u64)?;
        }
    }

    let mut net = NetConfig::default();
    if let Some(s) = args.streams {
        net.tuning.streams = s;
    }
    if args.cubic {
        net.tuning.congestion = Congestion::Cubic;
    }
    if let Some(c) = args.chunk_kb {
        net.tuning.chunk_kb = c;
    }
    if let Some(w) = args.stream_window_mb {
        net.tuning.stream_window_mb = w;
    }
    if let Some(r) = args.range_split_mb {
        net.tuning.range_split_mb = r;
    }
    let cfg = |name: &str| NodeConfig {
        data_dir: base.join(format!("{name}-data")),
        device_name: name.to_string(),
        app_version: toss_core::VERSION.to_string(),
        net: net.clone(),
    };
    let host = Node::start(cfg("bench-host")).await?;
    let client = Node::start(cfg("bench-client")).await?;
    host.set_host(Some(HostConfig::new(
        vec![Root {
            alias: "BENCH".into(),
            path: base.join("src"),
        }],
        base.join("return"),
    )))
    .await?;
    client.add_peer_addr(loopback_addr(&host));
    let ticket = host.create_ticket(TicketSpec {
        job: "bench".into(),
        root: "BENCH".into(),
        path: "job".into(),
        ret: String::new(),
        get: true,
        put: false,
        days: 1,
    })?;

    let mut rx = client.subscribe();
    let cpu0 = cpu_time();
    let started = Instant::now();
    let t = client.get(ticket, dest.clone()).await?;
    let mut max_streams = 0u32;
    let mut last_rtt = None;
    let mut last_path = ConnPath::Unknown;
    let wait = t.wait();
    tokio::pin!(wait);
    let st = loop {
        tokio::select! {
            st = &mut wait => break st,
            ev = rx.recv() => {
                if let Ok(TransferEvent::Progress { id, path, rtt_ms, streams, .. }) = ev {
                    if id == t.id {
                        max_streams = max_streams.max(streams);
                        if rtt_ms.is_some() { last_rtt = rtt_ms; }
                        last_path = path;
                    }
                }
            }
        }
    };
    let elapsed = started.elapsed();
    let cpu = cpu_time().zip(cpu0).map(|(b, a)| b.saturating_sub(a));
    if st.status != Status::Done {
        bail!("transfer ended {:?}: {:?}", st.status, st.error);
    }
    let path = if st.path == ConnPath::Unknown {
        last_path
    } else {
        st.path
    };
    client.shutdown().await;
    host.shutdown().await;
    let bytes = size * args.files as u64;
    let secs = elapsed.as_secs_f64().max(1e-9);
    Ok(BenchResult {
        path,
        rtt_ms: st.rtt_ms.or(last_rtt),
        mb_s: round2(bytes as f64 / (1024.0 * 1024.0) / secs),
        cpu_pct: cpu.map(|c| round2(c.as_secs_f64() / secs * 100.0)),
        streams: max_streams.max(st.streams),
        files: args.files,
        bytes,
        elapsed_s: round2(secs),
        note: args
            .rtt_ms
            .map(|r| format!("--rtt-ms {r} ignored: no delay injection on in-process loopback")),
    })
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
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

/// Writes `size` pseudo-random bytes (distinct per file and per MiB).
fn write_file(path: &Path, size: u64, seed: u64) -> Result<()> {
    const BLOCK: usize = 1 << 20;
    let mut f = std::io::BufWriter::new(
        std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?,
    );
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut block = vec![0u8; BLOCK];
    for chunk in block.chunks_mut(8) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let b = x.to_le_bytes();
        chunk.copy_from_slice(&b[..chunk.len()]);
    }
    let mut left = size;
    let mut n = 0u64;
    while left > 0 {
        block[..8].copy_from_slice(&(seed ^ (n << 20)).to_le_bytes());
        let len = left.min(BLOCK as u64) as usize;
        f.write_all(&block[..len])?;
        left -= len as u64;
        n += 1;
    }
    f.flush()?;
    Ok(())
}

/// Process CPU time (user + system), Linux only.
fn cpu_time() -> Option<Duration> {
    let s = std::fs::read_to_string("/proc/self/stat").ok()?;
    // Fields after the `(comm)`; utime and stime are fields 14 and 15 (1-based).
    let rest = &s[s.rfind(')')? + 2..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    let utime: u64 = f.get(11)?.parse().ok()?;
    let stime: u64 = f.get(12)?.parse().ok()?;
    // USER_HZ is 100 on every mainstream Linux.
    Some(Duration::from_millis((utime + stime) * 10))
}
