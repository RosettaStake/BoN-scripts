WINDOW A:

  cargo run --release create-wallets --wallets-dir ./A --number-of-wallets 498 --balanced
  
  cargo run --release fund --wallets-dir ./A --whale ./BoN.pem  --amount 2000000000000000000000
  
  cargo run --release transfer-all-cross-shards --wallets-dir ./A --batch-size 4 --amount 1 --sleep-time 0 --sign-threads 8 --send-parallelism 8

WINDOW B:

  cargo run --release create-wallets --wallets-dir ./B --number-of-wallets 498 --balanced
  
  cargo run --release fund --wallets-dir ./B --whale ./BoN.pem  --amount 500000000000000000000
  
  cargo run --release transfer-all-cross-shards --wallets-dir ./B --batch-size 1 --amount 10000000000000000 --sleep-time 0 --sign-threads 8 --send-parallelism 8
