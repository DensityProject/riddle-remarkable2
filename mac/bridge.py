#!/usr/bin/env python3
"""Diary bridge: lets the reMarkable diary (riddle) use Claude on Nate's
subscription, over USB or home Wi-Fi.

Speaks the slice of OpenAI's streaming /v1/chat/completions that riddle uses,
and answers each page by running Claude Code headless (`claude -p`, the engine
under the Agent SDK) with every tool, MCP server, plugin, setting source and
slash command disabled. Claude only ever sees the diary's prompt and page.

Lockdown:
- answers only the tablet: its USB address and its Wi-Fi address
  (BRIDGE_ALLOW, plus the `allow` file that tablet_link.py rewrites each
  time the tablet is plugged in); every other client gets 403 before
  anything is read.
  Plain HTTP + token: meant for trusted home Wi-Fi only
- bearer token required (~/diary-bridge/token, 0600); constant-time compare
- one request at a time, 8 MB body cap, 120 s Claude timeout
- runs Claude in an empty working directory, no session persistence
"""
import base64, hmac, http.server, json, os, re, subprocess, sys, threading, time

HOST = os.environ.get("BRIDGE_HOST", "0.0.0.0")
ALLOW = {a.strip() for a in os.environ.get("BRIDGE_ALLOW", "10.11.99.1").split(",") if a.strip()}
PORT = int(os.environ.get("BRIDGE_PORT", "8787"))
HERE = os.path.dirname(os.path.abspath(__file__))
TOKEN = open(os.path.join(HERE, "token")).read().strip()
ALLOW_FILE = os.path.join(HERE, "allow")
WORKDIR = os.path.join(HERE, "empty")
CLAUDE = os.environ.get("CLAUDE_BIN", os.path.expanduser("~/.local/bin/claude"))
MAX_BODY = 8_000_000
# Short names map to the newest model of each family (the CLI's own aliases
# lag behind: "sonnet" resolved to Sonnet 5, "opus" to Opus 5 on 2026-10-03).
MODELS = {
    "haiku": "claude-haiku-4-5-20251001",
    "sonnet": "claude-sonnet-5-5",
    "opus": "claude-opus-5-5",
}
DEFAULT_MODEL = "sonnet"
os.makedirs(WORKDIR, exist_ok=True)
busy = threading.Lock()
_allow_cache = [None, set()]  # (mtime, addresses) of ALLOW_FILE


def allowed(addr):
    """BRIDGE_ALLOW plus one address per line in ALLOW_FILE (re-read on change)."""
    if addr in ALLOW:
        return True
    try:
        mtime = os.stat(ALLOW_FILE).st_mtime
    except OSError:
        return False
    if mtime != _allow_cache[0]:
        with open(ALLOW_FILE) as f:
            _allow_cache[:] = [mtime, {l.strip() for l in f if l.strip()}]
    return addr in _allow_cache[1]


def log(msg):
    print(time.strftime("%Y-%m-%d %H:%M:%S ") + msg, flush=True)


def content_text(content):
    """OpenAI content is a string or a list of parts; keep the text."""
    if isinstance(content, str):
        return content
    return "\n".join(p.get("text", "") for p in content if p.get("type") == "text")


def build_turn(req):
    """Map riddle's chat request to (system prompt, Claude user content blocks)."""
    system, history, blocks = "", [], []
    msgs = req.get("messages", [])
    for i, m in enumerate(msgs):
        role = m.get("role")
        if role == "system":
            system += content_text(m.get("content", ""))
        elif i == len(msgs) - 1 and role == "user":
            content = m.get("content", "")
            parts = content if isinstance(content, list) else [{"type": "text", "text": content}]
            for p in parts:
                if p.get("type") == "image_url":
                    url = p.get("image_url", {}).get("url", "")
                    mm = re.match(r"data:(image/(?:png|jpeg));base64,([A-Za-z0-9+/=]+)$", url)
                    if mm:
                        base64.b64decode(mm.group(2), validate=True)
                        blocks.append({"type": "image", "source": {
                            "type": "base64", "media_type": mm.group(1), "data": mm.group(2)}})
                elif p.get("type") == "text" and p.get("text"):
                    blocks.append({"type": "text", "text": p["text"]})
        else:
            who = "Diary writer" if role == "user" else "You (the diary)"
            history.append(f"{who}: {content_text(m.get('content', ''))}")
    if history:
        blocks.insert(0, {"type": "text", "text":
                          "Earlier in this diary:\n" + "\n".join(history) + "\n\nNow, today's page:"})
    return system, blocks


def stream_claude(system, blocks, model, emit):
    """Run Claude headless and call emit(text) for each streamed text delta."""
    cmd = [CLAUDE, "-p", "--input-format", "stream-json", "--output-format", "stream-json",
           "--verbose", "--include-partial-messages", "--model", model,
           "--tools", "", "--setting-sources", "", "--strict-mcp-config",
           "--mcp-config", '{"mcpServers":{}}', "--disable-slash-commands",
           "--no-session-persistence", "--system-prompt", system or "You are a diary."]
    msg = {"type": "user", "message": {"role": "user", "content": blocks}}
    env = {k: v for k, v in os.environ.items() if not k.startswith(("ANTHROPIC_", "CLAUDE_CODE_"))}
    p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.PIPE, text=True, cwd=WORKDIR, env=env)
    timer = threading.Timer(120, p.kill)
    timer.start()
    streamed, final = False, ""
    try:
        p.stdin.write(json.dumps(msg) + "\n")
        p.stdin.close()
        for line in p.stdout:
            try:
                ev = json.loads(line)
            except ValueError:
                continue
            if ev.get("type") == "stream_event":
                d = ev.get("event", {}).get("delta", {})
                if d.get("type") == "text_delta" and d.get("text"):
                    streamed = True
                    emit(d["text"])
            elif ev.get("type") == "result":
                final = ev.get("result") or ""
                if ev.get("is_error"):
                    log("claude error: " + final[:200])
        p.wait()
    finally:
        timer.cancel()
    if not streamed and final:
        emit(final)
    if not streamed and not final:
        err = p.stderr.read()[:300] if p.stderr else ""
        raise RuntimeError("claude produced no reply " + err.strip())


class Handler(http.server.BaseHTTPRequestHandler):
    server_version = "diary-bridge"
    sys_version = ""

    def log_message(self, fmt, *args):
        log("http " + self.client_address[0] + " " + fmt % args)

    def deny(self, code):
        self.send_response(code)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_GET(self):
        self.deny(404)

    def do_POST(self):
        if not allowed(self.client_address[0]):
            return self.deny(403)
        auth = self.headers.get("Authorization", "")
        if not hmac.compare_digest(auth.encode(), ("Bearer " + TOKEN).encode()):
            return self.deny(401)
        if not self.path.rstrip("/").endswith("/chat/completions"):
            return self.deny(404)
        n = int(self.headers.get("Content-Length") or 0)
        if n <= 0 or n > MAX_BODY:
            return self.deny(413)
        try:
            req = json.loads(self.rfile.read(n))
            system, blocks = build_turn(req)
        except Exception as e:
            log(f"bad request: {e}")
            return self.deny(400)
        if not busy.acquire(blocking=False):
            return self.deny(429)
        try:
            asked = req.get("model")
            model = MODELS.get(asked) or (asked if asked in MODELS.values() else MODELS[DEFAULT_MODEL])
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.end_headers()

            def emit(text):
                chunk = {"choices": [{"index": 0, "delta": {"content": text}}]}
                self.wfile.write(("data: " + json.dumps(chunk, separators=(",", ":")) + "\n\n").encode())
                self.wfile.flush()

            t0 = time.time()
            try:
                stream_claude(system, blocks, model, emit)
                log(f"page answered by {model} in {time.time() - t0:.1f}s")
            except Exception as e:
                log(f"claude failed: {e}")
                emit("The ink would not answer. Try again in a moment.")
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            log("diary disconnected mid-reply")
        finally:
            busy.release()


if __name__ == "__main__":
    srv = http.server.ThreadingHTTPServer((HOST, PORT), Handler)
    log(f"listening on {HOST}:{PORT}, allowing {sorted(ALLOW)}")
    srv.serve_forever()
