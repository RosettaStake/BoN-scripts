#!/usr/bin/env python3

import argparse
import concurrent.futures
import random
import sys
import time
from pathlib import Path
from typing import Optional
from multiversx_sdk import Account, TransactionComputer, TransfersController, Address, UserPEM, UserSecretKey
from multiversx_sdk.network_providers import ProxyNetworkProvider
from multiversx_sdk.core.address import AddressComputer
from multiversx_sdk.core import Transaction

MIN_GAS_PRICE = 1_000_000_000
GAS_PRICE_TIERS_FILE = Path("gas_price_tiers.txt")

def load_gas_price_tiers() -> list[int]:
    try:
        if GAS_PRICE_TIERS_FILE.exists():
            prices = []
            for line in GAS_PRICE_TIERS_FILE.read_text().strip().splitlines():
                line = line.strip()
                if line and not line.startswith("#"):
                    p = int(line)
                    if p < MIN_GAS_PRICE:
                        print(f"⚠️ Gas price {p} is below MIN_GAS_PRICE ({MIN_GAS_PRICE}), clamping to {MIN_GAS_PRICE}")
                        p = MIN_GAS_PRICE
                    prices.append(p)
            if prices:
                print(f"📎 Loaded {len(prices)} gas price tiers from {GAS_PRICE_TIERS_FILE}: {prices}")
                return prices
    except (ValueError, OSError) as e:
        print(f"⚠️ Failed to read {GAS_PRICE_TIERS_FILE}: {e}")
    print(f"📎 Using default gas price tier: [{MIN_GAS_PRICE}]")
    return [MIN_GAS_PRICE]


class WalletManager:
    """Manages wallets organized by shards."""

    def __init__(self, wallets_dir: str, network_url: str):
        self.wallets_dir = Path(wallets_dir)
        self.provider = ProxyNetworkProvider(network_url)
        self.address_computer = AddressComputer()
        self.transaction_computer = TransactionComputer()

        # Wallet lists organized by shard
        self.shard_0_wallets: list[Account] = []
        self.shard_1_wallets: list[Account] = []
        self.shard_2_wallets: list[Account] = []

    def load_wallets(self):
        """Load all PEM files from the wallets directory and organize by shard."""
        if not self.wallets_dir.exists():
            print(f"Error: Directory {self.wallets_dir} does not exist.")
            sys.exit(1)

        pem_files = list(self.wallets_dir.glob("*.pem"))

        if not pem_files:
            print(f"Error: No PEM files found in {self.wallets_dir}")
            sys.exit(1)

        print(f"Loading {len(pem_files)} wallet(s) from {self.wallets_dir}...")

        for pem_file in pem_files:
            try:
                account = Account.new_from_pem(pem_file)
                shard = self.address_computer.get_shard_of_address(account.address)

                if shard == 0:
                    self.shard_0_wallets.append(account)
                elif shard == 1:
                    self.shard_1_wallets.append(account)
                elif shard == 2:
                    self.shard_2_wallets.append(account)
            except Exception as e:
                print(f"  ✗ Failed to load {pem_file.name}: {e}")

        print(f"\nWallets loaded by shard:")
        print(f"  Shard 0: {len(self.shard_0_wallets)} wallet(s)")
        print(f"  Shard 1: {len(self.shard_1_wallets)} wallet(s)")
        print(f"  Shard 2: {len(self.shard_2_wallets)} wallet(s)")
        print(
            f"  Total: {len(self.shard_0_wallets) + len(self.shard_1_wallets) + len(self.shard_2_wallets)} wallet(s)\n")

    def check_wallets(self):
        """Query the blockchain and print the current nonce and balance for every loaded wallet."""
        all_wallets = self.get_all_wallets()
        if not all_wallets:
            print("Error: No wallets loaded.")
            return

        print(f"Querying network for {len(all_wallets)} wallets...")
        print(f"{'Address':<65} | {'Shard':<5} | {'Nonce':<10} | {'Balance (EGLD)'}")
        print("-" * 110)

        def fetch(wallet: Account):
            try:
                account_data = self.provider.get_account(wallet.address)
                shard = self.address_computer.get_shard_of_address(wallet.address)
                balance_egld = account_data.balance / 10 ** 18
                return f"{wallet.address.to_bech32():<65} | {shard:<5} | {account_data.nonce:<10} | {balance_egld:.6f}"
            except Exception as e:
                return f"{wallet.address.to_bech32():<65} | Error fetching: {e}"

        with concurrent.futures.ThreadPoolExecutor(max_workers=16) as executor:
            lines = list(executor.map(fetch, all_wallets))

        for line in lines:
            print(line)
        print("-" * 110)

    def get_all_wallets(self) -> list[Account]:
        return self.shard_0_wallets + self.shard_1_wallets + self.shard_2_wallets

    def get_wallets_by_shard(self, shard: int) -> list[Account]:
        if shard == 0:
            return self.shard_0_wallets
        elif shard == 1:
            return self.shard_1_wallets
        elif shard == 2:
            return self.shard_2_wallets
        return []

    def fund_wallets(self, whale_pem_path: str, amount: int = 0):
        all_wallets = self.get_all_wallets()

        if not all_wallets:
            print("Error: No wallets loaded. Please load wallets first.")
            return

        whale_pem = Path(whale_pem_path)
        if not whale_pem.exists():
            print(f"Error: Whale wallet {whale_pem_path} does not exist.")
            sys.exit(1)

        try:
            whale = Account.new_from_pem(whale_pem)
            print(f"Whale wallet: {whale.address.to_bech32()}")
        except Exception as e:
            print(f"Error loading whale wallet: {e}")
            sys.exit(1)

        whale_account = self.provider.get_account(whale.address)
        whale_balance = int(whale_account.balance)
        print(f"Whale balance: {whale_balance / 10 ** 18:.4f} EGLD ({whale_balance} atomic units)")

        num_transactions = len(all_wallets)
        fee_reserve = num_transactions * (50_000 * 1000000000)

        if not amount:
            total_to_distribute = whale_balance - fee_reserve

            if total_to_distribute <= 0:
                print("Error: Insufficient balance for distribution (need to cover transaction fees).")
                return

            print(f"Distributing all available balance (minus fee reserve): {total_to_distribute / 10 ** 18:.4f} EGLD")
        else:
            if amount > whale_balance:
                print(f"Error: Specified amount exceeds whale balance.")
                return

            total_to_distribute = amount

            if total_to_distribute <= 0:
                print(
                    f"Error: Specified amount is not enough to cover the {fee_reserve / 10 ** 18:.4f} EGLD funding fees.")
                return

            print(f"Distributing specified amount (minus fee reserve): {total_to_distribute / 10 ** 18:.4f} EGLD")

        amount_per_wallet = total_to_distribute // len(all_wallets)
        total_spent = amount_per_wallet * len(all_wallets)
        remaining = whale_balance - total_spent - fee_reserve

        print(f"Amount per wallet: {amount_per_wallet / 10 ** 18:.6f} EGLD ({amount_per_wallet} atomic units)")
        print(f"Target wallets: {len(all_wallets)}")
        print(f"Whale remaining balance after funding fees: {remaining / 10 ** 18:.6f} EGLD\n")

        whale.nonce = whale_account.nonce
        chain_id = self.provider.get_network_config().chain_id
        controller = TransfersController(chain_id)

        all_transactions: list[Transaction] = []

        for wallet in all_wallets:
            tx = controller.create_transaction_for_native_token_transfer(
                sender=whale,
                nonce=whale.get_nonce_then_increment(),
                receiver=wallet.address,
                native_transfer_amount=amount_per_wallet,
                gas_limit=50000
            )
            all_transactions.append(tx)

        print("Sending funding transactions in batches of 50...")
        num_txs = 0
        burst_size = 50
        sleep_time = 6

        while all_transactions:
            burst = all_transactions[:burst_size]
            all_transactions = all_transactions[burst_size:]

            print(f"Sending batch of {len(burst)} funding transactions...")
            batch_num_txs, _ = self.provider.send_transactions(burst)
            num_txs += batch_num_txs
            print(f"  -> Batch successful ({batch_num_txs} txs sent).")

            if all_transactions:
                print(f"  -> Sleeping for {sleep_time} seconds to clear mempool...")
                time.sleep(sleep_time)

        print(f"\nSent {num_txs} funding transaction(s).")
        print(f"{'=' * 60}")

    def _get_relayer_account(self, relayer_address: str, shard_wallets: list[Account], shard: int) -> Account:
        relayer_addr = Address.new_from_bech32(relayer_address)
        relayer_shard = self.address_computer.get_shard_of_address(relayer_addr)

        if relayer_shard != shard:
            print(f"Warning: Relayer address is in shard {relayer_shard}, but transactions are in shard {shard}")
            sys.exit(1)

        relayer_pubkey = relayer_addr.get_public_key()
        for wallet in shard_wallets:
            if wallet.address.get_public_key() == relayer_pubkey:
                return wallet

        print(f"Error: Relayer address {relayer_address} not found in loaded wallets for shard {shard}.")
        sys.exit(1)

    def _sign_using_relayer(self, transaction: Transaction, relayer: Account) -> Transaction:
        serialized = self.transaction_computer.compute_bytes_for_signing(transaction)
        signature = relayer.sign(serialized)
        transaction.relayer_signature = signature
        return transaction

    def generate_intrashard_txs(self, shard: int, amount: int, relayer_address: Optional[str] = None,
                                random_relayer: bool = False, total_txs_per_wallet: int = 99) -> list[Transaction]:
        shard_wallets = self.get_wallets_by_shard(shard)
        if not shard_wallets:
            print(f"[SHARD {shard}] Error: No wallets found.")
            return []
        if shard not in [0, 1, 2]: return []

        print(f"[SHARD {shard}] Loaded {len(shard_wallets)} wallet(s)")
        print(f"[SHARD {shard}] Target: {total_txs_per_wallet} transactions per wallet")

        relayer_account: Account | None = None
        if relayer_address:
            relayer_account = self._get_relayer_account(relayer_address, shard_wallets, shard)
        elif random_relayer:
            print(f"[SHARD {shard}] Using random relayer for each transaction")

        # Build eligible-relayer lists once per sender (not once per tx)
        sender_to_eligible_relayers: dict[str, list[Account]] = {}
        if random_relayer:
            sender_pubkeys = {w.address.get_public_key() for w in shard_wallets}
            if len(sender_pubkeys) < 2:
                print(f"Error: Not enough wallets to randomly pick a relayer that is not the sender.")
                return []
            for sender in shard_wallets:
                sender_key = sender.address.get_public_key()
                sender_to_eligible_relayers[sender_key] = [w for w in shard_wallets if
                                                           w.address.get_public_key() != sender_key]

        print(f"[SHARD {shard}] Syncing wallet nonces...")
        with concurrent.futures.ThreadPoolExecutor(max_workers=min(len(shard_wallets), 16)) as executor:
            def sync_nonce(wallet: Account):
                wallet.nonce = self.provider.get_account(wallet.address).nonce

            list(executor.map(sync_nonce, shard_wallets))

        return self._generate_unsigned_txs(
            label=f"SHARD {shard}",
            sender_wallets=shard_wallets,
            receiver_wallets=shard_wallets,
            amount=amount,
            relayer_account=relayer_account,
            random_relayer=random_relayer,
            sender_to_eligible_relayers=sender_to_eligible_relayers,
            total_txs_per_wallet=total_txs_per_wallet,
        )

    def _generate_unsigned_txs(
            self,
            label: str,
            sender_wallets: list[Account],
            receiver_wallets: list[Account],
            amount: int,
            relayer_account: Optional[Account],
            random_relayer: bool,
            sender_to_eligible_relayers: dict[str, list[Account]],
            total_txs_per_wallet: int,
    ) -> list[tuple[Transaction, Account, Optional[Account]]]:
        """Generate unsigned transactions. Returns list of (tx, sender, relayer_or_None)."""
        chain_id = self.provider.get_network_config().chain_id

        all_entries: list[tuple[Transaction, Account, Optional[Account]]] = []
        total_txs = len(sender_wallets) * total_txs_per_wallet
        print(f"[{label}] Pre-generating {total_txs} unsigned transactions in memory...")

        for tx_idx in range(total_txs_per_wallet):
            for sender in sender_wallets:
                receiver = random.choice(receiver_wallets)
                relayer: Optional[Account] = None

                if relayer_account:
                    relayer = relayer_account
                elif random_relayer:
                    relayer = random.choice(sender_to_eligible_relayers[sender.address.get_public_key()])

                tx = Transaction(
                    sender=sender.address,
                    receiver=receiver.address,
                    gas_limit=50_000,
                    chain_id=chain_id,
                    nonce=sender.get_nonce_then_increment(),
                    value=amount,
                    relayer=relayer.address if relayer else None,
                )

                all_entries.append((tx, sender, relayer))

        print(f"[{label}] Created {len(all_entries)} unsigned transactions.")
        return all_entries

    def _sign_txs(self, entries: list[tuple[Transaction, Account, Optional[Account]]]) -> list[Transaction]:
        """Assign gas price tiers and sign all transactions."""
        tiers = load_gas_price_tiers()
        num_tiers = len(tiers)
        total = len(entries)
        tier_size = total // num_tiers

        for i, gas_price in enumerate(tiers):
            start = i * tier_size
            end = (i + 1) * tier_size if i < num_tiers - 1 else total
            print(f"  Tier {i + 1}: txs [{start}..{end}) ({end - start} txs) -> gas_price={gas_price}")

        signed: list[Transaction] = []
        for idx, (tx, sender, relayer) in enumerate(entries):
            tier_idx = min(idx // tier_size, num_tiers - 1) if tier_size > 0 else num_tiers - 1
            tx.gas_price = tiers[tier_idx]
            tx.signature = sender.sign_transaction(tx)
            if relayer:
                self._sign_using_relayer(tx, relayer)
            signed.append(tx)

        print(f"  Signed {len(signed)} transactions.")
        return signed

    def broadcast_txs(self, label: str, all_transactions: list[Transaction], num_wallets: int, batch_size: int = 99,
                      sleep_time: int = 6):
        if not all_transactions: return

        burst_size = batch_size * num_wallets
        print(f"[{label}] Broadcasting in bursts of {batch_size} txs per wallet ({burst_size} total per burst)...")

        num_txs = 0
        while all_transactions:
            burst = all_transactions[:burst_size]
            all_transactions = all_transactions[burst_size:]

            print(f"[{label}] Sending burst of {len(burst)} transactions...")

            burst_start_time = time.time()
            burst_sent_count = 0
            offset = 0

            while offset < len(burst):
                chunk = burst[offset:offset + 1000]

                while chunk:
                    try:
                        batch_num_txs, hashes = self.provider.send_transactions(chunk)
                    except Exception as e:
                        print(f"[{label}] ❌ send_transactions raised: {e}. Retrying after 1s...")
                        time.sleep(1)
                        burst_start_time = time.time()
                        continue

                    burst_sent_count += batch_num_txs
                    num_txs += batch_num_txs

                    if batch_num_txs == len(chunk):
                        break  # all accepted, move to next 1000-chunk

                    # Retry ONLY the rejected txs, not the next unsent ones
                    chunk = [tx for tx, h in zip(chunk, hashes) if not h]
                    print(
                        f"[{label}] ⚠️ Node accepted {batch_num_txs}/{len(chunk) + batch_num_txs} txs. Retrying {len(chunk)} unaccepted txs...")

                    elapsed = time.time() - burst_start_time
                    retry_sleep = max(0, sleep_time - elapsed)
                    if retry_sleep:
                        print(f"[{label}] -> Synchronizing block heartbeat for retry (sleeping {retry_sleep:.2f}s)...")
                        time.sleep(retry_sleep)
                    burst_start_time = time.time()

                offset += 1000

            burst_elapsed = time.time() - burst_start_time
            print(f"[{label}] -> Burst successful ({burst_sent_count} txs sent in {burst_elapsed:.2f}s).")

            if all_transactions:
                time_to_sleep = sleep_time - burst_elapsed
                if time_to_sleep > 0:
                    print(
                        f"[{label}] -> Synchronizing block heartbeat (sleeping {time_to_sleep:.2f}s to fill the remaining {sleep_time}s block window)...")
                    time.sleep(time_to_sleep)

        print(f"\n[{label}] Successfully broadcasted {num_txs} transactions!")

def wait_for_user_confirmation(self):
    print("\n" + "=" * 60)
    print("✅ ALL TRANSACTIONS GENERATED AND SEQUENCED IN MEMORY")
    print("=" * 60)
    while True:
        try:
            ans = input("Ready to BLAST? Press 'Y' then Enter to begin broadcasting: ")
            if ans.strip().lower() == 'y':
                print("🚀 BLAST INITIATED 🚀\n")
                break
        except EOFError:
            break


def transfer_intrashard(self, shard: int, amount: int, relayer_address: Optional[str] = None,
                        random_relayer: bool = False, total_txs_per_wallet: int = 99, batch_size: int = 99,
                        sleep_time: int = 6):
    entries = self.generate_intrashard_txs(shard, amount, relayer_address, random_relayer, total_txs_per_wallet)
    if not entries: return
    txs = self._sign_txs(entries)
    self.wait_for_user_confirmation()
    self.broadcast_txs(f"SHARD {shard}", txs, len(self.get_wallets_by_shard(shard)), batch_size, sleep_time)


def transfer_all_shards(self, amount: int, total_txs_per_wallet: int = 99, batch_size: int = 99, sleep_time: int = 6):
    """
    Multithreaded blaster targeting all three completely independent shard mempools concurrently.
    """
    print("\n" + "=" * 60)
    print("🚀 INITIATING MULTISHARD MEMORY GENERATION 🚀")
    print(f"Target: {total_txs_per_wallet} tx/wallet across ALL active shards")
    print("=" * 60 + "\n")

    with concurrent.futures.ThreadPoolExecutor(max_workers=3) as executor:
        futures = {
            shard: executor.submit(self.generate_intrashard_txs, shard, amount, None, False, total_txs_per_wallet)
            for shard in [0, 1, 2]
        }
        entries_by_shard = {shard: f.result() for shard, f in futures.items()}

    total_generated = sum(len(e) for e in entries_by_shard.values())
    if total_generated == 0:
        print("No transactions generated.")
        return

    # Interleave entries across shards so tiers are evenly distributed
    all_entries: list[tuple[int, int, Transaction, Account, Optional[Account]]] = []
    max_len = max(len(e) for e in entries_by_shard.values())
    for i in range(max_len):
        for shard, entries in entries_by_shard.items():
            if i < len(entries):
                tx, sender, relayer = entries[i]
                all_entries.append((shard, i, tx, sender, relayer))

    print(f"\nApplying gas price tiers across {len(all_entries)} total transactions...")
    tiers = load_gas_price_tiers()
    num_tiers = len(tiers)
    tier_size = len(all_entries) // num_tiers
    for i, gas_price in enumerate(tiers):
        start = i * tier_size
        end = (i + 1) * tier_size if i < num_tiers - 1 else len(all_entries)
        print(f"  Tier {i + 1}: txs [{start}..{end}) ({end - start} txs) -> gas_price={gas_price}")

    for idx, (shard, orig_idx, tx, sender, relayer) in enumerate(all_entries):
        tier_idx = min(idx // tier_size, num_tiers - 1) if tier_size > 0 else num_tiers - 1
        tx.gas_price = tiers[tier_idx]
        tx.signature = sender.sign_transaction(tx)
        if relayer:
            self._sign_using_relayer(tx, relayer)

    print(f"  Signed {len(all_entries)} transactions.")

    # Rebuild per-shard tx lists preserving original order
    txs_by_shard: dict[int, list[Transaction]] = {s: [] for s in entries_by_shard}
    for shard, orig_idx, tx, sender, relayer in all_entries:
        txs_by_shard[shard].append(tx)

    self.wait_for_user_confirmation()

    with concurrent.futures.ThreadPoolExecutor(max_workers=3) as executor:
        broadcast_futures = [
            executor.submit(self.broadcast_txs, f"SHARD {shard}", txs, len(self.get_wallets_by_shard(shard)),
                            batch_size, sleep_time)
            for shard, txs in txs_by_shard.items() if txs
        ]
        concurrent.futures.wait(broadcast_futures)

    for f in broadcast_futures:
        exc = f.exception()
        if exc:
            print(f"⚠️ A shard broadcaster thread failed with: {exc}")

    print("\n" + "=" * 60)
    print("✅ MULTISHARD BLASTER COMPLETE")
    print("=" * 60 + "\n")


def main():
    parser = argparse.ArgumentParser(description="MultiversX Guild Wars 1M Sprinter CLI")
    subparsers = parser.add_subparsers(dest="command", required=True)

    # Fund
    fund_parser = subparsers.add_parser("fund", help="Fund all wallets from a whale wallet")
    fund_parser.add_argument("--wallets-dir", type=str, required=True)
    fund_parser.add_argument("--network", type=str, required=True)
    fund_parser.add_argument("--whale", type=str, required=True)
    fund_parser.add_argument("--amount", type=int)

    # Intrashard
    transfer_parser = subparsers.add_parser("transfer-intrashard")
    transfer_parser.add_argument("--wallets-dir", type=str, required=True)
    transfer_parser.add_argument("--network", type=str, required=True)
    transfer_parser.add_argument("--shard", type=int, required=True, choices=[0, 1, 2])
    transfer_parser.add_argument("--amount", type=int, required=True)
    transfer_parser.add_argument("--relayer", type=str)
    transfer_parser.add_argument("--random-relayer", action="store_true")
    transfer_parser.add_argument("--total-txs-per-wallet", type=int, default=99)
    transfer_parser.add_argument("--batch-size", type=int, default=99)
    transfer_parser.add_argument("--sleep-time", type=int, default=6)

    # Transfer All Shards (Concurrent Blast Threading)
    all_shards_parser = subparsers.add_parser("transfer-all-shards")
    all_shards_parser.add_argument("--wallets-dir", type=str, required=True)
    all_shards_parser.add_argument("--network", type=str, required=True)
    all_shards_parser.add_argument("--amount", type=int, required=True)
    all_shards_parser.add_argument("--total-txs-per-wallet", type=int, default=99)
    all_shards_parser.add_argument("--batch-size", type=int, default=99)
    all_shards_parser.add_argument("--sleep-time", type=int, default=6)

    # Check Wallets
    check_parser = subparsers.add_parser("check-wallets", help="List nonces and balances of all loaded wallets")
    check_parser.add_argument("--wallets-dir", type=str, required=True)
    check_parser.add_argument("--network", type=str, required=True)

    # Create Wallets
    create_wallet = subparsers.add_parser("create-wallets")
    create_wallet.add_argument("--wallets-dir", type=str, required=True)
    create_wallet.add_argument("--number-of-wallets", type=int, required=True)
    create_wallet.add_argument("--balanced", action="store_true")

    args = parser.parse_args()

    if args.command == "fund":
        manager = WalletManager(args.wallets_dir, args.network)
        manager.load_wallets()
        manager.fund_wallets(args.whale, args.amount)

    elif args.command == "transfer-intrashard":
        manager = WalletManager(args.wallets_dir, args.network)
        manager.load_wallets()
        manager.transfer_intrashard(args.shard, args.amount, getattr(args, 'relayer', None),
                                    getattr(args, 'random_relayer', False), args.total_txs_per_wallet, args.batch_size,
                                    args.sleep_time)

    elif args.command == "transfer-all-shards":
        manager = WalletManager(args.wallets_dir, args.network)
        manager.load_wallets()
        manager.transfer_all_shards(args.amount, args.total_txs_per_wallet, args.batch_size, args.sleep_time)

    elif args.command == "check-wallets":
        manager = WalletManager(args.wallets_dir, args.network)
        manager.load_wallets()
        manager.check_wallets()

    elif args.command == "create-wallets":
        print("Creating wallets ...")
        num_wallets = args.number_of_wallets
        balanced = getattr(args, 'balanced', False)
        path_to_save = Path(args.wallets_dir)
        path_to_save.mkdir(parents=True, exist_ok=True)

        address_computer = AddressComputer()

        if balanced:
            base_count = num_wallets // 3
            remainder = num_wallets % 3
            quotas = {
                0: base_count + (1 if remainder > 0 else 0),
                2: base_count + (1 if remainder > 1 else 0),
                1: base_count
            }
            print(f"Balanced mode: Target quotas -> Shard 0: {quotas[0]}, Shard 1: {quotas[1]}, Shard 2: {quotas[2]}")

            created_per_shard = {0: 0, 1: 0, 2: 0}
            total_created = 0

            while total_created < num_wallets:
                user_secret_key = UserSecretKey.generate()
                public_key = user_secret_key.generate_public_key().to_address()
                shard = address_computer.get_shard_of_address(public_key)

                if created_per_shard[shard] < quotas[shard]:
                    label = public_key.to_bech32()
                    pem = UserPEM(label=label, secret_key=user_secret_key)
                    pem_path = path_to_save / f"{label}.pem"
                    pem.save(pem_path)

                    created_per_shard[shard] += 1
                    total_created += 1

                    if total_created % 50 == 0 or total_created == num_wallets:
                        print(
                            f"  Created {total_created}/{num_wallets} (S0: {created_per_shard[0]}, S1: {created_per_shard[1]}, S2: {created_per_shard[2]})")
        else:
            for _ in range(num_wallets):
                user_secret_key = UserSecretKey.generate()
                public_key = user_secret_key.generate_public_key().to_address()
                label = public_key.to_bech32()
                pem = UserPEM(label=label, secret_key=user_secret_key)
                pem_path = path_to_save / f"{label}.pem"
                pem.save(pem_path)

        print(f"Created {num_wallets} wallet(s) in {path_to_save.resolve()}")


if __name__ == "__main__":
    main()
