# Project Coding Rules (Non-Obvious Only)

- Firefox cookie extraction requires handling locked databases - always use temp copies when `database is locked` error occurs
- AJAX responses from Phabricator start with `for (;;);` security prefix - must strip before JSON parsing
- Suggestion extraction has priority order: `suggestionText` > `inline-suggestion-view` > `differential-inline-comment`
- Cross-platform Firefox profile paths: Windows uses `%APPDATA%/Mozilla/Firefox/Profiles`, macOS uses `~/Library/Application Support/Firefox/Profiles`, Linux uses `~/.mozilla/firefox`
- Progress bars require `enable_steady_tick()` for animation during async operations
- User cache in `PhabricatorCommentExtractor` prevents redundant API calls - always check cache first
- Inline comments marked as "done" are filtered by default - use `include_done` parameter to override
