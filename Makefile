.DEFAULT_GOAL := help

.PHONY: help install release

help:
	@printf '%s\n' \
		'Targets:' \
		'  make install                 Install the local badgers CLI with Cargo' \
		'  make release VERSION=x.y.z   Verify and publish a GitHub release'

install:
	cargo install --locked --force --path crates/badgers-cli

release:
	@test -n "$(VERSION)" || { echo 'error: VERSION is required (example: make release VERSION=1.2.3)' >&2; exit 2; }
	@case "$(VERSION)" in \
		v*) echo 'error: VERSION must not include the v prefix' >&2; exit 2 ;; \
		*[!0-9A-Za-z.+-]* | *..* | .* | *.) echo 'error: VERSION must be a SemVer-like value such as 1.2.3 or 1.2.3-rc.1' >&2; exit 2 ;; \
		*.*.*) ;; \
		*) echo 'error: VERSION must contain major, minor, and patch components' >&2; exit 2 ;; \
	esac
	@test -z "$$(git status --porcelain)" || { echo 'error: the worktree must be clean' >&2; git status --short >&2; exit 1; }
	@test "$$(git branch --show-current)" = main || { echo 'error: releases must be created from main' >&2; exit 1; }
	@gh auth status >/dev/null
	@git fetch --quiet origin main --tags
	@test "$$(git rev-parse HEAD)" = "$$(git rev-parse origin/main)" || { echo 'error: local main must match origin/main' >&2; exit 1; }
	@if git rev-parse --verify --quiet "refs/tags/v$(VERSION)" >/dev/null; then echo 'error: tag v$(VERSION) already exists' >&2; exit 1; fi
	@if gh release view "v$(VERSION)" >/dev/null 2>&1; then echo 'error: release v$(VERSION) already exists' >&2; exit 1; fi
	cargo fmt --check
	cargo test --workspace
	cargo clippy --workspace --all-targets -- -D warnings
	@case "$(VERSION)" in \
		*-*) prerelease=--prerelease ;; \
		*) prerelease= ;; \
	esac; \
	gh release create "v$(VERSION)" --target "$$(git rev-parse HEAD)" --title "v$(VERSION)" --generate-notes $$prerelease
	@case "$(VERSION)" in \
		*-*) ;; \
		*) version='$(VERSION)'; \
			major="$${version%%.*}"; \
			expected="$$(git rev-parse --verify "refs/tags/v$$major" 2>/dev/null || true)"; \
			git push --force-with-lease="refs/tags/v$$major:$$expected" origin "$$(git rev-parse HEAD):refs/tags/v$$major" ;; \
	esac
