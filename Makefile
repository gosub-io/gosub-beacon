.SILENT:

SHELL=/usr/bin/env bash

all: help

test: test_unit test_clippy test_fmt  ## Runs tests

bench: ## Benchmark the project
	cargo bench

build: ## Build the project
	if [ "$$(uname)" = Darwin ]; then \
		echo "make build builds the GTK frontend, which does not build on macOS." ;\
		echo "On a Mac: cargo build -p beacon-ffi, then cd swift && swift run BeaconMac (see swift/README.md)." ;\
		exit 1 ;\
	fi ;\
	source test-utils.sh ;\
	section "Cargo build" ;\
	cargo build --all

fix-format:  ## Fix formatting and clippy errors
	cargo fmt --all
	cargo clippy --all --fix --allow-dirty --allow-staged

check-format: test_clippy test_fmt ## Check the project for clippy and formatting errors

test_unit:
	source test-utils.sh ;\
	section "Cargo test" ;\
	cargo test --all --no-fail-fast --all-targets

test_clippy:
	source test-utils.sh ;\
	section "Cargo clippy" ;\
	cargo clippy -- -D warnings

test_fmt:
	source test-utils.sh ;\
	section "Cargo fmt" ;\
	cargo fmt --all -- --check

help: ## Display available commands
	echo "Available make commands:"
	echo
	grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-30s\033[0m %s\n", $$1, $$2}'
