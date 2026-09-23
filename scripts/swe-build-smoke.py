#!/usr/bin/env python3
"""Smoke-test an INSTALLED logos binary (called by scripts/swe-build.sh).

Checks, in order: --version matches; index/status/search/check over a fixture
project; `serve --ui` answers / with a self-only CSP AND the bundle the shell
references at full byte length (the check that catches the blank-page build);
CSS, favicon, /api/v1/overview JSON; a foreign Host is refused 403; and a stdio
MCP probe (initialize -> tools/list -> a real tools/call).

Uses http.client, not curl: the shell hook rewrites curl and strips headers.
Exit 0 = every check passed, 1 = at least one failed (each printed).
"""
import argparse
import http.client
import json
import os
import re
import select
import subprocess
import sys
import time

FAILS = []


def check(name, ok, detail=""):
    print(("PASS " if ok else "FAIL ") + name + (f" — {detail}" if detail else ""))
    if not ok:
        FAILS.append(name)


def run(bin_, *args, project):
    return subprocess.run([bin_, *args, "--project", project], capture_output=True, text=True, timeout=300)


def get(port, path, host=None, accept=None):
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
    headers = {}
    if host:
        headers["Host"] = host
    if accept:
        headers["Accept"] = accept
    c.request("GET", path, headers=headers)
    r = c.getresponse()
    body = r.read()
    c.close()
    return r.status, {k.lower(): v for k, v in r.getheaders()}, body


def smoke_ui(bin_, project, port):
    proc = subprocess.Popen([bin_, "serve", "--ui", "--port", str(port), "--project", project],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        status = None
        for _ in range(90):
            try:
                status, headers, body = get(port, "/")
                break
            except OSError:
                time.sleep(1)
        check("ui: GET / 200", status == 200, f"status={status}")
        if status != 200:
            return
        csp = headers.get("content-security-policy", "")
        check("ui: self-only CSP", "'self'" in csp, csp[:80])
        html = body.decode("utf-8", "replace")
        js = re.findall(r'src="(/assets/[^"]+\.js)"', html)
        css = re.findall(r'href="(/assets/[^"]+\.css)"', html)
        check("ui: shell references a JS bundle", bool(js))
        for path in js[:1]:
            s, h, b = get(port, path)
            declared = int(h.get("content-length", len(b)))
            check("ui: JS bundle 200 at full length", s == 200 and len(b) > 0 and len(b) == declared,
                  f"{path} status={s} bytes={len(b)} declared={declared}")
        for path in css[:1]:
            s, _, _ = get(port, path)
            check("ui: CSS 200", s == 200, path)
        s, _, b = get(port, "/api/v1/overview")
        try:
            json.loads(b)
            parsed = True
        except ValueError:
            parsed = False
        check("ui: /api/v1/overview JSON", s == 200 and parsed, f"status={s}")
        s, _, _ = get(port, "/favicon.svg")
        check("ui: /favicon.svg 200", s == 200, f"status={s}")
        s, _, _ = get(port, "/", host="evil")
        check("ui: foreign Host refused 403", s == 403, f"status={s}")
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()


def smoke_mcp(bin_, project):
    proc = subprocess.Popen([bin_, "serve", "--mcp", "--project", project],
                            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    fd = proc.stdout.fileno()
    buf = b""

    def send(msg):
        proc.stdin.write((json.dumps(msg) + "\n").encode())
        proc.stdin.flush()

    def recv(want_id, timeout=60):
        nonlocal buf
        deadline = time.time() + timeout
        while time.time() < deadline:
            while b"\n" in buf:
                line, buf = buf.split(b"\n", 1)
                if not line.strip():
                    continue
                try:
                    msg = json.loads(line)
                except ValueError:
                    continue
                if msg.get("id") == want_id:
                    return msg
            ready, _, _ = select.select([fd], [], [], 1)
            if ready:
                chunk = os.read(fd, 65536)  # os.read, not buffered read(N): it blocks
                if not chunk:
                    return None
                buf += chunk
        return None

    try:
        send({"jsonrpc": "2.0", "id": 1, "method": "initialize",
              "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                         "clientInfo": {"name": "swe-build-smoke", "version": "1"}}})
        r = recv(1)
        check("mcp: initialize", bool(r and "result" in r))
        send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        send({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
        r = recv(2)
        tools = [t.get("name") for t in (r or {}).get("result", {}).get("tools", [])]
        check("mcp: tools/list", "status" in tools, f"{len(tools)} tools")
        send({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "status", "arguments": {}}})
        r = recv(3)
        ok = bool(r and "result" in r and not r["result"].get("isError"))
        check("mcp: tools/call status", ok)
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--bin", required=True)
    ap.add_argument("--expect-version", required=True)
    ap.add_argument("--project", required=True)
    ap.add_argument("--port", type=int, default=4999)
    a = ap.parse_args()

    v = subprocess.run([a.bin, "--version"], capture_output=True, text=True)
    check("--version matches", v.returncode == 0 and a.expect_version in v.stdout, v.stdout.strip())
    r = run(a.bin, "index", "--quiet", project=a.project)
    check("index", r.returncode == 0, r.stderr.strip()[:200])
    r = run(a.bin, "status", "--json", project=a.project)
    check("status", r.returncode == 0, r.stderr.strip()[:200])
    r = run(a.bin, "search", "alpha", "--json", project=a.project)
    check("search finds the fixture symbol", r.returncode == 0 and "alpha" in r.stdout, r.stderr.strip()[:200])
    r = run(a.bin, "check", "--allow-no-rules", project=a.project)
    check("check", r.returncode == 0, f"exit={r.returncode}")
    smoke_ui(a.bin, a.project, a.port)
    smoke_mcp(a.bin, a.project)

    print(f"smoke: {len(FAILS)} failed" + (f" ({', '.join(FAILS)})" if FAILS else ""))
    return 1 if FAILS else 0


if __name__ == "__main__":
    sys.exit(main())
