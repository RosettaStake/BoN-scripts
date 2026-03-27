#!/usr/bin/env node
"use strict";
/**
 * Single entry point — starts both monitor and daemon as background daemons.
 * OpenClaw runs this once. Both processes are autonomous after that.
 *
 * Daemon starts in STOP state. Monitor controls green/red via control file.
 */

const fs = require("fs");
const path = require("path");
const { spawn } = require("child_process");

require("dotenv").config({ path: path.resolve(__dirname, "../.env") });

const CONTROL_FILE = process.env.SPAM_CONTROL_FILE || path.resolve(__dirname, "../.admin-spam-control");

function isRunning(pidFile) {
  try {
    const pid = parseInt(fs.readFileSync(pidFile, "utf8"), 10);
    if (!Number.isInteger(pid) || pid <= 0) return false;
    process.kill(pid, 0);
    return true;
  } catch { return false; }
}

function spawnDaemon(script, logName) {
  const logFile = path.resolve(__dirname, `../${logName}`);
  const out = fs.openSync(logFile, "a");
  const err = fs.openSync(logFile, "a");
  const child = spawn(process.execPath, [path.resolve(__dirname, script)], {
    detached: true,
    stdio: ["ignore", out, err],
    env: { ...process.env },
  });
  child.unref();
  console.log(`  ${script} spawned (PID ${child.pid}). Log: ${logFile}`);
}

function main() {
  // Daemon starts in STOP — only monitor writes "start" on green
  fs.writeFileSync(CONTROL_FILE, "stop", "utf8");

  const monitorPid = path.resolve(__dirname, "../.monitor.pid");
  const daemonPid = path.resolve(__dirname, "../.admin-spam-daemon.pid");

  if (!isRunning(monitorPid)) {
    spawnDaemon("monitor.js", ".monitor.log");
  } else {
    console.log("  monitor.js already running.");
  }

  if (!isRunning(daemonPid)) {
    spawnDaemon("admin-spam-daemon.js", ".admin-spam-daemon.log");
  } else {
    console.log("  admin-spam-daemon.js already running.");
  }

  console.log("\nAgent is autonomous. Waiting for admin GREEN command.");
}

main();
