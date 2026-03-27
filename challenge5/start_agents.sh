#!/bin/bash
set -e

ENV_FILE=".env"

# Ensure .env exists
if [ ! -f "$ENV_FILE" ]; then
  echo "❌ .env file not found. Copy .env.example and fill in your keys."
  exit 1
fi

# 1. Wallet Check
WALLETS_DIR="wallets"
mkdir -p "$WALLETS_DIR"
PEM_COUNT=$(find "$WALLETS_DIR" -maxdepth 1 -name "*.pem" | wc -l)
if [ "$PEM_COUNT" -lt 10 ]; then
  echo "⚠️ Fewer than 10 wallets found ($PEM_COUNT). Running create_wallets.sh..."
  ./create_wallets.sh
else
  echo "✓ 10 wallets found."
fi

# Source .env to read current values
set -a
source "$ENV_FILE"
set +a

# Generate OPENCLAW_GATEWAY_TOKEN if empty
if [ -z "$OPENCLAW_GATEWAY_TOKEN" ]; then
  TOKEN=$(openssl rand -hex 32)
  echo "🔑 Generated OPENCLAW_GATEWAY_TOKEN: $TOKEN"

  # Write it back into .env
  if grep -q "^OPENCLAW_GATEWAY_TOKEN=" "$ENV_FILE"; then
    sed -i "s|^OPENCLAW_GATEWAY_TOKEN=.*|OPENCLAW_GATEWAY_TOKEN=$TOKEN|" "$ENV_FILE"
  else
    echo "OPENCLAW_GATEWAY_TOKEN=$TOKEN" >> "$ENV_FILE"
  fi
else
  echo "✓ OPENCLAW_GATEWAY_TOKEN already set"
fi

echo "🚀 Starting agents..."
docker compose up --build
