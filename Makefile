.DEFAULT_GOAL := help

.PHONY: help install release

help:
	@printf '%s\n' \
		'Targets:' \
		'  make install                 Install the local badgers CLI with Cargo' \
		'  make release                 Interactively bump and publish a release' \
		'  make release BUMP=patch      Noninteractively bump and publish a release'

install:
	cargo install --locked --force --path crates/badgers-cli

release:
	@bash scripts/release.sh
