#!/usr/bin/env node
"use strict";
/**
 * Admin spam daemon — optimized for Challenge 5 Agent Arena.
 *
 * Sending strategy (TCP-inspired adaptive rate control):
 *   Phase 1 — BURST: send BURST_IN_FLIGHT txs immediately to fill first blocks
 *   Phase 2 — PACE:  measure block confirmation rate, match sending rate to it
 *                     keeps mempool near-empty → near-zero leak on RED
 *
 * Why matching block rate is optimal:
 *   Score = PermittedTxs - UnpermittedTxs
 *   Sending FASTER than blocks process → excess piles in mempool → leaks into RED
 *   Sending AT block rate → same confirmed count, zero leak → strictly better
 *
 * Adaptive rate measurement:
 *   Every nonce sync, compute: confirmed_per_sec = delta_nonce / delta_time
 *   Set maxInFlight = confirmed_per_sec * T_block (one block worth)
 *   This automatically adapts to competition level and network conditions.
 */

const fs = require("fs");
const path = require("path");
const axios = require("axios");
const { TransactionComputer, Transaction, Address } = require("@multiversx/sdk-core");
const { UserSigner } = require("@multiversx/sdk-wallet");

require("dotenv").config({ path: path.resolve(__dirname, "../.env") });

// ── Config ─────────────────────────────────────────────────────────────────

const CONTROL_FILE = process.env.SPAM_CONTROL_FILE || path.resolve(__dirname, "../.admin-spam-control");
const PID_FILE = process.env.SPAM_PID_FILE || path.resolve(__dirname, "../.admin-spam-daemon.pid");
const TARGET = process.env.SPAM_TARGET_ADDRESS;
const API_URL = (process.env.MULTIVERSX_API_URL || "https://api.battleofnodes.com").replace(/\/$/, "");
const OBSERVER_URL = (process.env.MULTIVERSX_OBSERVER_URL || process.env.MULTIVERSX_GATEWAY_URL || API_URL).replace(/\/$/, "");
// SEND_URL: gateway/proxy for sending txs. Observer nodes may not accept send-multiple.
const SEND_URL = (process.env.MULTIVERSX_GATEWAY_URL || process.env.MULTIVERSX_OBSERVER_URL || API_URL).replace(/\/$/, "");
const CHAIN_ID = process.env.MULTIVERSX_CHAIN_ID || "B";
const PEM_PATH = process.env.MULTIVERSX_PRIVATE_KEY;

const BATCH_SIZE = parseInt(process.env.SPAM_BATCH_SIZE || "50", 10);
const BURST_IN_FLIGHT = parseInt(process.env.SPAM_BURST_IN_FLIGHT || "500", 10);
const BURST_DURATION_MS = parseInt(process.env.SPAM_BURST_DURATION_MS || "2000", 10);
// Adaptive rate: initial paced cap before first measurement. Will be adjusted.
const INITIAL_PACED_IN_FLIGHT = parseInt(process.env.SPAM_INITIAL_PACED_IN_FLIGHT || "100", 10);
// Block time in ms (Supernova = 600ms)
const BLOCK_TIME_MS = parseInt(process.env.SPAM_BLOCK_TIME_MS || "600", 10);
// Min/max bounds for adaptive rate
const MIN_PACED_IN_FLIGHT = 10;
const MAX_PACED_IN_FLIGHT = 5000;

const CONTROL_POLL_MS = 50;
const RATE_MEASURE_MS = 600; // measure rate every ~1 block
const GAS_LIMIT = 50000;
const GAS_PRICE = 1000000000;
const TX_VERSION = 2;

// ── State ──────────────────────────────────────────────────────────────────

let spamming = false;
let localNonce = -1;
let nonceConfirmed = -1;
let greenStartTime = 0;
let txsSent = 0;
let lastNonceSync = 0;

// Adaptive rate state
let pacedInFlight = INITIAL_PACED_IN_FLIGHT;
let rateNonceStart = -1;
let rateTimeStart = 0;
let measuredRate = 0; // confirmed txs per second

// ── Helpers ────────────────────────────────────────────────────────────────

function readControl() {
  try { return fs.readFileSync(CONTROL_FILE, "utf8").trim().toLowerCase(); }
  catch { return "stop"; }
}

function sleep(ms) { return new Promise(r => setTimeout(r, ms)); }

function ts() { return new Date().toISOString().slice(11, 23); }

async function fetchAccountNonce(address) {
  const res = await axios.get(`${OBSERVER_URL}/address/${address}`, { timeout: 5000 });
  // Gateway: data.data.account.nonce | Observer: data.data.nonce
  return res.data?.data?.account?.nonce ?? res.data?.data?.nonce ?? 0;
}

async function fetchNetworkConfig() {
  const res = await axios.get(`${OBSERVER_URL}/network/config`, { timeout: 5000 });
  const cfg = res.data?.data?.config ?? {};
  return {
    chainId: cfg.erd_chain_id || CHAIN_ID,
    minGasPrice: parseInt(cfg.erd_min_gas_price || GAS_PRICE, 10),
    minTxVersion: parseInt(cfg.erd_min_transaction_version || TX_VERSION, 10),
  };
}

async function syncNonce(senderBech32) {
  try {
    const chainNonce = await fetchAccountNonce(senderBech32);
    const prevConfirmed = nonceConfirmed;
    if (localNonce < chainNonce) localNonce = chainNonce;
    nonceConfirmed = chainNonce;
    lastNonceSync = Date.now();

    // ── Adaptive rate measurement ──
    if (rateNonceStart >= 0 && rateTimeStart > 0) {
      const deltaNonce = chainNonce - rateNonceStart;
      const deltaTime = (Date.now() - rateTimeStart) / 1000; // seconds
      if (deltaTime > 0.3 && deltaNonce > 0) {
        measuredRate = deltaNonce / deltaTime;
        // Set paced in-flight to ~1 block worth of txs at measured rate
        // This keeps exactly 1 block of txs in mempool — minimal leak
        const target = Math.round(measuredRate * (BLOCK_TIME_MS / 1000));
        pacedInFlight = Math.max(MIN_PACED_IN_FLIGHT, Math.min(MAX_PACED_IN_FLIGHT, target));
      }
    }
    rateNonceStart = chainNonce;
    rateTimeStart = Date.now();

    return chainNonce;
  } catch (e) {
    console.error(`  Nonce sync failed: ${e.message}`);
    return localNonce;
  }
}

// ── Transaction Building & Sending ─────────────────────────────────────────

const txComputer = new TransactionComputer();

async function buildAndSignTx(nonce, signer, senderAddr, receiverAddr, config) {
  const tx = new Transaction({
    nonce: BigInt(nonce),
    value: 0n,
    receiver: receiverAddr,
    sender: senderAddr,
    gasPrice: BigInt(config.minGasPrice),
    gasLimit: BigInt(GAS_LIMIT),
    chainID: config.chainId,
    version: config.minTxVersion,
  });
  const bytes = txComputer.computeBytesForSigning(tx);
  tx.signature = await signer.sign(bytes);
  return txComputer.toPlainObject(tx, true);
}

async function sendBatch(txs) {
  if (txs.length === 0) return 0;
  try {
    const url = `${SEND_URL}/transaction/send-multiple`;
    if (txsSent === 0) {
      console.log(`  DEBUG send URL: ${url}`);
      console.log(`  DEBUG batch size: ${txs.length}`);
      console.log(`  DEBUG first tx: ${JSON.stringify(txs[0]).slice(0, 500)}`);
    }
    const res = await axios.post(url, txs, { timeout: 10000 });
    const hashes = res.data?.data?.txsHashes ?? {};
    const accepted = Object.keys(hashes).length;
    if (accepted === 0) {
      console.error(`  Send 0 accepted. URL: ${url}`);
      console.error(`  Response: ${JSON.stringify(res.data).slice(0, 500)}`);
      console.error(`  First tx: ${JSON.stringify(txs[0]).slice(0, 500)}`);
    }
    return accepted;
  } catch (e) {
    console.error(`  Batch send failed: ${e.message}`);
    if (e.response?.data) {
      console.error(`  Response: ${JSON.stringify(e.response.data).slice(0, 300)}`);
    }
    return 0;
  }
}

// ── Main Loop ──────────────────────────────────────────────────────────────

async function main() {
  if (!TARGET || !TARGET.startsWith("erd1")) {
    console.error("SPAM_TARGET_ADDRESS (erd1...) is required"); process.exit(1);
  }
  if (!PEM_PATH) {
    console.error("MULTIVERSX_PRIVATE_KEY is required"); process.exit(1);
  }

  const pemPath = path.isAbsolute(PEM_PATH) ? PEM_PATH : path.resolve(process.cwd(), PEM_PATH);
  if (!fs.existsSync(pemPath)) {
    console.error(`PEM not found: ${pemPath}`); process.exit(1);
  }

  const signer = UserSigner.fromPem(fs.readFileSync(pemPath, "utf8"));
  const senderAddress = signer.getAddress().bech32();
  // Pre-cache Address objects for signing (avoid per-tx allocation)
  const senderAddr = Address.newFromBech32(senderAddress);
  const targetAddr = Address.newFromBech32(TARGET);

  console.log(`Daemon started. Sender: ${senderAddress}`);
  console.log(`Target: ${TARGET}`);
  console.log(`Batch=${BATCH_SIZE} Burst=${BURST_IN_FLIGHT} InitialPace=${INITIAL_PACED_IN_FLIGHT} BlockTime=${BLOCK_TIME_MS}ms`);

  // PID file
  fs.writeFileSync(PID_FILE, String(process.pid), "utf8");
  process.on("exit", () => { try { fs.unlinkSync(PID_FILE); } catch {} });
  process.on("SIGINT", () => process.exit(0));
  process.on("SIGTERM", () => process.exit(0));

  // Network config
  let config;
  try {
    config = await fetchNetworkConfig();
    console.log(`Chain=${config.chainId} gasPrice=${config.minGasPrice}`);
  } catch (e) {
    console.error(`Network config failed: ${e.message}, using defaults`);
    config = { chainId: CHAIN_ID, minGasPrice: GAS_PRICE, minTxVersion: TX_VERSION };
  }

  // Initial state
  let lastControl = readControl();
  if (lastControl === "start") {
    spamming = true;
    greenStartTime = Date.now();
    await syncNonce(senderAddress);
    console.log(`Initial: start, nonce=${localNonce}`);
  } else {
    console.log("Initial: stop. Waiting...");
  }

  // ── Event loop ──
  while (true) {
    const control = readControl();
    if (control !== lastControl) {
      lastControl = control;
      if (control === "start") {
        spamming = true;
        greenStartTime = Date.now();
        pacedInFlight = INITIAL_PACED_IN_FLIGHT; // reset adaptive rate
        rateNonceStart = -1;
        measuredRate = 0;
        await syncNonce(senderAddress);
        console.log(`[${ts()}] GREEN nonce=${localNonce} sending...`);
      } else {
        if (spamming) {
          const inFlight = localNonce - nonceConfirmed;
          console.log(`[${ts()}] RED total=${txsSent} inFlight=${inFlight} rate=${measuredRate.toFixed(0)}/s pace=${pacedInFlight}`);
        }
        spamming = false;
      }
    }

    if (!spamming) { await sleep(CONTROL_POLL_MS); continue; }

    // ── Burst vs Adaptive Pace ──
    const elapsed = Date.now() - greenStartTime;
    const isBurst = elapsed < BURST_DURATION_MS;
    const maxInFlight = isBurst ? BURST_IN_FLIGHT : pacedInFlight;
    const inFlight = localNonce - nonceConfirmed;

    if (inFlight >= maxInFlight) {
      // At capacity — sync nonce to see how many confirmed
      if (Date.now() - lastNonceSync > RATE_MEASURE_MS) {
        await syncNonce(senderAddress);
      } else {
        await sleep(20);
      }
      continue;
    }

    // ── Build & send batch ──
    const canSend = Math.min(BATCH_SIZE, maxInFlight - inFlight);
    const startNonce = localNonce;
    const txs = [];
    for (let i = 0; i < canSend; i++) {
      // Mid-batch control check for fast stop
      if (i > 0 && i % 10 === 0) {
        if (readControl() !== "start") {
          spamming = false; lastControl = "stop";
          console.log(`[${ts()}] RED (mid-batch) inFlight=${startNonce - nonceConfirmed}`);
          break;
        }
      }
      txs.push(buildAndSignTx(startNonce + i, signer, senderAddr, targetAddr, config));
    }

    // Debug: log first tx payload once
    if (txs.length > 0 && txsSent === 0) {
      console.log(`  DEBUG first tx: ${JSON.stringify(txs[0])}`);
    }

    if (txs.length > 0) {
      const resolvedTxs = await Promise.all(txs);
      const accepted = await sendBatch(resolvedTxs);
      localNonce = startNonce + accepted; // only advance by what was accepted
      txsSent += accepted;
      if (accepted < txs.length) {
        // Some rejected — resync to get true chain state
        await syncNonce(senderAddress);
      }
      if (txsSent % 500 < BATCH_SIZE) {
        console.log(
          `  [${ts()}] sent=${txsSent} inFlight=${localNonce - nonceConfirmed} ` +
          `rate=${measuredRate.toFixed(0)}/s pace=${pacedInFlight} ${isBurst ? "BURST" : "PACE"}`
        );
      }
    }

    // Periodic nonce sync for rate measurement
    if (Date.now() - lastNonceSync > RATE_MEASURE_MS) {
      await syncNonce(senderAddress);
    }
  }
}

main().catch(e => { console.error(e); process.exit(1); });
