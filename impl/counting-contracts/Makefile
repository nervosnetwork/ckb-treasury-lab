MODE := release
BUILD_DIR := build/$(MODE)
CONTRACTS := counting-config-type-script counting-type-script counting-policy-type-script counting-proposal-type-script counting-vote-type-script
PACKAGES := counting-config-type-script counting-type-script counting-policy-type-script counting-proposal-type-script counting-vote-type-script
CUSTOM_RUSTFLAGS := -C debug-assertions
RUSTFLAGS := -C target-feature=+zba,+zbb,+zbc,+zbs -C passes=lower-atomic $(CUSTOM_RUSTFLAGS)
CLANG := $(shell ../scripts/find_clang)
AR := $(subst clang,llvm-ar,$(CLANG))
OBJCOPY := $(subst clang,llvm-objcopy,$(CLANG))

fmt:
	cargo fmt

build:
	rm -rf $(BUILD_DIR)
	mkdir -p $(BUILD_DIR)
	RUSTFLAGS="$(RUSTFLAGS)" TARGET_CC="$(CLANG)" TARGET_AR="$(AR)" \
		cargo build --target=riscv64imac-unknown-none-elf --release $(addprefix -p ,$(PACKAGES))
	@for binary in $(CONTRACTS); do \
		cp target/riscv64imac-unknown-none-elf/$(MODE)/$$binary $(BUILD_DIR)/$$binary.debug; \
		cp $(BUILD_DIR)/$$binary.debug $(BUILD_DIR)/$$binary; \
		$(OBJCOPY) --strip-debug --strip-all $(BUILD_DIR)/$$binary; \
	done

test: build
	TOP=$(CURDIR) MODE=$(MODE) cargo test -p counting-common -p counting-builder -p counting-contract-tests -p counting-live-e2e

clippy:
	cargo clippy -p counting-common -p counting-builder -p counting-contract-tests -p counting-live-e2e --all-targets -- -D warnings
	RUSTFLAGS="$(RUSTFLAGS)" TARGET_CC="$(CLANG)" TARGET_AR="$(AR)" \
		cargo clippy --target=riscv64imac-unknown-none-elf --release $(addprefix -p ,$(PACKAGES)) -- -D warnings

size: build
	@for binary in $(CONTRACTS); do \
		bytes=$$(wc -c < "$(BUILD_DIR)/$$binary" | tr -d ' '); \
		printf '%-36s %8s bytes\n' "$$binary" "$$bytes"; \
	done

live-e2e: build
	CKB_BIN="$${CKB_BIN:-/Users/yukang/code/ckb/target/debug/ckb}" \
	CKB_REPO="$${CKB_REPO:-/Users/yukang/code/ckb}" \
		cargo run -p counting-live-e2e

.PHONY: fmt build test clippy size live-e2e
