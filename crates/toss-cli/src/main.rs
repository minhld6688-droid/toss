//! `toss-cli` — thin command line front-end for `toss-core`, for tests on two
//! real machines. See `contracts/core-api.md`.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use toss_core::{
    jobs::JobStore,
    node::{
        default_data_dir, issue_admin_ticket_offline, issue_ticket_offline,
        load_or_create_secret_key,
    },
    transfer::Status,
    HostConfig, JobRecord, Node, NodeConfig, Root, Ticket, TicketSpec, TransferEvent,
};

/// Host config saved by `toss-cli host` so `toss-cli ticket` can validate specs.
const HOST_FILE: &str = "cli-host.json";

#[derive(Parser, Debug)]
#[command(name = "toss-cli", version, about = "Toss P2P file transfer (CLI)")]
struct Cli {
    /// Data directory (key, jobs.json, revoked.json). Default: OS data dir/toss.
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    /// Device name shown to the other side. Default: hostname.
    #[arg(long, global = true)]
    name: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Print endpoint id and device name.
    Id,
    /// Run in host mode until Ctrl-C, printing one line per job event.
    Host {
        /// Root as ALIAS=PATH (repeatable).
        #[arg(long = "root", value_parser = parse_root, required = true)]
        roots: Vec<Root>,
        /// Return root (where uploads land).
        #[arg(long = "return")]
        return_root: PathBuf,
        /// Max concurrent transfers.
        #[arg(long, default_value_t = 2)]
        max_jobs: u32,
        /// Max parallel streams per transfer.
        #[arg(long, default_value_t = 4)]
        max_streams: u32,
        /// Fixed UDP port to listen on (e.g. for a router port forward).
        /// Default: a random port.
        #[arg(long)]
        port: Option<u16>,
    },
    /// Create a ticket on this host (uses the config of the last `host` run).
    Ticket(TicketArgs),
    /// List jobs issued by this host.
    Jobs,
    /// Revoke a ticket by id (hex).
    Revoke {
        /// Ticket id (32 hex chars).
        id: String,
    },
    /// Download a job.
    Get {
        /// Ticket (toss1…, toss://t/…, or web link).
        ticket: String,
        /// Parent folder; files land in DEST/<job>/. Default: ~/Toss.
        #[arg(long)]
        dest: Option<PathBuf>,
    },
    /// Upload files/folders into a job's return folder.
    Put {
        /// Ticket.
        ticket: String,
        /// Files or folders.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// Remote control of a host with an admin ticket.
    Ctl {
        /// Admin ticket.
        admin: String,
        #[command(subcommand)]
        cmd: CtlCmd,
    },
}

#[derive(Args, Debug)]
struct TicketArgs {
    /// Job name (also the folder name on the receiving side).
    #[arg(long)]
    job: Option<String>,
    /// Root alias.
    #[arg(long)]
    root: Option<String>,
    /// Path under the root.
    #[arg(long, default_value = "")]
    path: String,
    /// Return sub-folder for uploads (default: job name).
    #[arg(long, default_value = "")]
    ret: String,
    /// Allow download (default if neither --get nor --put).
    #[arg(long)]
    get: bool,
    /// Allow upload.
    #[arg(long)]
    put: bool,
    /// Validity in days (0 = never). Default 7 (admin: 0).
    #[arg(long)]
    days: Option<u32>,
    /// Create an admin ticket instead (remote control).
    #[arg(long)]
    admin: bool,
    /// Also print the web link for this page host.
    #[arg(long)]
    page_host: Option<String>,
}

impl TicketArgs {
    fn spec(&self) -> Result<TicketSpec> {
        let job = self
            .job
            .clone()
            .ok_or_else(|| anyhow!("--job is required"))?;
        let root = self
            .root
            .clone()
            .ok_or_else(|| anyhow!("--root is required"))?;
        let get = self.get || !self.put;
        Ok(TicketSpec {
            job,
            root,
            path: self.path.clone(),
            ret: self.ret.clone(),
            get,
            put: self.put,
            days: self.days.unwrap_or(7),
        })
    }
}

#[derive(Subcommand, Debug)]
enum CtlCmd {
    /// Host status.
    Status,
    /// List a folder on the host.
    Ls {
        /// Root alias.
        root: String,
        /// Path under the root.
        #[arg(default_value = "")]
        path: String,
    },
    /// Create a job ticket on the host.
    Ticket(TicketArgs),
    /// List jobs on the host.
    Jobs,
    /// Revoke a ticket on the host.
    Revoke {
        /// Ticket id (hex).
        id: String,
    },
}

fn parse_root(s: &str) -> Result<Root, String> {
    let (alias, path) = s.split_once('=').ok_or("expected ALIAS=PATH")?;
    Ok(Root {
        alias: alias.to_string(),
        path: PathBuf::from(path),
    })
}

fn parse_id(s: &str) -> Result<[u8; 16]> {
    let v = hex::decode(s.trim()).context("id must be hex")?;
    v.try_into()
        .map_err(|_| anyhow!("id must be 16 bytes (32 hex chars)"))
}

fn parse_ticket(s: &str) -> Result<Ticket> {
    Ticket::from_str(s).map_err(|e| anyhow!("{e}"))
}

fn device_name(cli: &Cli) -> String {
    cli.name.clone().unwrap_or_else(|| {
        gethostname::gethostname()
            .into_string()
            .unwrap_or_else(|_| "toss-cli".into())
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,toss_core=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let data_dir = cli.data_dir.clone().unwrap_or_else(default_data_dir);
    let name = device_name(&cli);
    let cfg = NodeConfig {
        data_dir: data_dir.clone(),
        device_name: name.clone(),
        app_version: toss_core::VERSION.to_string(),
    };
    match &cli.cmd {
        Cmd::Id => {
            let key = load_or_create_secret_key(&data_dir)?;
            println!("{}  {}", key.public(), name);
        }
        Cmd::Host {
            roots,
            return_root,
            max_jobs,
            max_streams,
            port,
        } => {
            let mut hc = HostConfig::new(roots.clone(), return_root.clone());
            hc.max_parallel_jobs = *max_jobs;
            hc.max_parallel_streams = *max_streams;
            hc.bind_port = *port;
            run_host(cfg, hc).await?;
        }
        Cmd::Ticket(args) => {
            let t = if args.admin {
                issue_admin_ticket_offline(&data_dir, args.days.unwrap_or(0))?
            } else {
                let hc = load_host_file(&data_dir)?;
                issue_ticket_offline(&data_dir, &hc, args.spec()?)?
            };
            print_ticket(&t, args.page_host.as_deref());
        }
        Cmd::Jobs => print_jobs(&JobStore::open(&data_dir).list()),
        Cmd::Revoke { id } => {
            JobStore::open(&data_dir).revoke(parse_id(id)?)?;
            println!("revoked {id}");
        }
        Cmd::Get { ticket, dest } => {
            let ticket = parse_ticket(ticket)?;
            let dest = match dest {
                Some(d) => d.clone(),
                None => dirs::home_dir()
                    .ok_or_else(|| anyhow!("no home dir; pass --dest"))?
                    .join("Toss"),
            };
            let node = Node::start(cfg).await?;
            let t = node.get(ticket, dest).await?;
            finish_transfer(node, t).await?;
        }
        Cmd::Put { ticket, paths } => {
            let ticket = parse_ticket(ticket)?;
            let node = Node::start(cfg).await?;
            let t = node.put(ticket, paths.clone()).await?;
            finish_transfer(node, t).await?;
        }
        Cmd::Ctl { admin, cmd } => {
            let admin = parse_ticket(admin)?;
            let node = Node::start(cfg).await?;
            let res = run_ctl(&node, admin, cmd).await;
            node.shutdown().await;
            res?;
        }
    }
    Ok(())
}

fn load_host_file(data_dir: &Path) -> Result<HostConfig> {
    let p = data_dir.join(HOST_FILE);
    let bytes = std::fs::read(&p)
        .with_context(|| format!("{} not found; run `toss-cli host …` first", p.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", p.display()))
}

fn print_ticket(t: &Ticket, page_host: Option<&str>) {
    println!("{t}");
    eprintln!(
        "id: {}  job: {}  deep link: {}",
        t.id_hex(),
        t.body.job,
        t.deep_link()
    );
    if let Some(h) = page_host {
        println!("{}", t.web_url(h));
    }
}

fn print_jobs(jobs: &[JobRecord]) {
    for j in jobs {
        let mode = match (j.get, j.put) {
            (true, true) => "get+put",
            (true, false) => "get",
            (false, true) => "put",
            _ => "admin",
        };
        let last = j
            .last
            .as_ref()
            .map(|l| {
                format!(
                    "last: {:?} by {} {} files {} bytes {}",
                    l.op,
                    l.device,
                    l.files,
                    l.bytes,
                    if l.ok { "ok" } else { "FAILED" }
                )
            })
            .unwrap_or_else(|| "never used".into());
        println!(
            "{}  {:<24} {}:{} [{}] exp={} {}{}",
            hex::encode(j.id),
            j.job,
            j.root,
            j.path,
            mode,
            j.exp,
            if j.revoked { "REVOKED " } else { "" },
            last
        );
    }
}

async fn run_host(cfg: NodeConfig, hc: HostConfig) -> Result<()> {
    let data_dir = cfg.data_dir.clone();
    let node = Node::start_with_port(cfg, hc.bind_port).await?;
    node.set_host(Some(hc.clone())).await?;
    std::fs::write(data_dir.join(HOST_FILE), serde_json::to_vec_pretty(&hc)?)
        .context("save host config")?;
    println!(
        "host {} ({}) ready{}; Ctrl-C to stop",
        node.endpoint_id(),
        node.device_name(),
        hc.bind_port
            .map(|p| format!(" on UDP port {p}"))
            .unwrap_or_default()
    );
    let mut rx = node.subscribe();
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    if let Some(line) = event_line(&ev) {
                        println!("{line}");
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
    }
    println!("stopping");
    node.shutdown().await;
    Ok(())
}

fn event_line(ev: &TransferEvent) -> Option<String> {
    Some(match ev {
        TransferEvent::Started {
            id,
            job,
            op,
            side,
            peer,
        } => {
            format!("{id} started {op:?} {job:?} side={side:?} peer={peer}")
        }
        TransferEvent::Manifest {
            id,
            files,
            bytes,
            to_do_files,
            to_do_bytes,
        } => {
            format!(
                "{id} manifest {files} files {bytes} B, to do {to_do_files} files {to_do_bytes} B"
            )
        }
        TransferEvent::Progress { .. } => return None,
        TransferEvent::Done {
            id,
            files,
            bytes,
            elapsed_s,
            dest,
        } => format!(
            "{id} done {files} files {bytes} B in {elapsed_s}s{}",
            dest.as_ref()
                .map(|d| format!(" -> {d}"))
                .unwrap_or_default()
        ),
        TransferEvent::Error {
            id,
            code,
            msg,
            retry_in_s,
        } => format!(
            "{id} error {code:?}: {msg}{}",
            retry_in_s
                .map(|s| format!(" (retry in {s}s)"))
                .unwrap_or_default()
        ),
        TransferEvent::Paused { id } => format!("{id} paused"),
        TransferEvent::HostStatus(s) => {
            format!(
                "status active={} queued={} uptime={}s",
                s.active, s.queued, s.uptime_s
            )
        }
    })
}

async fn finish_transfer(node: Node, t: toss_core::Transfer) -> Result<()> {
    let mut rx = node.subscribe();
    let mut last_print = Instant::now() - Duration::from_secs(10);
    let wait = t.wait();
    tokio::pin!(wait);
    let st = loop {
        tokio::select! {
            st = &mut wait => break st,
            _ = tokio::signal::ctrl_c() => {
                eprintln!("interrupted; pausing (re-run the same command to resume)");
                node.pause(&t.id).await.ok();
                node.shutdown().await;
                bail!("interrupted");
            }
            ev = rx.recv() => {
                let ev = match ev {
                    Ok(ev) => ev,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    // No more events (cannot happen while the node lives): just wait.
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break (&mut wait).await,
                };
                match &ev {
                    TransferEvent::Progress { id, bytes_done, bytes_total, files_done, files_total, speed_bps, eta_s, path } if *id == t.id => {
                        if last_print.elapsed() >= Duration::from_secs(1) {
                            last_print = Instant::now();
                            eprintln!(
                                "{:>5.1}%  {files_done}/{files_total} files  {}/{}  {}/s  eta {}  {path:?}",
                                if *bytes_total > 0 { *bytes_done as f64 * 100.0 / *bytes_total as f64 } else { 100.0 },
                                human(*bytes_done), human(*bytes_total), human(*speed_bps),
                                eta_s.map(|s| format!("{s}s")).unwrap_or_else(|| "?".into()),
                            );
                        }
                    }
                    other => {
                        if let Some(line) = event_line(other) {
                            if line.starts_with(&t.id) { eprintln!("{line}"); }
                        }
                    }
                }
            }
        }
    };
    node.shutdown().await;
    match st.status {
        Status::Done => {
            println!(
                "done: {} files, {}{}",
                st.files_total,
                human(st.bytes_total),
                st.dest.map(|d| format!(" -> {d}")).unwrap_or_default()
            );
            Ok(())
        }
        _ => bail!(
            "transfer failed: {}",
            st.error.unwrap_or_else(|| format!("{:?}", st.status))
        ),
    }
}

async fn run_ctl(node: &Node, admin: Ticket, cmd: &CtlCmd) -> Result<()> {
    let ctl = node.ctl(admin).await?;
    match cmd {
        CtlCmd::Status => {
            let s = ctl.status().await?;
            println!(
                "{} v{} roots={} active={} queued={} uptime={}s",
                s.device,
                s.version,
                s.roots.join(","),
                s.active,
                s.queued,
                s.uptime_s
            );
        }
        CtlCmd::Ls { root, path } => {
            for e in ctl.list_dir(root, path).await? {
                if e.is_dir {
                    println!("{}/", e.name);
                } else {
                    println!("{}  {}", e.name, human(e.size));
                }
            }
        }
        CtlCmd::Ticket(args) => {
            if args.admin {
                bail!("admin tickets can only be created on the host itself");
            }
            let t = ctl.create_ticket(args.spec()?).await?;
            print_ticket(&t, args.page_host.as_deref());
        }
        CtlCmd::Jobs => print_jobs(&ctl.list_jobs().await?),
        CtlCmd::Revoke { id } => {
            ctl.revoke(parse_id(id)?).await?;
            println!("revoked {id}");
        }
    }
    ctl.close().await;
    Ok(())
}

fn human(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}
