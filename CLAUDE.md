# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository Overview

A Rust CLI tool that extracts Phabricator code review comments and formats them as Markdown for LLM analysis. The tool handles both regular comments (via API) and JavaScript-rendered inline code suggestions (via web scraping with Firefox cookies).

## Build and Run Commands

```bash
# Build optimized binary
cargo build --release

# Run the tool
./target/release/phab-comments-to-md --diff-id D12345 --token YOUR_TOKEN

# Run with environment variables
export PHABRICATOR_TOKEN=your-token
./target/release/phab-comments-to-md --diff-id 12345

# Run in development with logging
RUST_LOG=debug cargo run -- --diff-id 12345 --token YOUR_TOKEN

# Enable web payload dumping for debugging
./target/release/phab-comments-to-md --diff-id 12345 --token YOUR_TOKEN --dump-web
```

## Architecture Overview

### Dual Authentication System
The tool requires **two authentication mechanisms** working together:
1. **API Token** (`--token` or `PHABRICATOR_TOKEN`): For Phabricator API calls to fetch comment metadata
2. **Firefox Cookies**: Auto-extracted from Firefox's SQLite database for AJAX requests to fetch JavaScript-rendered suggestions

### Two-Stage Extraction Pipeline

**Stage 1: API-based extraction** (`get_transactions`)
- Fetches revision PHID using `differential.revision.search`
- Gets transactions via `transaction.search` API
- Extracts comment metadata: author, timestamp, file path, line numbers
- **Limitation**: API does not return rendered code suggestions (JavaScript-only content)

**Stage 2: Web scraping** (`fetch_suggestion_from_web`)
- Extracts Firefox cookies from `cookies.sqlite` (handles locked databases)
- Scrapes main revision page (`/D{id}`) to extract `ref` parameters
- Posts AJAX requests to `/differential/changeset/` with extracted refs
- Parses HTML response to extract inline code suggestions
- Uses `comment_id` anchors (`inline-{comment_id}`) to match suggestions to comments

### Key Data Structures

- `PhabricatorCommentExtractor`: Main orchestrator with API client, user cache, and ref caches
- `CommentsData`: Holds general comments, inline comments, and review actions
- `InlineComment`: Represents file-level comments with suggestions (has `comment_id` for matching)
- `Comment`: General (non-inline) comments
- `ReviewAction`: Accept/reject/request-changes actions

### Critical Implementation Details

**Firefox Cookie Extraction** (`extract_firefox_cookies`)
- Cross-platform profile detection (Windows/macOS/Linux have different paths)
- Handles locked databases by creating temporary copies
- Selects most recently modified profile with required cookies (`phsid`, `phusr`)
- Fallback to `PHABRICATOR_COOKIES` environment variable

**Changeset Ref Discovery** (`extract_ref_parameters_from_page`)
- Scrapes initial HTML for `ref=<changeset_id>` parameters
- Multiple regex patterns to catch different JavaScript/HTML formats
- Ref parameters identify specific file changesets in AJAX requests

**Suggestion Matching Strategy** (`fetch_suggestion_from_web`)
1. Try `fetch_changeset_data_for_comment`: Uses comment_id anchor for exact matching
2. Check `ref_cache_by_comment`: Caches successful ref-to-comment mappings
3. Verify anchor exists via `changeset_contains_inline_anchor`
4. Fallback to heuristics if anchor-based matching fails

**Response Parsing** (`parse_suggestions_from_ajax`)
- Strips `for (;;);` security prefix from Phabricator AJAX responses
- Parses nested JSON: `{ payload: { changeset: "<html>" } }`
- Extracts diff from `inline-suggestion-view` HTML tables
- Cell-specific selectors: `td.left.old` (removed lines), `td.right.new` (added lines)
- Preserves indentation while stripping aural markers (`"- "`, `"+ "`)

**Done Comment Filtering**
- API provides `isDone` field for inline comments
- By default, filters out resolved comments for cleaner LLM input
- `--include-done` flag includes them with `[DONE]` markers

### Web Payload Debugging

Use `--dump-web` flag to dump all AJAX/HTML/JSON payloads to `./_phab_debug/`:
- Changeset responses are saved with naming: `changeset_ref_{ref}_*.json`
- Helps diagnose suggestion extraction issues
- Files show which ref parameters contain which inline anchors

## Common Development Patterns

### Adding New API Endpoints
1. Define response struct with serde derives (e.g., `TransactionSearchResult`)
2. Use snake_case fields with `#[serde(rename = "camelCase")]` for API JSON
3. Make error handling consistent: check `error_code`, then `result` field
4. Add to `PhabricatorCommentExtractor` as `async fn`

### Modifying Suggestion Extraction
- The anchor-based matching (`extract_suggestion_for_comment_id_in_html`) is the most reliable method
- Avoid proximity heuristics; use `comment_id` anchors when possible
- Test with `--dump-web` to inspect HTML structure changes
- Remember: Phabricator HTML can have multiple inline blocks clustered together

### Handling Platform Differences
Firefox profile paths vary by OS:
- **macOS**: `~/Library/Application Support/Firefox/Profiles/`
- **Linux**: `~/.mozilla/firefox/`
- **Windows**: `%APPDATA%/Mozilla/Firefox/Profiles/`

## Environment Variables

- `PHABRICATOR_TOKEN`: API token (required)
- `PHABRICATOR_BASE_URL`: Override default Mozilla Phabricator URL
- `PHABRICATOR_COOKIES`: Manual cookie override (`"phsid=id; phusr=user"`)
- `RUST_LOG`: Set logging level (`debug`, `info`, `warn`, `error`)
- `RUST_BACKTRACE=1`: Automatically enabled in `main()` for error context

## Codebase-Specific Quirks

- **`for (;;);` prefix**: All Phabricator AJAX responses start with this JavaScript security measure; must be stripped before JSON parsing
- **Gzip handling**: `reqwest` automatically handles gzip decompression via `Accept-Encoding: gzip` header
- **User-Agent required**: Some requests fail without proper User-Agent header mimicking Firefox
- **CSRF tokens**: Required for changeset AJAX requests; extracted from main revision page HTML
- **Progress bars**: Uses `indicatif` with steady tick animation; must call `finish_and_clear()` before final output
- **No tests**: This is a utility tool with no test suite; test manually with real Phabricator revisions
