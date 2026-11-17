#!/usr/bin/env python3
"""Write a fake $HOME full of made-up Claude Code, Codex and pi sessions.

Nothing here comes from a real machine: every project, prompt and reply is
invented so the README recording never shows anyone's actual sessions.

usage: seed.py <fake-home>
"""
import json
import os
import shutil
import sys
import time
from pathlib import Path

HOME = Path(sys.argv[1]).resolve()
SRC = "/home/demo/src"
NOW = time.time()

# (agent, id, project, title, minutes ago, [(role, text), ...])
SESSIONS = [
    ("claude", "1f6c2a9e-0b1d-4c55-9a3e-7d1e2b9c4a01", "webshop",
     "Add rate limiting to checkout API", 2, [
         ("user", "Checkout is getting hammered by bots. Add per-IP rate limiting to POST /checkout."),
         ("assistant", "I'll add a token-bucket limiter backed by the existing Redis cache, 10 requests per minute per IP."),
         ("user", "Sounds good, keep the limit configurable."),
         ("assistant", "Added RATE_LIMIT_PER_MIN to config and wired the middleware. Before I run the load test against staging, can I hit https://staging.example.test?"),
     ]),
    ("claude", "2a7d3b8f-1c2e-4d66-8b4f-8e2f3c0d5b12", "infra",
     "Migrate Terraform state to remote backend", 5, [
         ("user", "Move our Terraform state from local files to the S3 backend with DynamoDB locking."),
         ("assistant", "Writing backend.tf and running terraform init -migrate-state for each workspace..."),
     ]),
    ("codex", "019a4e1c-7f3b-7c21-9d4e-5a6b7c8d9e01", "payments",
     "Refactor Stripe webhook handler", 9, [
         ("user", "The webhook handler is one 400-line function. Split it by event type."),
         ("assistant", "Split into handle_invoice_paid, handle_subscription_updated and handle_refund. Tests pass."),
     ]),
    ("claude", "3b8e4c9a-2d3f-4e77-9c5a-9f3a4d1e6c23", "blog",
     "Draft post on Rust error handling", 34, [
         ("user", "Help me outline a post comparing anyhow and thiserror."),
         ("assistant", "Outline: 1) libraries vs binaries, 2) thiserror for typed errors, 3) anyhow for context, 4) when to mix them."),
         ("user", "Great, draft section 2."),
         ("assistant", "Drafted section 2 with an example of a ParseError enum and #[from] conversions."),
     ]),
    ("pi", "b7e1c3d5-9f2a-4b6c-8d0e-1f2a3b4c5d6e", "cli-tool",
     "add a --json output flag", 58, [
         ("user", "add a --json output flag"),
         ("assistant", "Added --json; each row is now serialized with serde_json when the flag is set."),
     ]),
    ("claude", "4c9f5d0b-3e4a-4f88-8d6b-0a4b5e2f7d34", "webshop",
     "Fix flaky cart integration test", 60 * 3, [
         ("user", "test_cart_totals fails about one run in ten. Find out why."),
         ("assistant", "The test shares a Redis cache with the pricing test and both run in parallel. Giving each test its own key prefix fixes it."),
     ]),
    ("codex", "019a3d0b-6e2a-7b10-8c3d-4e5f6a7b8c90", "infra",
     "Rotate expired TLS certificates", 60 * 7, [
         ("user", "The ingress cert expired. Rotate it and add an alert 14 days before expiry."),
         ("assistant", "Renewed via cert-manager and added a Prometheus alert on certmanager_certificate_expiration_timestamp_seconds."),
     ]),
    ("claude", "5d0a6e1c-4f5b-4a99-9e7c-1b5c6f3a8e45", "dotfiles",
     "Set up Neovim LSP for Go", 60 * 26, [
         ("user", "Configure gopls in my Neovim config with format on save."),
         ("assistant", "Added gopls via nvim-lspconfig and a BufWritePre autocmd that runs vim.lsp.buf.format."),
     ]),
    ("pi", "c8f2d4e6-0a3b-4c7d-9e1f-2a3b4c5d6e7f", "webshop",
     "why is the docker build so slow", 60 * 30, [
         ("user", "why is the docker build so slow"),
         ("assistant", "COPY . . runs before npm ci, so every source change busts the dependency layer. Copy package*.json first."),
     ]),
    ("claude", "6e1b7f2d-5a6c-4b00-8f8d-2c6d7a4b9f56", "api-gateway",
     "Add OpenTelemetry tracing", 60 * 50, [
         ("user", "Instrument the gateway with OpenTelemetry and export to the collector."),
         ("assistant", "Added the OTLP exporter and spans around upstream calls, including the Redis cache lookups."),
     ]),
    ("codex", "019a2c9a-5d19-7a0f-7b2c-3d4e5f6a7b8c", "mobile-app",
     "Upgrade React Native to 0.76", 60 * 75, [
         ("user", "Upgrade React Native and fix whatever breaks."),
         ("assistant", "Upgraded to 0.76, enabled the new architecture and replaced two unmaintained native modules."),
     ]),
    ("claude", "7f2c8a3e-6b7d-4c11-9a9e-3d7e8b5c0a67", "analytics",
     "Speed up the daily report query", 60 * 100, [
         ("user", "The daily revenue report takes 40 seconds. Make it faster."),
         ("assistant", "Added a composite index on (created_at, status) and the query now runs in 300 ms."),
     ]),
]

# What `claude agents --json` reports for the shimmed `claude` binary.
LIVE = [
    {"sessionId": SESSIONS[0][1], "kind": "background", "id": "a1b2", "state": "blocked"},
    {"sessionId": SESSIONS[1][1], "kind": "interactive", "state": "running"},
    {"sessionId": SESSIONS[3][1], "kind": "interactive", "state": "idle"},
]


def write_jsonl(path, rows, minutes_ago):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")
    t = NOW - minutes_ago * 60
    os.utime(path, (t, t))


def claude(sid, cwd, title, msgs, ago):
    rows = []
    for role, text in msgs:
        rows.append({"type": role, "cwd": cwd, "sessionId": sid,
                     "message": {"role": role, "content": [{"type": "text", "text": text}]}})
    rows.insert(1, {"type": "ai-title", "aiTitle": title})
    enc = cwd.replace("/", "-")
    write_jsonl(HOME / ".claude/projects" / enc / f"{sid}.jsonl", rows, ago)


def codex(sid, cwd, title, msgs, ago):
    rows = [{"type": "session_meta", "payload": {"id": sid, "cwd": cwd, "thread_source": "user"}}]
    for role, text in msgs:
        kind = "input_text" if role == "user" else "output_text"
        rows.append({"type": "response_item", "payload": {
            "type": "message", "role": role, "content": [{"type": kind, "text": text}]}})
    write_jsonl(HOME / ".codex/sessions/2026/11/16" / f"rollout-{sid}.jsonl", rows, ago)
    with (HOME / ".codex/session_index.jsonl").open("a") as f:
        f.write(json.dumps({"id": sid, "thread_name": title}) + "\n")


def pi(sid, cwd, title, msgs, ago):
    rows = [{"type": "session", "id": sid, "cwd": cwd}]
    for role, text in msgs:
        rows.append({"type": "message", "message": {
            "role": role, "content": [{"type": "text", "text": text}]}})
    enc = cwd.replace("/", "-")
    write_jsonl(HOME / ".pi/agent/sessions" / enc / f"{sid}.jsonl", rows, ago)


def main():
    if HOME.exists():
        shutil.rmtree(HOME)
    (HOME / ".codex").mkdir(parents=True)
    for agent, sid, project, title, ago, msgs in SESSIONS:
        {"claude": claude, "codex": codex, "pi": pi}[agent](
            sid, f"{SRC}/{project}", title, msgs, ago)
    (HOME / "agents.json").write_text(json.dumps(LIVE))


main()
