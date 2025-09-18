# AGENTS.md

This file provides guidance to agents when working with code in this repository.

## Build Commands
- `cargo build --release` - Build optimized binary (required for production use)
- `cargo run -- --help` - Show CLI usage (binary must be built first)

## Authentication Requirements
- **Dual authentication needed**: Both API token AND Firefox cookies required for full functionality
- API token: For basic Phabricator API calls (`--token` or `PHABRICATOR_TOKEN` env var)
- Firefox cookies: Auto-extracted from most recent Firefox profile for JavaScript-rendered suggestions
- Manual cookie override: `PHABRICATOR_COOKIES="phsid=id; phusr=user"` if Firefox detection fails

## Non-Standard Patterns
- **Firefox SQLite integration**: Directly reads Firefox `cookies.sqlite` database across platforms
- **Database lock handling**: Creates temp copies when Firefox is running (database locked)
- **AJAX response parsing**: Strips `for (;;);` security prefix from Phabricator responses
- **Dual extraction methods**: API for basic comments + web scraping for JavaScript-rendered suggestions
- **Cross-platform cookie paths**: Different Firefox profile locations on Windows/macOS/Linux

## Critical Implementation Details
- **Progress bar integration**: Uses `indicatif` with steady tick animation during long operations
- **Suggestion extraction**: Prioritizes responses with `suggestionText` > `inline-suggestion-view` > `differential-inline-comment`
- **Chronological sorting**: All comments sorted by timestamp for natural reading flow
- **Done comment filtering**: Excludes resolved comments by default, use `--include-done` to include with [DONE] markers

## Environment Variables
- `PHABRICATOR_TOKEN` - API token (required)
- `PHABRICATOR_BASE_URL` - Base URL (defaults to Mozilla's Phabricator)
- `PHABRICATOR_COOKIES` - Manual cookie override format
- `RUST_LOG` - Logging level (defaults to "info")
- `RUST_BACKTRACE=1` - Enabled by default for error context
