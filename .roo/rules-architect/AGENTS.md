# Project Architecture Rules (Non-Obvious Only)

- Dual authentication architecture: API tokens for Phabricator API + Firefox cookies for web scraping JavaScript-rendered content
- Cross-platform Firefox integration: Direct SQLite database access with platform-specific profile paths and lock handling
- Hybrid extraction strategy: API calls for basic data + authenticated web scraping for suggestions that require JavaScript rendering
- Progress tracking architecture: Uses `indicatif` with steady tick animation for long-running async operations
- User caching layer: In-memory PHID-to-username cache prevents redundant API calls within single execution
- Comment filtering architecture: Default excludes "done" comments, optional inclusion with explicit markers
- Response parsing pipeline: Strips Phabricator security prefix `for (;;);` before JSON parsing
