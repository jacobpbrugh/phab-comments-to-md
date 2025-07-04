# phab-comments-to-md

Extract Phabricator review comments and format them as Markdown for further analysis by LLM agents.

## Installation

```bash
cargo build --release
```

## Usage

### Basic Usage

```bash
# Using full URL
./target/release/phab-comments-to-md --url https://phabricator.services.mozilla.com/D12345 --token your-api-token

# Using diff ID (defaults to Mozilla's Phabricator)
./target/release/phab-comments-to-md --diff-id 12345 --token your-api-token
./target/release/phab-comments-to-md --diff-id D12345 --token your-api-token  # 'D' prefix optional

# Using diff ID and custom base URL
./target/release/phab-comments-to-md --diff-id 12345 --base-url https://phabricator.example.com --token your-api-token

# Save to file
./target/release/phab-comments-to-md --diff-id 12345 --token your-api-token --output review.md

# Include "done" comments (useful for LLM verification)
./target/release/phab-comments-to-md --diff-id 12345 --token your-api-token --include-done
```

### Environment Variables

Set environment variables to simplify usage:

```bash
# Set both token and base URL (for non-Mozilla Phabricator instances)
export PHABRICATOR_TOKEN=your-api-token
export PHABRICATOR_BASE_URL=https://phabricator.example.com

# For Mozilla's Phabricator, only token is needed (base URL defaults to Mozilla's)
export PHABRICATOR_TOKEN=your-api-token

# Now you can use just the diff ID
./target/release/phab-comments-to-md --diff-id 12345
```

### Getting an API Token

1. Go to https://phabricator.services.mozilla.com/settings/user/\<username\>/page/apitokens/
2. Click "Generate API Token"
3. Give it a name and generate the token
4. Use the token with `--token` or set it as `PHABRICATOR_TOKEN` environment variable

## Options

```
Options:
  --url <URL>              Full Phabricator review URL
  --diff-id <DIFF_ID>      Differential revision ID (with or without 'D' prefix)
  --base-url <BASE_URL>    Base Phabricator URL (defaults to Mozilla's Phabricator)
  --token <TOKEN>          Phabricator API token (or set PHABRICATOR_TOKEN env var)
  --output <OUTPUT>        Output file path (defaults to stdout)
  --include-done           Include comments marked as "done" (useful for LLM verification)
  -h, --help              Print help
  -V, --version           Print version
```

**Environment Variables:**
- `PHABRICATOR_TOKEN` - API token (avoids passing token on command line)
- `PHABRICATOR_BASE_URL` - Base URL (for non-Mozilla Phabricator instances)

You must provide either `--url` OR `--diff-id`. When using `--diff-id`, the base URL defaults to Mozilla's Phabricator.

## Output Format

The tool generates Markdown with:
- General comments sorted chronologically
- Inline comments grouped by file and sorted chronologically

Comments marked as "done" are automatically filtered out to focus on active
discussion. Use `--include-done` to include them with clear [DONE] markers for
LLM verification of addressed feedback.

## License

Licensed under either of

* Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
* MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.
