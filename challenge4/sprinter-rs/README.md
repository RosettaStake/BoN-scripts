  # Build
  cargo build --release

  # Set alias                                                                                                                           
  alias sprinter='./target/release/sprinter'                                                                                        
                                                            
  # 1. Create wallets (20 S0, 60 S1, 20 S2)                                                                                         
  sprinter create-wallets --wallets-dir wallets/ --shards 20,60,20
                                                                                                                                    
  # 2. Fund small amount for deploy gas (~0.1 EGLD/wallet = 10 EGLD)
  sprinter fund --wallets-dir wallets/ --whale BoN.pem --amount 10000000000000000000
                                                                                                                                    
  # 3. Deploy forwarders (1 per shard, batches of 20)
  sprinter challenge4 deploy --wallets-dir wallets/ --wasm-path forwarder-blind-bon.wasm                                            
                                                                                                                                    
  # 4. Collect remaining EGLD back
  sprinter collect --wallets-dir wallets/ --destination erd168emr3utuznv4cy0g55sw43yyqd86lkhr7jzzrcf0rcp3znfas7sjqp008              
                                                                                                                                    
  # 5. Fund with competition budget (after 500 EGLD received)
  sprinter fund --wallets-dir wallets/ --whale BoN.pem --amount 500000000000000000000                                               
                                                            
  # 6. Wrap EGLD → WEGLD (0.015 EGLD/wallet = 1.5 EGLD total)                                                                       
  sprinter challenge4 wrap --wallets-dir wallets/
                                                                                                                                    
  # 7. Spam (fires at 15:59:59.5 UTC)                                                                                               
  sprinter challenge4 spam --wallets-dir wallets/ --gas-limit 30000000 --gas-limit-cross 30000000 --milestone-gas-price 5000000000
  --gas-price 1000000000 --start-at 15:59:59.5                                                                                      
                                                            
  # 8. Collect funds back (post-challenge)                                                                                          
  sprinter collect --wallets-dir wallets/ --destination erd168emr3utuznv4cy0g55sw43yyqd86lkhr7jzzrcf0rcp3znfas7sjqp008
