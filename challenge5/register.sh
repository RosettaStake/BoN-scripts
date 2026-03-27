#!/bin/bash
# Script to trigger registration for all 10 agents

set -e

for i in {1..10}; do
  CONTAINER="mvx-agent-$i"
  echo "🏗️  Registration for $CONTAINER..."
  
  # Run the registration script inside the container
  # We use --user node since that's the default context in the Dockerfile
  docker exec --user node -w /home/node/.openclaw/workspace/skills/multiversx/moltbot-starter-kit "$CONTAINER" npx ts-node scripts/register.ts
  
  echo "-----------------------------------"
done

echo "✅ All 10 agents have been processed."
