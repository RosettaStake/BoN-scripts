#!/usr/bin/env node
"use strict";
/**
 * Start the monitor as a detached background process.
 * Checks if already running via PID file.
 */

const fs = require("fs");
const path = require("path");
const { spawn } = require("child_process");

require("dotenv").config({ path: path.resolve(__dirname, "../.env") });

const PID_FILE = path.resolve(__dirname, "../.monitor.pid");
const MONITOR_SCRIPT = path.resolve(__dirname, "monitor.js");

function isRunning() {
  try {
    const pid = parseInt(fs.readFileSync(PID_FILE, "utf8"), 10);
    if (!Number.isInteger(pid) || pid <= 0) return false;
    process.kill(pid, 0);
    return true;
  } catch { return false; }
}

function main() {
  if (isRunning()) {
    console.log("Monitor already running.");
    return;
  }

  const configs = [
    "SPAM_ADMIN_ADDRESS",
    "SPAM_TARGET_ADDRESS",
    "MULTIVERSX_PRIVATE_KEY",
    "MULTIVERSX_API_URL",
    "MULTIVERSX_CHAIN_ID",
    "MONITOR_POLL_MS",
    "SPAM_BATCH_SIZE",
  ];
  console.log("--- Monitor Configuration ---");
  configs.forEach(key => {
    console.log(`${key}: ${process.env[key] || "not set"}`);
  });
  console.log("----------------------------");

  const logFile = path.resolve(__dirname, "../.monitor.log");
  const out = fs.openSync(logFile, "a");
  const err = fs.openSync(logFile, "a");
  const child = spawn(process.execPath, [MONITOR_SCRIPT], {
    detached: true,
    stdio: ["ignore", out, err],
    env: { ...process.env },
  });
  child.unref();
  console.log(`Monitor spawned (PID ${child.pid}). Log: ${logFile}`);
}

main();
