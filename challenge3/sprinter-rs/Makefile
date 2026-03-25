.PHONY: build run release clean \
	test-intrashard test-cross-shard test-all-shards test-all-cross-shards test-transfers

build:
	cargo build

release:
	cargo build --release

run:
	cargo run -- $(ARGS)

clean:
	cargo clean

# ── Integration tests (require live network at http://57.129.86.55:8079) ──────

test-intrashard:
	cargo test --test integration_test test_transfer_intrashard -- --ignored --nocapture

test-cross-shard:
	cargo test --test integration_test test_transfer_cross_shard -- --ignored --nocapture

test-all-shards:
	cargo test --test integration_test test_transfer_all_shards -- --ignored --nocapture

test-all-cross-shards:
	cargo test --test integration_test test_transfer_all_cross_shards -- --ignored --nocapture

test-transfers: test-intrashard test-cross-shard test-all-shards test-all-cross-shards
