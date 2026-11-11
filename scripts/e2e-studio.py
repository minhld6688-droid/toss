#!/usr/bin/env python3
"""Studio pipeline end to end with real processes.

The studio machine is `toss-cli host` (what the Toss app runs on Windows); the editor is
`toss-engine` (what MFZ Pulse runs on a Mac). Checks, in order:

1. a shooter link is recognised before anything happens (peek), then downloads into the folder
   the editor picked;
2. the editor's app is killed with SIGKILL mid-download and started again: the download carries
   on from where it stopped and every file matches the studio's byte for byte;
3. a backup link becomes a place to drop folders into (b:<id>), a job folder sent there lands in
   Backup/<editor>/, the finished transfer carries a receipt, and freeing deletes the local copy.

    cargo build -p toss-cli -p toss-engine && python3 scripts/e2e-studio.py
    BIG_MB=3000 python3 scripts/e2e-studio.py     # a bigger file
"""
import hashlib, json, os, shutil, signal, subprocess, sys, threading, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CLI = os.path.join(ROOT, "target/debug/toss-cli")
ENG = os.path.join(ROOT, "target/debug/toss-engine")
W = "/tmp/toss-e2e-studio"
BIG_MB = int(os.environ.get("BIG_MB", "1500"))
JOB = "10.11.26 12A1 Test"
SHOOTER = "Tuan Future"


def log(*a):
    print(time.strftime("%H:%M:%S"), *a, flush=True)


def blake(path):
    h = hashlib.blake2b()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def tree(base):
    out = {}
    for d, _, files in os.walk(base):
        for n in files:
            if n.startswith(".") or n.endswith(".part"):
                continue
            p = os.path.join(d, n)
            out[os.path.relpath(p, base)] = p
    return out


class Engine:
    def __init__(self, data, dest):
        os.makedirs(data, exist_ok=True)
        sp = os.path.join(data, "settings.json")
        if not os.path.exists(sp):
            json.dump({"device_name": "Mac Ngoc Anh", "dest_dir": dest, "receive_policy": "contacts"}, open(sp, "w"))
        self.p = subprocess.Popen([ENG, "--data-dir", data, "--no-localsend"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL, text=True, bufsize=1)
        self.n, self.replies, self.state = 0, {}, {}
        self.ready = threading.Event()
        threading.Thread(target=self._read, daemon=True).start()
        assert self.ready.wait(60), "engine never said ready"

    def _read(self):
        for line in self.p.stdout:
            m = json.loads(line)
            if "event" in m:
                if m["event"] == "ready":
                    self.ready.set()
                if m["event"] == "transfer":
                    self.state[m["data"]["id"]] = m["data"]
            else:
                self.replies[m["id"]] = m

    def call(self, cmd, timeout=60, **kw):
        self.n += 1
        i = self.n
        self.p.stdin.write(json.dumps({"id": i, "cmd": cmd, **kw}) + "\n")
        self.p.stdin.flush()
        t = time.time()
        while i not in self.replies:
            if time.time() - t > timeout:
                raise TimeoutError(cmd)
            time.sleep(0.05)
        r = self.replies.pop(i)
        if not r.get("ok"):
            raise RuntimeError(f"{cmd}: {r.get('error')}")
        return r.get("result")

    def wait(self, tid, until, timeout=600):
        t = time.time()
        while time.time() - t < timeout:
            for s in self.call("transfers"):
                if s["id"] == tid:
                    self.state[tid] = s
                    if s["status"] == "error":
                        raise RuntimeError(f"transfer failed: {s.get('error')}")
                    if until(s):
                        return s
            time.sleep(0.25)
        raise TimeoutError(f"transfer {tid}: {self.state.get(tid)}")

    def kill(self):
        self.p.send_signal(signal.SIGKILL)
        self.p.wait()

    def close(self):
        try:
            self.p.stdin.close()
            self.p.wait(10)
        except Exception:
            self.p.kill()


def main():
    shutil.rmtree(W, ignore_errors=True)
    jobs, backup, ssd = f"{W}/Jobs", f"{W}/Backup", f"{W}/ssd"
    src = f"{jobs}/2026/11/{JOB}/1_RAW/{SHOOTER}"
    os.makedirs(f"{src}/100NCZ_7", exist_ok=True)
    os.makedirs(ssd, exist_ok=True)
    log(f"studio data: {BIG_MB} MB + 24 small files")
    with open(f"{src}/100NCZ_7/DSC_0001.NEF", "wb") as f:
        block = os.urandom(1 << 20)
        for i in range(BIG_MB):
            f.write(block[: (1 << 20) - 1] + bytes([i % 256]))
    for i in range(24):
        with open(f"{src}/DSC_{i + 2:04d}.NEF", "wb") as f:
            f.write(os.urandom(2 << 20))

    host = subprocess.Popen([CLI, "--data-dir", f"{W}/studio", "--name", "Studio 2007", "host", "--root", f"Jobs={jobs}",
                             "--return", backup], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        time.sleep(6)

        def ticket(*args):
            return subprocess.run([CLI, "--data-dir", f"{W}/studio", "ticket", *args], check=True, capture_output=True,
                                  text=True).stdout.strip()

        get_t = ticket("--job", f"{JOB} - {SHOOTER}", "--root", "Jobs", "--path", f"2026/11/{JOB}/1_RAW/{SHOOTER}", "--get", "--days", "30")
        put_t = ticket("--job", "Backup · Ngoc Anh", "--root", "Jobs", "--put", "--ret", "Ngoc Anh", "--days", "0")

        ed = Engine(f"{W}/editor", f"{W}/editor-inbox")
        peek = ed.call("peek", text=get_t)
        assert peek["kind"] == "get" and peek["job"] == f"{JOB} - {SHOOTER}", peek
        assert ed.call("peek", text=put_t)["kind"] == "put"
        log("1. peek: shooter link and backup link told apart before acting")

        r = ed.call("open", text=get_t, dest=ssd)
        tid = r["id"]
        total = sum(os.path.getsize(p) for p in tree(src).values())
        s = ed.wait(tid, lambda s: s["bytes_done"] > total * 0.3)
        log(f"2. downloading into the picked folder, {s['bytes_done'] >> 20} MB of {total >> 20} MB: killing the app (SIGKILL)")
        ed.kill()
        at_kill = s["bytes_done"]
        t0 = time.time()
        ed = Engine(f"{W}/editor", f"{W}/editor-inbox")
        first = None
        while first is None and time.time() - t0 < 60:
            for x in ed.call("transfers"):
                if x["id"] == tid and x["status"] in ("running", "done"):
                    first = x
            time.sleep(0.1)
        assert first, "the download did not come back after restart"
        s = ed.wait(tid, lambda s: s["status"] == "done")
        log(f"   restarted: picked up at {first['bytes_done'] >> 20} MB (killed at {at_kill >> 20} MB), done in {time.time() - t0:.1f}s, path {s['path']}")
        got = tree(f"{ssd}/{JOB} - {SHOOTER}")
        want = tree(src)
        assert sorted(got) == sorted(want), (sorted(got)[:5], sorted(want)[:5])
        for rel in want:
            assert blake(got[rel]) == blake(want[rel]), rel
        log(f"   {len(want)} files byte-identical to the studio")

        b = ed.call("open", text=put_t)
        assert b["kind"] == "backup", b
        peers = ed.call("peers")
        key = f"b:{b['id']}"
        assert any(p["key"] == key for p in peers), peers
        log(f"3. backup link kept as a place to drop into: {b['name']}")

        work = f"{ssd}/{JOB} - {SHOOTER}"
        os.makedirs(f"{work}/Color", exist_ok=True)
        for i in range(6):
            with open(f"{work}/Color/DSC_{i:04d}.jpg", "wb") as f:
                f.write(os.urandom(3 << 20))
        sent = tree(work)
        btid = ed.call("send", key=key, paths=[work])
        s = ed.wait(btid, lambda s: s["status"] == "done" and "receipt" in s, timeout=900)
        rc = s["receipt"]
        assert rc["files"] == len(sent), rc
        landed = tree(f"{backup}/Ngoc Anh/{JOB} - {SHOOTER}")
        assert sorted(landed) == sorted(sent), (sorted(landed)[:5], sorted(sent)[:5])
        for rel in sent:
            assert blake(landed[rel]) == blake(sent[rel]), rel
        log(f"   backed up {len(sent)} files to Backup/Ngoc Anh, receipt: {rc['files']} files, {rc['bytes'] >> 20} MB")

        freed = ed.call("free", tid=btid)
        assert freed["files"] == len(sent) and not freed["kept"], freed
        assert not os.path.exists(work), "the freed job folder is gone"
        assert all(os.path.exists(p) for p in landed.values()), "the studio copy is untouched"
        log(f"   freed {freed['files']} files ({freed['bytes'] >> 20} MB) on the editor's machine; studio copy intact")
        ed.close()
        log("ALL OK")
    finally:
        host.terminate()
        host.wait()


if __name__ == "__main__":
    sys.exit(main())
