This script is based on https://github.com/multiversx/mx-bon-supernova-scripts.

#### 1. Create Wallets

Generate new wallets and save them as PEM files in a specified directory.

```sh
python3 main.py create-wallets --wallets-dir ./wallets --number-of-wallets 500 --balanced
```


#### 2. Distribute eGLD

```sh
python3 sprinter.py fund --wallets-dir ./wallets --whale BoN.pem --network https://gateway.battleofnodes.com --amount 500000000000000000000
```

#### 3. Spamming intrashard transactions

```sh
python3 sprinter.py transfer-all-shards --wallets-dir ./wallets --amount 0 --network https://gateway.battleofnodes.com --total-txs-per-wallet 1000 --batch-size 80 --sleep-time 6
```

### 4. Notes

- There is an exception so we our wallets in shard 1 stopped sending transactions in Window B of challenge #1. We will look at it later.
- There might still be some issue need to be handled, we make this script in rush
