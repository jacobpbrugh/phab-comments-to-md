# Project Debug Rules (Non-Obvious Only)

- Firefox database lock errors require copying `cookies.sqlite` to temp file for debugging
- AJAX responses contain `for (;;);` prefix that must be stripped before JSON parsing - check raw response first
- Suggestion extraction failures often due to missing CSRF tokens - verify cookie authentication is working
- Progress bar animation stops if `enable_steady_tick()` not called - required for long-running operations
- User cache misses indicate PHID resolution issues - check API token permissions
- Empty inline comments usually contain JavaScript-rendered suggestions not accessible via API
- Cross-platform cookie path detection failures: check Firefox profile directory permissions and existence
