.PHONY: default test build release fmt clean

# The CLI is pinned to the version zenex-contracts builds with. The
# `stellar-cli` target checks that the `stellar` on PATH is that version, so a
# different install fails the build instead of emitting different artifacts.
STELLAR_CLI_VERSION ?= 27.1.0
# Exported, not spliced into the recipe, so the value never reaches the shell
# as text.
export STELLAR_CLI_VERSION

default: build

# Fails unless the `stellar` on PATH is the pinned version.
.PHONY: stellar-cli
stellar-cli:
	@stellar --version | grep -q "^stellar $$STELLAR_CLI_VERSION " || \
		{ echo "stellar-cli $$STELLAR_CLI_VERSION is required, found: $$(stellar --version | head -1)" >&2; exit 1; }

# `--tests` selects every target with `test = true`, the in-crate unit suites
# included. `--lib` is named alongside it so the intent reads explicitly. The
# tests need no built WASM: the only WASM they load is committed testdata.
test:
	cargo test --locked --all --lib --tests

build: stellar-cli
	stellar contract build --locked --optimize

# Builds the release WASMs into `wasm/` with the checked-out commit embedded as
# the `source_commit` contract meta. A changed or untracked file could reach the
# build and make the embedded commit lie, so the target fails unless
# `git status` is clean. Commit the output in the next commit: a file cannot
# contain the hash of its own commit.
release: stellar-cli
	@test -z "$$(git status --porcelain)" || { echo "release needs a clean checkout; commit, stash or remove changed and untracked files" >&2; exit 1; }
	rm -f wasm/*.wasm
	stellar contract build --locked --optimize --meta source_commit=$$(git rev-parse HEAD) --out-dir wasm

fmt:
	cargo fmt --all

clean:
	cargo clean
