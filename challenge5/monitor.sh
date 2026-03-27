#!/bin/bash

echo "🔍 Command the agents to start monitor"
for i in {1..10}; do
    docker exec --user node mvx-agent-$i node openclaw.mjs agent --session-id "manual-test-$(date +%s)" --message "USE the spam-skill skill start the monitor"
    echo "Agent $i started monitor"
done