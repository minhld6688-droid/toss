//! `toss-cli` — thin command line front-end for `toss-core`, for tests on two
//! real machines. See `contracts/core-api.md`.

mod bench;

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use toss_core::{
    contacts::{create_invite_offline, read_contacts, resolve_in},
    jobs::JobStore,
    node::{
        default_data_dir, issue_admin_ticket_offline, issue_ticket_offline,
        load_or_create_secret_key,
    },
    transfer::Status,
    HostAddr, HostConfig, JobRecord, NetConfig, Node, NodeConfig, Root, Ticket, TicketSpec,
    TransferEvent,
};

/// Host config saved by `toss-cli host` so `toss-cli ticket` can validate specs,
/// plus the running host's last known relay/direct addresses for tickets.
const HOST_FILE: &str = "cli-host.json";

/// Contents of [`HOST_FILE`]: the host config (flattened, so files written by
/// older versions still load) and the last known addresses.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct HostFile {
    #[serde(flatten)]
    cfg: HostConfig,
    /// Last known relay URL + direct addresses of the running host.
    #[serde(default)]
    addr: HostAddr,
    /// Unix seconds when `addr` was written (0 = never).
    #[serde(default)]
    addr_updated: u64,
}

#[derive(Parser, Debug)]
#[command(name = "toss-cli", version, about = "Toss P2P file transfer (CLI)")]
struct Cli {
    /// Data directory (key, jobs.json, revoked.json). Default: OS data dir/toss.
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    /// Device name shown to the other side. Default: hostname.
    #[arg(long, global = true)]
    name: Option<String>,
    /// get/put: give up (exit non-zero with the last error) once the host has
    /// been unreachable or busy for this many seconds in a row. 0 = keep
    /// retrying as the app does (busy: 1 h, offline: 24 h).
    #[arg(long, global = true, default_value_t = 0, value_name = "SECS")]
    max_wait: u64,
    /// Own relay server URL, tried before n0's relays (repeatable).
    #[arg(long = "relay", global = true, value_name = "URL")]
    relays: Vec<String>,
    /// Disable mDNS same-LAN discovery.
    #[arg(long, global = true)]
    no_mdns: bool,
    /// Disable UPnP / NAT-PMP / PCP port mapping.
    #[arg(long, global = true)]
    no_portmap: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Print endpoint id and device name, then the running host's relay URL.
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
    /// Network diagnostics as JSON (relay, NAT, port mapping, likelihood of
    /// direct connections). Uses a throwaway identity on a random port, so it
    /// can run next to a running host. With --ticket, also dials that host and
    /// adds a "connect" report (path, RTT, reason).
    Diag {
        /// Ticket of the host to test (toss1…, toss://t/…, or web link).
        #[arg(long)]
        ticket: Option<String>,
    },
    /// Loopback benchmark: two nodes in this process, one get; prints JSON.
    Bench(bench::BenchArgs),
    /// Create a pairing invite (Danh bạ máy). By default it is signed offline
    /// with the addresses of the last `toss-cli host` run on this data dir
    /// (that host accepts the pairings); with --wait this process starts a
    /// node, prints an invite with its live addresses and stays up to accept
    /// pairings and pushes until Ctrl-C.
    Invite {
        /// Validity in days (0 = never).
        #[arg(long, default_value_t = 7)]
        days: u32,
        /// Also print the web link for this page host.
        #[arg(long)]
        page_host: Option<String>,
        /// Run a node and wait for pairings / pushes.
        #[arg(long)]
        wait: bool,
        /// With --wait: where pushes from contacts land. Default: ~/Toss.
        #[arg(long)]
        inbox: Option<PathBuf>,
    },
    /// Pair with the device that issued an invite (tossinv1…, toss://i/…, or web link).
    Pair {
        /// The invite.
        invite: String,
    },
    /// List paired devices (contacts.json of this data dir).
    Contacts,
    /// Send files/folders to a contact (id, unique id prefix or name). Waits
    /// until the contact is online; it may have to accept the files.
    Send {
        /// Contact id, id prefix or name.
        contact: String,
        /// Files or folders.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
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
        net: NetConfig {
            relay_urls: cli.relays.clone(),
            mdns: !cli.no_mdns,
            portmap: !cli.no_portmap,
            ..Default::default()
        },
    };
    match &cli.cmd {
        Cmd::Id => {
            let key = load_or_create_secret_key(&data_dir)?;
            println!("{}  {}", key.public(), name);
            match std::fs::read(data_dir.join(HOST_FILE))
                .ok()
                .and_then(|b| serde_json::from_slice::<HostFile>(&b).ok())
            {
                Some(f) if f.addr_updated > 0 => {
                    println!(
                        "relay: {}",
                        if f.addr.relay.is_empty() {
                            "none"
                        } else {
                            &f.addr.relay
                        }
                    );
                    println!(
                        "direct addresses: {} ({} as of {}s ago, written by `toss-cli host`)",
                        f.addr.addrs.len(),
                        f.addr.addrs.join(" "),
                        now_unix().saturating_sub(f.addr_updated)
                    );
                }
                _ => println!("relay: unknown (no `toss-cli host` has run with this data dir)"),
            }
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
                let addr = load_host_file(&data_dir)
                    .map(|f| f.addr)
                    .unwrap_or_default();
                issue_admin_ticket_offline(&data_dir, args.days.unwrap_or(0), &addr)?
            } else {
                let hf = load_host_file(&data_dir)?;
                if hf.addr.is_empty() {
                    eprintln!(
                        "warning: no relay/direct address known for this host (is `toss-cli host` \
                         running and online?); clients will depend on DNS discovery"
                    );
                }
                issue_ticket_offline(&data_dir, &hf.cfg, args.spec()?, &hf.addr)?
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
            finish_transfer(node, t, cli.max_wait).await?;
        }
        Cmd::Put { ticket, paths } => {
            let ticket = parse_ticket(ticket)?;
            let node = Node::start(cfg).await?;
            let t = node.put(ticket, paths.clone()).await?;
            finish_transfer(node, t, cli.max_wait).await?;
        }
        Cmd::Ctl { admin, cmd } => {
            let admin = parse_ticket(admin)?;
            let node = Node::start(cfg).await?;
            let res = run_ctl(&node, admin, cmd).await;
            node.shutdown().await;
            res?;
        }
        Cmd::Diag { ticket } => {
            let ticket = ticket.as_deref().map(parse_ticket).transpose()?;
            run_diag(cfg, ticket).await?;
        }
        Cmd::Bench(args) => bench::run(args).await?,
        Cmd::Invite {
            days,
            page_host,
            wait,
            inbox,
        } => {
            let page_host = page_host.as_deref().unwrap_or("");
            if *wait {
                let node = Node::start(cfg).await?;
                node.set_inbox_dir(inbox.clone().unwrap_or_else(default_inbox));
                node.wait_online(Duration::from_secs(15)).await;
                let info = node.create_invite(*days, page_host)?;
                print_invite(&info, !page_host.is_empty());
                println!(
                    "waiting for pairings and pushes (inbox: {}); Ctrl-C to stop",
                    node.inbox_dir().display()
                );
                serve_events(&node, true).await;
                node.shutdown().await;
            } else {
                let addr = load_host_file(&data_dir)
                    .map(|f| f.addr)
                    .unwrap_or_default();
                if addr.is_empty() {
                    eprintln!(
                        "warning: no address known for this device (no `toss-cli host` has run \
                         with this data dir); the pairing device will depend on DNS discovery. \
                         Use --wait to run a node for the pairing instead."
                    );
                }
                let info = create_invite_offline(&data_dir, &name, *days, page_host, &addr)?;
                print_invite(&info, !page_host.is_empty());
            }
        }
        Cmd::Pair { invite } => {
            let node = Node::start(cfg).await?;
            node.wait_online(Duration::from_secs(5)).await;
            let res = node.accept_invite(invite).await;
            node.shutdown().await;
            let c = res?;
            println!("paired with {} ({}, {})", c.name, c.platform, c.id);
        }
        Cmd::Contacts => {
            let list = read_contacts(&data_dir);
            if list.is_empty() {
                println!("no contacts (pair with `toss-cli pair <invite>`)");
            }
            for c in list {
                println!(
                    "{}  {:<24} {:<8} auto_accept={} last_seen={}{}",
                    c.id,
                    c.name,
                    c.platform,
                    c.auto_accept,
                    c.last_seen,
                    if c.their_name != c.name {
                        format!(" (reports as {})", c.their_name)
                    } else {
                        String::new()
                    }
                );
            }
        }
        Cmd::Send { contact, paths } => {
            let c = resolve_in(&read_contacts(&data_dir), contact).ok_or_else(|| {
                anyhow!("no contact matches {contact:?} (see `toss-cli contacts`)")
            })?;
            let node = Node::start(cfg).await?;
            node.wait_online(Duration::from_secs(5)).await;
            eprintln!("sending to {} ({})", c.name, c.id);
            let t = node.send_to_contact(&c.id, paths.clone()).await?;
            finish_transfer(node, t, cli.max_wait).await?;
        }
    }
    Ok(())
}

/// `~/Toss` (inbox for pushes when not hosting).
fn default_inbox() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join("Toss"))
        .unwrap_or_else(|| PathBuf::from("Toss"))
}

fn print_invite(info: &toss_core::InviteInfo, web: bool) {
    println!("{}", info.invite);
    eprintln!(
        "invite from {:?}, expires {}; deep link: {}",
        info.name,
        if info.exp == 0 {
            "never".to_string()
        } else {
            format!("in {}h", info.exp.saturating_sub(now_unix()) / 3600)
        },
        info.deep_link
    );
    if web {
        println!("{}", info.web_url);
    }
}

/// Prints node events until Ctrl-C. `auto_accept`: pushes that wait for an
/// answer (contact with auto-accept off) are accepted.
async fn serve_events(node: &Node, auto_accept: bool) {
    let mut rx = node.subscribe();
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    if let TransferEvent::PushIncoming(p) = &ev {
                        if auto_accept {
                            if let Err(e) = node.answer_push(&p.push_id, true) {
                                eprintln!("could not accept push {}: {e:#}", p.push_id);
                            }
                        }
                    }
                    if let Some(line) = event_line(&ev) {
                        println!("{line}");
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
    }
}

/// `toss-cli diag`: prints `NetDiag` as JSON (+ `"connect": ConnectReport`
/// with `--ticket`). Runs on a temporary data dir: starting an endpoint with
/// this machine's key next to a running host would publish a competing
/// pkarr record for it.
async fn run_diag(cfg: NodeConfig, ticket: Option<Ticket>) -> Result<()> {
    let tmp = std::env::temp_dir().join(format!("toss-diag-{}", std::process::id()));
    let node = Node::start(NodeConfig {
        data_dir: tmp.clone(),
        ..cfg
    })
    .await?;
    let res = async {
        node.wait_online(Duration::from_secs(10)).await;
        node.wait_net_report(Duration::from_secs(10)).await;
        // Give the port mapping a few seconds to answer.
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut diag = node.diagnostics().await;
        while diag.portmap.status == "probing" && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(500)).await;
            diag = node.diagnostics().await;
        }
        let mut out = serde_json::to_value(&diag)?;
        if let Some(t) = ticket {
            let rep = node.test_connect(&t).await;
            out["connect"] = serde_json::to_value(&rep)?;
        }
        println!("{}", serde_json::to_string_pretty(&out)?);
        anyhow::Ok(())
    }
    .await;
    node.shutdown().await;
    let _ = std::fs::remove_dir_all(&tmp);
    res
}

fn load_host_file(data_dir: &Path) -> Result<HostFile> {
    let p = data_dir.join(HOST_FILE);
    let bytes = std::fs::read(&p)
        .with_context(|| format!("{} not found; run `toss-cli host …` first", p.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", p.display()))
}

fn save_host_file(data_dir: &Path, f: &HostFile) -> Result<()> {
    // Temp file + rename: `toss-cli ticket` may read it at any moment.
    let p = data_dir.join(HOST_FILE);
    let tmp = data_dir.join(format!("{HOST_FILE}.tmp{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(f)?)
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &p).with_context(|| format!("rename to {}", p.display()))
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn print_ticket(t: &Ticket, page_host: Option<&str>) {
    println!("{t}");
    eprintln!(
        "id: {}  job: {}  deep link: {}",
        t.id_hex(),
        t.body.job,
        t.deep_link()
    );
    eprintln!(
        "host relay: {}  direct addresses: {}{}",
        if t.body.relay.is_empty() {
            "none"
        } else {
            &t.body.relay
        },
        t.body.addrs.len(),
        if t.body.addrs.is_empty() {
            String::new()
        } else {
            format!(" ({})", t.body.addrs.join(" "))
        }
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
    let node = Node::start_host(cfg, hc.bind_port).await?;
    node.set_host(Some(hc.clone())).await?;
    // v0.5: pushes from contacts land in the return root, accepted automatically.
    node.set_inbox_dir(hc.return_root.clone());
    let mut hf = HostFile {
        cfg: hc.clone(),
        addr: node.host_addr(),
        addr_updated: now_unix(),
    };
    save_host_file(&data_dir, &hf).context("save host config")?;
    // Tickets carry the relay URL: give the relay connection a moment first.
    let online = node.wait_online(Duration::from_secs(15)).await;
    hf.addr = node.host_addr();
    hf.addr_updated = now_unix();
    save_host_file(&data_dir, &hf).context("save host config")?;
    println!(
        "host {} ({}) ready{}; Ctrl-C to stop",
        node.endpoint_id(),
        node.device_name(),
        hc.bind_port
            .map(|p| format!(" on UDP port {p}"))
            .unwrap_or_default()
    );
    println!(
        "relay: {}{}  direct addresses: {} ({})",
        if hf.addr.relay.is_empty() {
            "none"
        } else {
            &hf.addr.relay
        },
        if online { "" } else { " (not connected yet)" },
        hf.addr.addrs.len(),
        hf.addr.addrs.join(" ")
    );
    // Keep `cli-host.json` current so `toss-cli ticket` embeds live addresses.
    {
        let node = node.clone();
        let data_dir = data_dir.clone();
        tokio::spawn(async move {
            let mut cur = hf.addr.clone();
            while let Some(next) = node.host_addr_changed(&cur).await {
                println!("addresses changed: {}", next.summary());
                hf.addr = next.clone();
                hf.addr_updated = now_unix();
                if let Err(e) = save_host_file(&data_dir, &hf) {
                    eprintln!("could not update {HOST_FILE}: {e:#}");
                }
                cur = next;
            }
        });
    }
    serve_events(&node, true).await;
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
            contact,
        } => {
            format!(
                "{id} started {op:?} {job:?} side={side:?} peer={peer}{}",
                contact
                    .as_ref()
                    .map(|c| format!(" contact={c}"))
                    .unwrap_or_default()
            )
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
        TransferEvent::ContactsChanged {} => "contacts changed".to_string(),
        TransferEvent::ContactPresence { id, online, rtt_ms } => format!(
            "contact {id} {}{}",
            if *online { "online" } else { "offline" },
            rtt_ms.map(|r| format!(" rtt={r}ms")).unwrap_or_default()
        ),
        TransferEvent::PushIncoming(p) => format!(
            "push {} from {} ({}): {:?} {} files {} B",
            p.push_id, p.from_name, p.from_id, p.job, p.files, p.bytes
        ),
        TransferEvent::LanBlocked { blocked } => format!("local network blocked: {blocked}"),
        TransferEvent::UpdateRequested { device } => {
            format!("update requested by {device} (only the desktop app updates itself)")
        }
    })
}

/// Waits for the transfer, printing progress. With `max_wait > 0`, gives up
/// once the host has been unreachable/busy for that many seconds in a row
/// (counted from the start, reset whenever a session gets as far as the
/// manifest), pausing the transfer (`.part` files are kept) and failing with
/// the last error.
async fn finish_transfer(node: Node, t: toss_core::Transfer, max_wait: u64) -> Result<()> {
    let mut rx = node.subscribe();
    let mut last_print = Instant::now() - Duration::from_secs(10);
    let wait = t.wait();
    tokio::pin!(wait);
    let limit = (max_wait > 0).then(|| Duration::from_secs(max_wait));
    let mut waiting_since = Some(Instant::now());
    let mut last_error: Option<String> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let st = loop {
        tokio::select! {
            st = &mut wait => break st,
            _ = tick.tick(), if limit.is_some() => {
                if let (Some(limit), Some(since)) = (limit, waiting_since) {
                    if since.elapsed() >= limit {
                        let why = last_error
                            .clone()
                            .or_else(|| node.transfer(&t.id).and_then(|s| s.error))
                            .unwrap_or_else(|| "still connecting to the host".into());
                        eprintln!("giving up after {max_wait}s without reaching the host (--max-wait); .part files are kept");
                        node.pause(&t.id).await.ok();
                        node.shutdown().await;
                        bail!("host not reachable within {max_wait}s: {why}");
                    }
                }
            }
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
                    TransferEvent::Manifest { id, .. } if *id == t.id => waiting_since = None,
                    TransferEvent::Error { id, msg, retry_in_s: Some(_), .. } if *id == t.id => {
                        last_error = Some(msg.clone());
                        waiting_since.get_or_insert_with(Instant::now);
                    }
                    _ => {}
                }
                match &ev {
                    TransferEvent::Progress { id, bytes_done, bytes_total, files_done, files_total, speed_bps, eta_s, path, .. } if *id == t.id => {
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
            // v0.4: final connection path (after any relay -> direct upgrade), RTT and streams;
            // e2e.yml reads `path=` from this line.
            println!(
                "done: {} files, {}{} path={} rtt={} streams={}",
                st.files_total,
                human(st.bytes_total),
                st.dest.map(|d| format!(" -> {d}")).unwrap_or_default(),
                format!("{:?}", st.path).to_lowercase(),
                st.rtt_ms
                    .map(|r| format!("{r}ms"))
                    .unwrap_or_else(|| "?".into()),
                st.streams,
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
