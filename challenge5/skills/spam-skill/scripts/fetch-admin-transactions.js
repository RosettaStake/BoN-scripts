#!/usr/bin/env node
"use strict";
/**
 * Fetch admin transactions from MultiversX API.
 * Filters: sender=ADMIN, receiver=TARGET, status=success.
 * Outputs JSON lines to stdout for OpenClaw to interpret.
 */

const path = require("path");
const axios = require("axios");

require("dotenv").config({ path: path.resolve(__dirname, "../.env") });

const API_URL = (process.env.MULTIVERSX_API_URL || "https://api.battleofnodes.com").replace(/\/$/, "");
const ADMIN_ADDRESS = process.env.SPAM_ADMIN_ADDRESS;
const TARGET_ADDRESS = process.env.SPAM_TARGET_ADDRESS;

function decodeData(base64) {
  if (!base64) return "";
  try { return Buffer.from(base64, "base64").toString("utf-8"); }
  catch { return ""; }
}

async function main() {
  if (!ADMIN_ADDRESS || !ADMIN_ADDRESS.startsWith("erd1")) {
    console.error("SPAM_ADMIN_ADDRESS (erd1...) is required");
    process.exit(1);
  }

  // Filter by sender=ADMIN and optionally receiver=TARGET
  let url = `${API_URL}/accounts/${ADMIN_ADDRESS}/transactions?sender=${ADMIN_ADDRESS}&size=10&order=desc&status=success`;
  if (TARGET_ADDRESS && TARGET_ADDRESS.startsWith("erd1")) {
    url += `&receiver=${TARGET_ADDRESS}`;
  }

  try {
    const res = await axios.get(url, { timeout: 10000 });
    const body = res.data ?? {};
    const txs = Array.isArray(body) ? body : (body.data ?? body.transactions ?? []);

    if (!Array.isArray(txs) || txs.length === 0) {
      console.log(JSON.stringify({ status: "no_transactions", message: "No admin transactions found" }));
      return;
    }

    for (const tx of txs) {
      const data = decodeData(tx.data ?? "");
      console.log(JSON.stringify({
        txHash: tx.txHash ?? tx.hash,
        data,
        timestamp: tx.timestamp ?? tx.blockTimestamp,
      }));
    }
  } catch (e) {
    console.error(`API error: ${e.message ?? e}`);
    if (e.response?.status) console.error(`Status: ${e.response.status}`);
    process.exit(1);
  }
}

main();
