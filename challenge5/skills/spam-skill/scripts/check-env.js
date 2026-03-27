#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");

require("dotenv").config({ path: path.resolve(__dirname, "../.env") });

const required = [
  "SPAM_ADMIN_ADDRESS",
  "SPAM_TARGET_ADDRESS",
  "MULTIVERSX_PRIVATE_KEY",
];

const missing = required.filter((name) => !process.env[name] || !process.env[name].trim());

if (missing.length > 0) {
  console.error("Missing required env vars:");
  for (const name of missing) {
    console.error(`- ${name}`);
  }
  process.exit(1);
}

const pemPathRaw = process.env.MULTIVERSX_PRIVATE_KEY;
const pemPath = path.isAbsolute(pemPathRaw)
  ? pemPathRaw
  : path.resolve(process.cwd(), pemPathRaw);

if (!fs.existsSync(pemPath)) {
  console.error(`MULTIVERSX_PRIVATE_KEY file not found: ${pemPath}`);
  process.exit(1);
}

console.log("Environment check passed.");
