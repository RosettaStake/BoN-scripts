#!/bin/bash
set -e

# Help message
USAGE="Usage: ./fund_wallets.sh --wallets-dir <dir> --whale <pem> --amount <amount> --network <url>"

# Parse arguments
while [[ "$#" -gt 0 ]]; do
    case $1 in
        --wallets-dir) WALLETS_DIR="$2"; shift ;;
        --whale) WHALE_PEM="$2"; shift ;;
        --amount) AMOUNT="$2"; shift ;;
        --network) PROXY="$2"; shift ;;
        *) echo "Unknown parameter passed: $1"; echo "$USAGE"; exit 1 ;;
    esac
    shift
done

# Validate required arguments
if [ -z "$WALLETS_DIR" ] || [ -z "$WHALE_PEM" ] || [ -z "$AMOUNT" ] || [ -z "$PROXY" ]; then
    echo "❌ Missing required arguments."
    echo "$USAGE"
    exit 1
fi

# Function to get address from pem using the convert command
get_address() {
    mxpy --log-level error wallet convert --infile "$1" --in-format pem --out-format address-bech32 | grep -oE 'erd1[a-z0-9]+'
}

# Ensure wallets directory exists
if [ ! -d "$WALLETS_DIR" ]; then
    echo "❌ Wallets directory $WALLETS_DIR not found."
    exit 1
fi

# Ensure whale PEM exists
if [ ! -f "$WHALE_PEM" ]; then
    echo "❌ Whale PEM $WHALE_PEM not found."
    exit 1
fi

# Get whale address and initial nonce
WHALE_ADDRESS=$(get_address "$WHALE_PEM")
ACCOUNT_JSON=$(mxpy --log-level error get account --address "$WHALE_ADDRESS" --proxy "$PROXY")
NONCE=$(echo "$ACCOUNT_JSON" | jq '.account.nonce')
CHAIN_ID=$(mxpy --log-level error get network-config --proxy "$PROXY" | jq -r '.erd_chain_id')

if [ -z "$NONCE" ] || [ "$NONCE" == "null" ]; then
  echo "❌ Could not fetch nonce for $WHALE_ADDRESS. Check proxy and network connection."
  exit 1
fi

echo "🐋 Whale Wallet: $WHALE_ADDRESS (Initial Nonce: $NONCE)"
echo "💰 Amount per wallet: $AMOUNT"
echo "🌐 Proxy: $PROXY (Chain: $CHAIN_ID)"

# Iterate through wallets
for pem in "$WALLETS_DIR"/*.pem; do
    [ -e "$pem" ] || continue
    
    # Skip the whale PEM if it's in the same directory
    if [ "$(realpath "$pem")" == "$(realpath "$WHALE_PEM")" ]; then
        continue
    fi
    
    RECEIVER_ADDRESS=$(get_address "$pem")
    
    echo "💸 Funding $RECEIVER_ADDRESS (Nonce: $NONCE)..."
    
    # Send transaction
    # We use --send and --gas-limit 50000 (standard transfer)
    mxpy --log-level error tx new --receiver "$RECEIVER_ADDRESS" --value "$AMOUNT" --pem "$WHALE_PEM" --proxy "$PROXY" --chain "$CHAIN_ID" --nonce "$NONCE" --send --gas-limit 50000 > /dev/null
    
    # Increment nonce for the next transaction
    NONCE=$((NONCE + 1))
done

echo "✅ All funding transactions sent!"
