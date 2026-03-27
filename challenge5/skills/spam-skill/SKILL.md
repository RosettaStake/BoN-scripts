---
name: spam-skill
description: Challenge 5 Agent Arena — autonomous Red Light / Green Light agent. Monitors admin commands on-chain, classifies intent via Claude Haiku, sends transactions during green windows with adaptive rate control.
---

# Spam Skill — Challenge 5 Agent Arena

## When to Use

- User says "start the challenge 5 agent", "run the spam agent", "watch admin and spam"
- Challenge 5 Red Light / Green Light game
- Admin sends natural-language commands on-chain; agent must react autonomously

## Architecture

Two persistent processes, managed by OpenClaw:

1. **monitor.js** — polls observer for admin→target txs (500ms), classifies via Claude Haiku, writes control file
2. **admin-spam-daemon.js** — reads control file (50ms), sends txs with adaptive rate control (burst-then-pace)

OpenClaw's role: run `start-all` once. Both daemons are autonomous after that.

## Workflow

1. Run `start-all` — starts both monitor and daemon in background. Daemon starts in STOP state.
2. Monitor detects admin GREEN command → writes "start" to control file → daemon begins sending.
3. Monitor detects admin RED command → writes "stop" → daemon stops.
4. **No further action needed.** Both processes run autonomously until challenge ends.
5. Use `admin-spam-stop` for emergency manual stop.

## Health Check

```bash
# Check monitor
PID=$(cat .monitor.pid 2>/dev/null) && kill -0 $PID 2>/dev/null && echo "monitor: running" || echo "monitor: DOWN"

# Check daemon
PID=$(cat .admin-spam-daemon.pid 2>/dev/null) && kill -0 $PID 2>/dev/null && echo "daemon: running" || echo "daemon: DOWN"

# View logs
tail -20 .monitor.log
tail -20 .admin-spam-daemon.log
```

## Scripts

### start-all (primary entry point)

```bash
node scripts/start-all.js
```

Starts both monitor and daemon as background processes. Daemon starts in STOP state — monitor controls green/red. **Run this once.**

### admin-spam-stop

```bash
node scripts/admin-spam-stop.js
```

Emergency stop — writes "stop" to control file immediately.

### monitor-start (manual)

```bash
node scripts/monitor-start.js
```

Starts monitor only. Use `start-all` instead for normal operation.

### admin-spam-start (manual)

```bash
node scripts/admin-spam-start.js
```

Starts daemon only and writes "start". **Caution: bypasses monitor.** Use `start-all` instead.

### admin-spam-stop

```bash
node scripts/admin-spam-stop.js
```

Emergency stop — writes "stop" to control file immediately.

### fetch-admin-transactions

```bash
node scripts/fetch-admin-transactions.js
```

Debug tool — prints latest admin transactions as JSON lines.

## How It Works

### Monitor (monitor.js)
- Polls API every 500ms for admin→target txs
- On new tx: **immediately writes "stop"** (safety-first pause)
- Calls Claude Haiku to classify command as GREEN or RED (~200ms)
- GREEN → writes "start" to control file (daemon resumes)
- RED → keeps "stop" (daemon stays paused)

### Daemon (admin-spam-daemon.js)
- Phase 1 BURST: sends 500 in-flight txs immediately (first 2s of green)
- Phase 2 PACE: measures block confirmation rate, matches send rate to it
- Adaptive rate = near-zero mempool backlog = near-zero leak on RED
- Batch sends 50 txs via /transaction/send-multiple
- Local nonce tracking with periodic resync

### Reaction Timeline
```
t=0.0s  Admin command in block N
t=0.5s  Monitor detects tx, writes "stop"
t=0.7s  Claude Haiku classifies → "start" or stays "stop"
t=0.75s Daemon reads control file
t=1.2s  Daemon's txs in block N+2
```

## Prerequisites

- **Environment**: Copy `.env.example` to `.env` and fill in all values
- **Node.js**: v18+ with npm dependencies installed (`cd scripts && npm install`)
- **Anthropic API key**: Required for Claude Haiku classification

## Environment

| Variable | Required | Description |
|----------|----------|-------------|
| SPAM_ADMIN_ADDRESS | Yes | Admin wallet (erd1...) — source of commands |
| SPAM_TARGET_ADDRESS | Yes | Target wallet — receives spam transactions |
| MULTIVERSX_PRIVATE_KEY | Yes | Agent wallet PEM file path |
| ANTHROPIC_API_KEY | Yes | Anthropic API key for Claude Haiku classification |
| MULTIVERSX_API_URL | No | API base (default: api.battleofnodes.com) |
| MULTIVERSX_OBSERVER_URL | No | Direct observer node URL (fastest) |
| MULTIVERSX_CHAIN_ID | No | Chain ID (default: B) |
| MONITOR_POLL_MS | No | Monitor poll interval (default: 500) |
| SPAM_BATCH_SIZE | No | Txs per send call (default: 50) |
| SPAM_BURST_IN_FLIGHT | No | Burst phase max in-flight (default: 500) |
| SPAM_BURST_DURATION_MS | No | Burst phase duration (default: 2000) |
| SPAM_INITIAL_PACED_IN_FLIGHT | No | Initial pace cap before adaptive (default: 100) |
| SPAM_BLOCK_TIME_MS | No | Block time for rate calc (default: 600) |
