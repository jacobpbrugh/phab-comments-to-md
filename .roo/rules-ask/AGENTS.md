# Project Documentation Rules (Non-Obvious Only)

- `test_suggestion.rs` is a standalone debugging script, not a proper test file - shows manual JSON parsing approach
- Dual authentication system (API + cookies) is not obvious from CLI help - both are required for full functionality
- Firefox cookie extraction works across platforms but uses different profile paths on each OS
- AJAX responses from Phabricator have security prefix `for (;;);` that must be stripped before parsing
- Suggestion extraction prioritizes different HTML elements in specific order based on content quality
- Progress bar implementation uses `indicatif` with steady tick animation for long-running async operations
- User cache prevents redundant API calls but is not persisted between runs
