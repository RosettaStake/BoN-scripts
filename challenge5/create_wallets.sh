#!/bin/bash
mkdir -p wallets
for i in {1..10}
do
  mxpy wallet new --format pem --outfile "wallets/mvx-agent-$i.pem"
done
echo "Wallets created in wallets/ folder"
