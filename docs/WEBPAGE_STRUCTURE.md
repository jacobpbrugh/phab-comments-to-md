# Phabricator Differential Webpage Map and Extraction Strategy

This document captures what we’ve learned about the structure of Phabricator’s Differential “changeset” webpage (as delivered via the AJAX endpoint), why the current code extracts data the way it does, and a proposal to robustly associate each inline comment with the correct suggestion diff.

## Data Sources
- API endpoints (token auth):
  - `differential.revision.search` → revision PHID.
  - `transaction.search` → transactions on a revision, including inline comments with fields: `path`, `line`, `length`, `diff.id`, `isDone`, and a `comments[].id` per inline.
- Web endpoint (cookie + CSRF auth):
  - `GET /D<id>` → initial HTML; contains `ref=<changeset_id>` hints and CSRF.
  - `POST /differential/changeset/` with `ref=<changeset_id>` and AJAX headers → JSON: `{ error, payload: { changeset: "<html>", ... } }`.
  - All JSON responses are prefixed with `for (;;);`.

## AJAX Response Structure
- Top-level JSON: `error`, `payload`.
- `payload.changeset` is an HTML fragment rendering a single file’s diff view. It contains:
  - A diff table: `<table class="differential-diff … diff-1up">` or `<table class="diff-1up-simple-table">`.
  - Row structure: `<tr>` with `<td class="left old">` and `<td class="right new">` cells (depending on mode), plus number columns `<td class="n">`.
  - Line anchors in the number columns as element ids:
    - `id="C<ref>OL<N>"` for old-line N, and `id="C<ref>NL<N>"` for new-line N.
    - Example: `C27068264NL18` with `data-n="18"`.
- Inline comments are embedded into this changeset fragment as discrete blocks:
  - An anchor preceding each inline comment block:
    - `<a name="inline-<comment_id>" id="inline-<comment_id>" class="differential-inline-comment-anchor"></a>`
  - The inline comment container immediately after the anchor:
    - `<div class="differential-inline-comment …" data-sigil="differential-inline-comment" …>`
    - May include `inline-is-done` if the comment is resolved.
    - A header area: `<div class="differential-inline-comment-head …" data-sigil="differential-inline-header">` that shows author, state, etc.
    - The content area: `<div class="differential-inline-comment-content">`.
    - If the inline contains a code suggestion, there is a nested block:
      - `<div class="inline-suggestion-view PhabricatorMonospaced">` containing a suggestion table `<table class="diff-1up-simple-table">` showing the suggested code as a diff-like table.

## Important Observations
- The suggestion table within `inline-suggestion-view` may or may not contain line anchors (OL/NL) inside the suggestion rows. When present, these anchors provide an exact range for the suggestion.
- Even when suggestion tables lack anchors, the surrounding changeset still contains anchors for nearby lines (in adjacent rows). The nearest anchors may not always precisely match the inline’s intended lines.
- The page may include many inline comment blocks and suggestion tables. Multiple blocks can be close in HTML position; naïve proximity-based mapping can mismatch.
- Every inline comment block has a stable anchor with the inline’s numeric id: `inline-<comment_id>`. This appears to match the `comments[].id` returned by the API for inline transactions.
  - Example seen: `<a id="inline-4481240" class="differential-inline-comment-anchor">`.
- Security prefix: all AJAX responses start with `for (;;);` and must be stripped before JSON parsing.

## Why the Current Code Works the Way It Does
- Dual-source extraction:
  - API is used to enumerate transactions and inline comments (who, when, file, line, length). The Phabricator API does not return rendered code suggestions.
  - Web scraping is used to fetch the rendered changeset HTML and extract suggestions from `inline-suggestion-view`.
- Authentication:
  - API token is sufficient for API calls.
  - Cookies (and CSRF) are required for the AJAX `changeset` endpoint. We auto-extract Firefox cookies or use `PHABRICATOR_COOKIES`.
- Changeset targeting:
  - The page uses `ref=<changeset_id>` to identify a file’s changeset. We collect `ref` ids from the main page and try them to get the right HTML fragment containing inline comments and suggestions.
- Suggestion extraction (today):
  - Try JSON paths for `suggestionText` as a fallback.
  - Parse `payload.changeset` HTML; search `inline-suggestion-view` tables and build a `+/-` diff from cells.
  - Heuristics based on nearest anchors and HTML proximity are used to associate a suggestion to the inline’s line range when the suggestion table lacks anchors.

## Pitfalls with Heuristics
- If multiple inline comments exist near each other and some suggestion tables lack explicit anchors, proximity/nearest-anchor scoring can assign the wrong suggestion to an inline.
- The changeset fragment can duplicate surrounding context, creating multiple close candidates with similar content.
- Without a deterministic link between the API inline record and its HTML block, scoring may still fail in clustered comment regions (e.g., lines 221/232 vs. 225–230 cases).

## Deterministic Anchor for Matching
- The changeset HTML embeds anchors named `inline-<comment_id>` immediately before the corresponding inline comment block.
- The API gives us each inline’s `comment.id`.
- Therefore, the most reliable mapping is:
  1. Given the inline’s `comment_id`, find `id="inline-<comment_id>"` in `payload.changeset`.
  2. Starting at that anchor, parse the adjacent `.differential-inline-comment` block.
  3. Inside that block, if a `.inline-suggestion-view` exists, parse its table to construct the suggestion diff.
  4. If no suggestion view exists, fall back to `suggestionText` if present, or leave the inline’s text as-is.

This avoids guessing by proximity or global nearest-line anchors.

## Proposal: Correct Matching Algorithm

1) Fetch changeset HTML for the file that contains the inline, as done today (extract refs, pick the right ref that contains inline comments for that file).

2) For each inline from the API (`TransactionData` where `type == "inline"`):
- Identify `comment_id` and the file `path`.
- In the corresponding `payload.changeset` for that file, locate the exact block via the unique anchor:
  - Search for `<a id="inline-{comment_id}" class="differential-inline-comment-anchor">`.
  - From that anchor, find the immediately following `<div class="differential-inline-comment …">` container.

3) Extract suggestion content deterministically:
- Within this container, look for `.inline-suggestion-view`.
- If found, parse the nested `<table class="diff-1up-simple-table">`:
  - Build `-` and `+` lines from rows by checking left/old vs right/new cells (`left old`, `right new`, or the simplified table without explicit old column where only `right new` is populated).
  - If present, prefer anchors within the suggestion table to compute exact ranges; however, range computation is optional for rendering the suggestion.

4) Fallbacks (in order):
- If `.inline-suggestion-view` is absent: check for JSON `suggestionText` within the same AJAX response and use that diff text.
- If neither is available, keep the API-provided inline comment text (some inlines are not code suggestions).

5) Handling DONE/resolved comments:
- Respect the presence of `inline-is-done` class on the container; filter unless `--include-done` is set (current behavior).

6) Handling missing/incorrect ref:
- If a file’s expected `inline-<comment_id>` anchor is not found in the chosen `ref`, try other `ref` ids captured from the main page for the same file.
- Cache the mapping from `path -> ref` when first found to reduce retries.

7) Performance and robustness tweaks:
- Strip `for (;;);` before parsing JSON.
- Decompress content if server uses gzip encoding (handled by reqwest).
- Keep extracting Firefox cookies and CSRF as implemented.

## Why This Proposal Fixes The Reported Mismatch
- The ambiguous cases (e.g., lines 221/232 vs 225–230) are caused by proximity-based scoring across several nearby inline comments and suggestion tables. The block-level anchor `inline-<comment_id>` provides an exact, deterministic location for the inline’s container. Parsing the suggestion inside that specific container eliminates cross-talk between adjacent comments.

## Implementation Notes
- We already carry `comment_id` in `InlineComment` (from the API). The changes needed are localized to suggestion extraction:
  - When calling `fetch_suggestion_from_web(...)` for a specific inline, include the `comment_id` and make the extractor search for `id="inline-{comment_id}"` in the `payload.changeset` HTML to select the right block.
  - Add a new method, e.g. `extract_suggestion_for_comment_id(html, comment_id, include_done) -> Option<String>`.
  - Only if this anchor is not found in the selected `ref`, fall back to the existing nearest-line approach, then JSON `suggestionText`.
- Keep the progress bar and existing cookie/CSRF logic.

## Open Questions / Variants
- Some Phabricator skins or modes may vary class names (`diff-2up`, different tables). We should pattern-match both 1up and 2up shapes.
- Some instances may omit the `differential-inline-comment-anchor` element, though it’s present in the observed payloads. We’ll retain our current heuristics as a last resort.
- For multi-line suggestions that do not include anchors, we will still render the suggestion correctly from the suggestion table; the exact numeric range is not required for output correctness.

---

By anchoring suggestion extraction to the inline’s unique HTML anchor `inline-<comment_id>`, we eliminate ambiguity and reliably associate each inline comment with the proper suggestion.
