# writeme — a terminal writing surface that doesn't suck
#
# One artifact: the binary. No daemon, no install dir bullshit.

CARGO    ?= cargo
BIN_NAME ?= wme
TARGET   ?= release

BUILT_BIN := target/$(TARGET)/$(BIN_NAME)

INSTALL_DIR := $(HOME)/.local/bin
INSTALL_BIN := $(INSTALL_DIR)/$(BIN_NAME)

.PHONY: help check clippy build run install uninstall test clean

.DEFAULT_GOAL := check

help:
	@echo "writeme — a terminal writing surface that doesn't suck"
	@echo ""
	@echo "fast iteration (no link):"
	@echo "  make check       cargo check"
	@echo "  make clippy      lints + check"
	@echo ""
	@echo "build + install:"
	@echo "  make build       compile a release binary at $(BUILT_BIN)"
	@echo "  make run         build + run with no args"
	@echo "  make install     build + copy to $(INSTALL_BIN)"
	@echo "  make uninstall   remove $(INSTALL_BIN)"
	@echo ""
	@echo "  make test        cargo test"
	@echo "  make clean       cargo clean"

check:
	$(CARGO) check

clippy:
	$(CARGO) clippy -- -D warnings

build:
	$(CARGO) build --$(TARGET)

run: build
	$(BUILT_BIN)

install: build
	@mkdir -p "$(INSTALL_DIR)"
	@install -m 0755 "$(BUILT_BIN)" "$(INSTALL_BIN)"
	@echo "installed -> $(INSTALL_BIN)"

uninstall:
	@if [ -e "$(INSTALL_BIN)" ]; then \
		rm -f "$(INSTALL_BIN)"; \
		echo "removed -> $(INSTALL_BIN)"; \
	else \
		echo "no $(INSTALL_BIN) — already gone"; \
	fi

test:
	$(CARGO) test

clean:
	$(CARGO) clean
