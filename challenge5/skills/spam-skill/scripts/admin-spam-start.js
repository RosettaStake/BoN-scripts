#!/usr/bin/env node
"use strict";
/**
 * Signal the admin-spam daemon to start spamming.
 * Auto-starts the daemon in background if not already running.
 * Cross-platform (macOS + Linux).
 */

const fs = require("fs");
const path = require("path");
const { spawn } = require("child_process");

require("dotenv").config({ path: path.resolve(__dirname, "../.env") });

const CONTROL_FILE = process.env.SPAM_CONTROL_FILE || path.resolve(__dirname, "../.admin-spam-control");
const PID_FILE = process.env.SPAM_PID_FILE || path.resolve(__dirname, "../.admin-spam-daemon.pid");
const DAEMON_SCRIPT = path.resolve(__dirname, "admin-spam-daemon.js");

function isDaemonRunning() {
  try {
    const pid = parseInt(fs.readFileSync(PID_FILE, "utf8"), 10);
    if (!Number.isInteger(pid) || pid <= 0) return false;
    process.kill(pid, 0); // throws if process doesn't exist
    return true;
  } catch {
    return false;
  }
}

function startDaemon() {
  const logFile = path.resolve(__dirname, "../.admin-spam-daemon.log");
  const out = fs.openSync(logFile, "a");
  const err = fs.openSync(logFile, "a");
  const child = spawn(process.execPath, [DAEMON_SCRIPT], {
    detached: true,
    stdio: ["ignore", out, err],
    env: { ...process.env },
  });
  child.unref();
  console.log(`Daemon spawned (PID ${child.pid}). Log: ${logFile}`);
}

function main() {
  if (!isDaemonRunning()) {
    startDaemon();
    // Give daemon time to start and write PID
    setTimeout(() => {
      fs.writeFileSync(CONTROL_FILE, "start", "utf8");
      console.log("Started. Daemon will begin spamming.");
    }, 500);
  } else {
    fs.writeFileSync(CONTROL_FILE, "start", "utf8");
    console.log("Started. Daemon already running.");
  }
}

main();
