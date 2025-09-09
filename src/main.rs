// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use anyhow::{Context, Result};
use chrono::DateTime;
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use log::{debug, error, info, trace, warn};
use regex::Regex;
use reqwest::Client;
use rusqlite::{Connection, OpenFlags};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use url::Url;

#[derive(Parser, Debug)]
#[command(
    author = "Mozilla",
    version = "0.1.0",
    about = "Extract Phabricator review comments and format them as Markdown",
    long_about = "Extract comments from Phabricator Differential reviews and format them as Markdown for analysis by LLM agents. Comments are sorted chronologically for natural reading flow."
)]
struct Args {
    /// Phabricator URL (e.g., https://phabricator.services.mozilla.com/D12345)
    #[arg(long, help = "Full Phabricator review URL")]
    url: Option<String>,

    /// Differential revision ID (with or without 'D' prefix)
    #[arg(
        long,
        help = "Differential revision ID (with or without 'D' prefix, use with --base-url or PHABRICATOR_BASE_URL)"
    )]
    diff_id: Option<String>,

    /// Base Phabricator URL (can also be set via PHABRICATOR_BASE_URL env var)
    #[arg(
        long,
        help = "Base Phabricator URL (defaults to Mozilla's Phabricator, or set PHABRICATOR_BASE_URL env var)"
    )]
    base_url: Option<String>,

    /// Phabricator API token (can also be set via PHABRICATOR_TOKEN env var)
    #[arg(
        long,
        help = "Phabricator API token (or set PHABRICATOR_TOKEN env var)"
    )]
    token: Option<String>,

    /// Output file path (optional, defaults to stdout)
    #[arg(long, help = "Output file path (defaults to stdout)")]
    output: Option<String>,

    /// Include comments marked as "done" (marked as [DONE] in output)
    #[arg(
        long,
        help = "Include comments marked as 'done' (useful for LLM verification of addressed feedback)"
    )]
    include_done: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct UserSearchResult {
    #[serde(rename = "error_code")]
    error_code: Option<String>,
    #[serde(rename = "error_info")]
    error_info: Option<String>,
    result: Option<UserSearchData>,
}

#[derive(Debug, Serialize, Deserialize)]
struct UserSearchData {
    data: Vec<UserData>,
}

#[derive(Debug, Serialize, Deserialize)]
struct UserData {
    fields: UserFields,
}

#[derive(Debug, Serialize, Deserialize)]
struct UserFields {
    #[serde(rename = "realName")]
    real_name: Option<String>,
    username: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RevisionSearchResult {
    #[serde(rename = "error_code")]
    error_code: Option<String>,
    #[serde(rename = "error_info")]
    error_info: Option<String>,
    result: Option<RevisionSearchData>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RevisionSearchData {
    data: Vec<RevisionData>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RevisionData {
    phid: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct TransactionSearchResult {
    #[serde(rename = "error_code")]
    error_code: Option<String>,
    #[serde(rename = "error_info")]
    error_info: Option<String>,
    result: Option<TransactionSearchData>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TransactionSearchData {
    data: Vec<TransactionData>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TransactionData {
    #[serde(rename = "type")]
    transaction_type: Option<String>,
    #[serde(rename = "authorPHID")]
    author_phid: Option<String>,
    #[serde(rename = "dateCreated")]
    date_created: u64,
    comments: Vec<CommentData>,
    fields: Option<serde_json::Value>,
    id: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct CommentData {
    content: CommentContent,
    id: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct CommentContent {
    raw: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct Comment {
    author: String,
    author_phid: String,
    date: String,
    date_timestamp: u64,
    content: String,
    transaction_id: String,
    comment_id: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct InlineComment {
    author: String,
    author_phid: String,
    date: String,
    date_timestamp: u64,
    content: String,
    file_path: String,
    line_number: u32,
    line_length: u32,
    diff_id: String,
    is_done: bool,
    transaction_id: String,
    comment_id: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct ReviewAction {
    author: String,
    author_phid: String,
    date: String,
    action: String,
    comments: Vec<String>,
    transaction_id: String,
}

#[derive(Debug)]
struct CommentsData {
    general_comments: Vec<Comment>,
    inline_comments: Vec<InlineComment>,
    review_actions: Vec<ReviewAction>,
}

struct PhabricatorCommentExtractor {
    base_url: String,
    api_token: String,
    client: Client,
    user_cache: HashMap<String, String>,
    current_revision_id: Option<u32>,
}

#[allow(dead_code)]
impl PhabricatorCommentExtractor {
    fn new(base_url: String, api_token: String) -> Self {
        let client = Client::builder()
            .user_agent(
                "phab-comments-to-md/0.1.0 (https://github.com/padenot/phab-comments-to-md)",
            )
            .build()
            .expect("Failed to build HTTP client");

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_token,
            client,
            user_cache: HashMap::new(),
            current_revision_id: None,
        }
    }

    async fn extract_firefox_cookies(&self, domain: &str) -> Result<HashMap<String, String>> {
        // Try environment variable first for manual cookie specification
        if let Ok(cookie_env) = std::env::var("PHABRICATOR_COOKIES") {
            let mut cookies = HashMap::new();
            for cookie_pair in cookie_env.split(';') {
                let cookie_pair = cookie_pair.trim();
                if let Some((name, value)) = cookie_pair.split_once('=') {
                    cookies.insert(name.trim().to_string(), value.trim().to_string());
                }
            }
            if cookies.contains_key("phsid") && cookies.contains_key("phusr") {
                return Ok(cookies);
            }
        }

        // Extract cookies directly from Firefox SQLite database
        self.extract_cookies_from_firefox_db(domain).await
    }

    async fn extract_cookies_from_firefox_db(
        &self,
        domain: &str,
    ) -> Result<HashMap<String, String>> {
        let profile_dir = self.find_firefox_profile_dir(domain).await?;
        let cookies_db_path = profile_dir.join("cookies.sqlite");

        if !cookies_db_path.exists() {
            anyhow::bail!(
                "Firefox cookies database not found at: {}",
                cookies_db_path.display()
            );
        }

        // Open database in immutable mode (handle locked databases)
        let (conn, temp_db) =
            match Connection::open_with_flags(&cookies_db_path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
                Ok(conn) => (conn, None),
                Err(e) if e.to_string().contains("database is locked") => {
                    let temp_db = std::env::temp_dir()
                        .join(format!("cookies_extract_{}.sqlite", std::process::id()));
                    std::fs::copy(&cookies_db_path, &temp_db)?;
                    let conn =
                        Connection::open_with_flags(&temp_db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
                    (conn, Some(temp_db))
                }
                Err(e) => return Err(e.into()),
            };

        let mut stmt =
            conn.prepare("SELECT host, name, value FROM moz_cookies WHERE host LIKE ?1")?;

        let domain_pattern = format!("%{}%", domain);
        let cookie_iter = stmt.query_map([&domain_pattern], |row| {
            Ok((
                row.get::<_, String>(1)?, // name
                row.get::<_, String>(2)?, // value
            ))
        })?;

        let mut cookies = HashMap::new();
        for cookie_result in cookie_iter {
            let (name, value) = cookie_result?;
            cookies.insert(name, value);
        }

        // Ensure we have the required cookies
        if !cookies.contains_key("phsid") || !cookies.contains_key("phusr") {
            anyhow::bail!(
                "Required cookies (phsid, phusr) not found for domain: {}. Found cookies: {:?}",
                domain,
                cookies.keys().collect::<Vec<_>>()
            );
        }

        // Clean up temporary database if used
        if let Some(temp_path) = temp_db {
            let _ = std::fs::remove_file(temp_path);
        }

        Ok(cookies)
    }

    async fn find_firefox_profile_dir(&self, domain: &str) -> Result<std::path::PathBuf> {
        let firefox_dir = if cfg!(target_os = "windows") {
            dirs::config_dir()
                .ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?
                .join("Mozilla")
                .join("Firefox")
                .join("Profiles")
        } else if cfg!(target_os = "macos") {
            dirs::home_dir()
                .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
                .join("Library")
                .join("Application Support")
                .join("Firefox")
                .join("Profiles")
        } else {
            // Linux and other Unix-like systems
            dirs::home_dir()
                .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
                .join(".mozilla")
                .join("firefox")
        };

        if !firefox_dir.exists() {
            anyhow::bail!("Firefox directory not found: {}", firefox_dir.display());
        }

        // Find all profile directories
        let mut profiles = Vec::new();
        for entry in std::fs::read_dir(&firefox_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let cookies_db = path.join("cookies.sqlite");
                if cookies_db.exists() {
                    if let Ok(metadata) = cookies_db.metadata() {
                        if let Ok(modified) = metadata.modified() {
                            profiles.push((path, modified));
                        }
                    }
                }
            }
        }

        if profiles.is_empty() {
            anyhow::bail!(
                "No Firefox profiles with cookies.sqlite found in: {}",
                firefox_dir.display()
            );
        }

        // Sort by modification time (most recent first)
        profiles.sort_by(|a, b| b.1.cmp(&a.1));

        // Take the most recent profile that has the required cookies
        let domain_pattern = format!("%{}%", domain);

        for (profile_path, _modified) in profiles {
            let cookies_db = profile_path.join("cookies.sqlite");

            // Try to read cookies from this profile
            match Connection::open_with_flags(&cookies_db, OpenFlags::SQLITE_OPEN_READ_ONLY) {
                Ok(conn) => {
                    // Check if we can find the required cookies
                    if let Ok(mut stmt) = conn.prepare(
                        "SELECT COUNT(*) FROM moz_cookies WHERE host LIKE ?1 AND (name = 'phsid' OR name = 'phusr')"
                    ) {
                        if let Ok(count) = stmt.query_row([&domain_pattern], |row| row.get::<_, i32>(0)) {
                            if count >= 2 {
                                return Ok(profile_path);
                            }
                        }
                    }
                }
                Err(e) => {
                    // Check if it's a database lock error
                    if e.to_string().contains("database is locked")
                        || e.to_string().contains("database disk image is malformed")
                    {
                        // Try to copy the database and read from the copy
                        let temp_db = std::env::temp_dir()
                            .join(format!("cookies_temp_{}.sqlite", std::process::id()));
                        if let Ok(_) = std::fs::copy(&cookies_db, &temp_db) {
                            if let Ok(conn) = Connection::open_with_flags(
                                &temp_db,
                                OpenFlags::SQLITE_OPEN_READ_ONLY,
                            ) {
                                if let Ok(mut stmt) = conn.prepare(
                                    "SELECT COUNT(*) FROM moz_cookies WHERE host LIKE ?1 AND (name = 'phsid' OR name = 'phusr')"
                                ) {
                                    if let Ok(count) = stmt.query_row([&domain_pattern], |row| row.get::<_, i32>(0)) {
                                        if count >= 2 {
                                            // Clean up temp file
                                            let _ = std::fs::remove_file(&temp_db);
                                            return Ok(profile_path);
                                        }
                                    }
                                }
                            }
                            // Clean up temp file
                            let _ = std::fs::remove_file(&temp_db);
                        }
                    }
                }
            }
        }

        anyhow::bail!("No Firefox profile found with required Phabricator cookies")
    }

    async fn get_csrf_token_with_cookies(&self, revision_id: u32, domain: &str) -> Option<String> {
        let url = format!("{}/D{}", self.base_url, revision_id);
        let mut request_builder = self.client.get(&url);

        // Add Firefox cookies for authentication
        if let Ok(cookies) = self.extract_firefox_cookies(domain).await {
            let mut cookie_string = String::new();
            for (name, value) in cookies {
                if !cookie_string.is_empty() {
                    cookie_string.push_str("; ");
                }
                cookie_string.push_str(&format!("{}={}", name, value));
            }
            if !cookie_string.is_empty() {
                request_builder = request_builder.header("Cookie", cookie_string);
            }
        }

        if let Ok(response) = request_builder.send().await {
            if let Ok(html) = response.text().await {
                // Look for CSRF token in the HTML
                let csrf_re = regex::Regex::new(r#"__csrf__.*?value="([^"]+)""#).unwrap();
                if let Some(captures) = csrf_re.captures(&html) {
                    return Some(captures.get(1)?.as_str().to_string());
                }

                // Alternative pattern
                let csrf_re2 = regex::Regex::new(r#""current":"([^"]+)""#).unwrap();
                if let Some(captures) = csrf_re2.captures(&html) {
                    return Some(captures.get(1)?.as_str().to_string());
                }
            }
        }
        None
    }

    /// Fetches JavaScript-rendered suggestions from Phabricator web interface
    /// using authenticated AJAX requests with extracted ref parameters
    async fn fetch_suggestion_from_web(
        &self,
        revision_id: u32,
        line_number: u32,
        file_path: &str,
        include_done: bool,
    ) -> Option<String> {
        if let Some(changeset_data) = self.fetch_changeset_data(revision_id).await {
            if let Some(suggestions) = self
                .parse_suggestions_from_ajax(&changeset_data, line_number, file_path, include_done)
                .await
            {
                return Some(suggestions);
            }
        }
        None
    }

    async fn get_changeset_ids(&self, revision_id: u32) -> Vec<String> {
        // First get the changeset IDs from the differential API
        let revision_phid = match self.get_revision_phid(revision_id).await {
            Ok(phid) => phid,
            Err(_) => return Vec::new(),
        };

        // Get transactions to find diff information
        let transactions = match self.get_transactions(&revision_phid).await {
            Ok(trans) => trans,
            Err(_) => return Vec::new(),
        };

        let mut changeset_ids = Vec::new();

        // Look for differential diff information in transactions
        for transaction in transactions {
            if let Some(fields) = transaction.fields {
                // Check for diff field that might contain changeset information
                if let Some(diff_field) = fields.get("diff") {
                    if let Some(diff_obj) = diff_field.as_object() {
                        if let Some(id) = diff_obj.get("id") {
                            if let Some(id_str) = id.as_str() {
                                changeset_ids.push(id_str.to_string());
                            } else if let Some(id_num) = id.as_u64() {
                                changeset_ids.push(id_num.to_string());
                            }
                        }
                    }
                }
            }
        }

        // If we didn't find changeset IDs in transactions, try the direct diff API
        if changeset_ids.is_empty() {
            if let Some(diff_id) = self.get_latest_diff_id(revision_id).await {
                changeset_ids.push(diff_id);
            }
        }

        changeset_ids
    }

    async fn get_latest_diff_id(&self, revision_id: u32) -> Option<String> {
        // Try to get the latest diff ID by searching for diffs of this revision
        let url = format!("{}/api/differential.diff.search", self.base_url);
        let params = [
            ("api.token", self.api_token.as_str()),
            ("constraints[revisionIDs][0]", &revision_id.to_string()),
            ("order", "newest"),
            ("limit", "1"),
        ];

        match self.client.post(&url).form(&params).send().await {
            Ok(response) => {
                if let Ok(result) = response.json::<serde_json::Value>().await {
                    if let Some(data) = result
                        .get("result")
                        .and_then(|r| r.get("data"))
                        .and_then(|d| d.as_array())
                    {
                        if let Some(first_diff) = data.first() {
                            if let Some(diff_id) = first_diff.get("id") {
                                if let Some(id_str) = diff_id.as_str() {
                                    return Some(id_str.to_string());
                                } else if let Some(id_num) = diff_id.as_u64() {
                                    return Some(id_num.to_string());
                                }
                            }
                        }
                    }
                }
            }
            Err(_) => {}
        }
        None
    }

    /// Extracts ref parameters from Phabricator revision page HTML for AJAX requests
    async fn extract_ref_parameters_from_page(&self, revision_id: u32) -> Vec<String> {
        let url = format!("{}/D{}", self.base_url, revision_id);

        // Try to extract Firefox cookies for authentication
        let domain = if let Ok(parsed_url) = Url::parse(&self.base_url) {
            parsed_url
                .host_str()
                .unwrap_or("phabricator.services.mozilla.com")
                .to_string()
        } else {
            "phabricator.services.mozilla.com".to_string()
        };

        let mut request_builder = self.client.get(&url);

        // Add Firefox cookies if available
        if let Ok(cookies) = self.extract_firefox_cookies(&domain).await {
            if !cookies.is_empty() {
                let mut cookie_string = String::new();
                for (name, value) in cookies {
                    if !cookie_string.is_empty() {
                        cookie_string.push_str("; ");
                    }
                    cookie_string.push_str(&format!("{}={}", name, value));
                }
                request_builder = request_builder.header("Cookie", cookie_string);
            }
        }

        match request_builder.send().await {
            Ok(response) => {
                if let Ok(html) = response.text().await {
                    // Extract all ref parameters from the HTML using regex
                    let re = regex::Regex::new(r#"ref=(\d+)"#).unwrap();
                    let mut refs = Vec::new();

                    for captures in re.captures_iter(&html) {
                        if let Some(ref_match) = captures.get(1) {
                            let ref_value = ref_match.as_str().to_string();
                            if !refs.contains(&ref_value) {
                                refs.push(ref_value);
                            }
                        }
                    }

                    // If no refs found with simple pattern, try more comprehensive search
                    if refs.is_empty() {
                        // Try looking for refs in various JavaScript formats
                        let patterns = vec![
                            r#""ref":"(\d+)""#,        // JSON: "ref":"123456"
                            r#"'ref':\s*'(\d+)'"#,     // JS: 'ref': '123456'
                            r#"ref:\s*'(\d+)'"#,       // JS: ref: '123456'
                            r#"ref:\s*(\d+)"#,         // JS: ref: 123456
                            r#"\bC(\d{7,8})[ON]L\d+"#, // HTML IDs like C8450617OL1, C8450617NL1
                        ];

                        for pattern in patterns {
                            let re = regex::Regex::new(pattern).unwrap();
                            for captures in re.captures_iter(&html) {
                                if let Some(ref_match) = captures.get(1) {
                                    let ref_value = ref_match.as_str().to_string();
                                    if !refs.contains(&ref_value) && ref_value.len() >= 7 {
                                        refs.push(ref_value);
                                    }
                                }
                            }
                        }

                        // Look for changeset IDs in JavaScript data or JSON (7-8 digit numbers)
                        if refs.is_empty() {
                            let js_re = regex::Regex::new(r"\b\d{7,8}\b").unwrap();
                            for js_match in js_re.find_iter(&html) {
                                let ref_value = js_match.as_str().to_string();
                                if !refs.contains(&ref_value) {
                                    refs.push(ref_value);
                                }
                            }
                        }

                        // Try to find them in differential/ URLs specifically
                        let diff_re =
                            regex::Regex::new(r"differential/changeset/[^?]*\?[^&]*ref=(\d+)")
                                .unwrap();
                        for captures in diff_re.captures_iter(&html) {
                            if let Some(ref_match) = captures.get(1) {
                                let ref_value = ref_match.as_str().to_string();
                                if !refs.contains(&ref_value) {
                                    refs.push(ref_value);
                                }
                            }
                        }
                    }

                    refs
                } else {
                    Vec::new()
                }
            }
            Err(_) => Vec::new(),
        }
    }

    async fn fetch_changeset_with_refs(
        &self,
        revision_id: u32,
        ref_params: &[String],
    ) -> Option<String> {
        // Get domain for Firefox cookies
        let domain = if let Ok(parsed_url) = Url::parse(&self.base_url) {
            parsed_url
                .host_str()
                .unwrap_or("phabricator.services.mozilla.com")
                .to_string()
        } else {
            "phabricator.services.mozilla.com".to_string()
        };

        // Get CSRF token first (this also needs cookies)
        let csrf_token = self
            .get_csrf_token_with_cookies(revision_id, &domain)
            .await
            .unwrap_or_else(|| "dummy".to_string());

        // Set up the AJAX request similar to the curl command
        let changeset_url = format!("{}/differential/changeset/", self.base_url);

        let headers = [
            (
                "User-Agent",
                "Mozilla/5.0 (X11; Linux x86_64; rv:142.0) Gecko/20100101 Firefox/142.0",
            ),
            ("Accept", "*/*"),
            ("Accept-Language", "en-US,en;q=0.5"),
            ("Accept-Encoding", "gzip, deflate, br"),
            ("X-Phabricator-Csrf", &csrf_token),
            ("X-Phabricator-Via", &format!("/D{}", revision_id)),
            ("Content-Type", "application/x-www-form-urlencoded"),
            ("Origin", &self.base_url),
            ("Connection", "keep-alive"),
            ("Sec-Fetch-Dest", "empty"),
            ("Sec-Fetch-Mode", "cors"),
            ("Sec-Fetch-Site", "same-origin"),
        ];

        // Try each ref parameter and prioritize those with suggestionText
        let mut best_response = None;
        let mut best_score = 0;

        for ref_param in ref_params {
            let form_data = [
                ("ref", ref_param.as_str()),
                ("device", "1up"),
                ("__wflow__", "true"),
                ("__ajax__", "true"),
                ("__metablock__", "7"),
            ];

            let mut request = self.client.post(&changeset_url);
            for (key, value) in headers.iter() {
                request = request.header(*key, *value);
            }

            // Add Firefox cookies for authentication
            if let Ok(cookies) = self.extract_firefox_cookies(&domain).await {
                let mut cookie_string = String::new();
                for (name, value) in cookies {
                    if !cookie_string.is_empty() {
                        cookie_string.push_str("; ");
                    }
                    cookie_string.push_str(&format!("{}={}", name, value));
                }
                if !cookie_string.is_empty() {
                    request = request.header("Cookie", cookie_string);
                }
            }

            match request.form(&form_data).send().await {
                Ok(response) => {
                    if let Ok(text) = response.text().await {
                        // Check if this response contains suggestions and score it
                        let has_suggestion_text = text.contains("suggestionText");
                        let has_inline_view = text.contains("inline-suggestion-view");
                        let has_inline_comment = text.contains("differential-inline-comment");

                        // Score responses: suggestionText > inline-suggestion-view > differential-inline-comment
                        let mut score = 0;
                        if has_suggestion_text {
                            score += 100;
                        }
                        if has_inline_view {
                            score += 10;
                        }
                        if has_inline_comment {
                            score += 1;
                        }

                        if score > best_score {
                            best_score = score;
                            best_response = Some(text);
                        }
                    }
                }
                Err(_) => continue,
            }
        }

        if let Some(response) = best_response {
            return Some(response);
        }

        None
    }

    async fn fetch_changeset_data(&self, revision_id: u32) -> Option<String> {
        // First try to extract ref parameters from the initial page
        let ref_params = self.extract_ref_parameters_from_page(revision_id).await;

        if !ref_params.is_empty() {
            // Use the extracted ref parameters directly
            if let Some(result) = self
                .fetch_changeset_with_refs(revision_id, &ref_params)
                .await
            {
                return Some(result);
            }
        }

        // Fallback: Get the actual changeset IDs the old way
        let changeset_ids = self.get_changeset_ids(revision_id).await;

        if changeset_ids.is_empty() {
            return None;
        }

        // Get CSRF token first
        let csrf_token = self
            .get_csrf_token(revision_id)
            .await
            .unwrap_or_else(|| "dummy".to_string());

        // Set up the AJAX request similar to the curl command
        let changeset_url = format!("{}/differential/changeset/", self.base_url);

        let headers = [
            (
                "User-Agent",
                "Mozilla/5.0 (X11; Linux x86_64; rv:142.0) Gecko/20100101 Firefox/142.0",
            ),
            ("Accept", "*/*"),
            ("Accept-Language", "en-US,en;q=0.5"),
            ("Accept-Encoding", "gzip, deflate, br"),
            ("X-Phabricator-Csrf", &csrf_token),
            ("X-Phabricator-Via", &format!("/D{}", revision_id)),
            ("Content-Type", "application/x-www-form-urlencoded"),
            ("Origin", &self.base_url),
            ("Connection", "keep-alive"),
            ("Sec-Fetch-Dest", "empty"),
            ("Sec-Fetch-Mode", "cors"),
            ("Sec-Fetch-Site", "same-origin"),
        ];

        // Try each changeset ID until we find one with suggestions
        for changeset_id in changeset_ids {
            // Try to get changeset data for each specific file that might contain suggestions
            let result = self
                .try_fetch_specific_changeset(&changeset_url, &headers, &changeset_id)
                .await;
            if result.is_some() {
                return result;
            }

            // Note: The proper solution would be to extract ref values from the HTML page
            // but this requires session authentication (cookies), not API tokens.
            // For now, we limit our attempts to the API-provided changeset IDs.
        }

        // If no specific changeset worked, try to find file-specific changesets
        if let Some(result) = self
            .try_fetch_file_specific_changeset(revision_id, &changeset_url, &headers)
            .await
        {
            return Some(result);
        }

        None
    }

    async fn try_fetch_specific_changeset(
        &self,
        changeset_url: &str,
        headers: &[(&str, &str); 12],
        changeset_id: &str,
    ) -> Option<String> {
        let form_data = [
            ("ref", changeset_id),
            ("device", "2up"),
            ("__wflow__", "true"),
            ("__ajax__", "true"),
            ("__metablock__", "2"),
        ];

        let mut request = self.client.post(changeset_url);
        for (key, value) in headers.iter() {
            request = request.header(*key, *value);
        }

        match request.form(&form_data).send().await {
            Ok(response) => {
                match response.text().await {
                    Ok(text) => {
                        // Check if this response contains suggestions or meaningful diff content
                        if text.contains("inline-suggestion-view")
                            || text.contains("suggestionText")
                            || (text.len() > 1000 && text.contains("differential-diff"))
                        {
                            return Some(text);
                        }
                    }
                    Err(_) => {}
                }
            }
            Err(_) => {}
        }
        None
    }

    async fn try_fetch_file_specific_changeset(
        &self,
        revision_id: u32,
        changeset_url: &str,
        headers: &[(&str, &str); 12],
    ) -> Option<String> {
        // Try some variations of changeset IDs
        let potential_refs = vec![
            format!("{}", revision_id),
            format!("{}", revision_id + 1),
            format!("{}", revision_id + 2),
            format!("{}", revision_id - 1),
            format!("{}", revision_id - 2),
        ];

        for ref_id in potential_refs {
            let form_data = [
                ("ref", ref_id.as_str()),
                ("device", "2up"),
                ("__wflow__", "true"),
                ("__ajax__", "true"),
                ("__metablock__", "2"),
            ];

            let mut request = self.client.post(changeset_url);
            for (key, value) in headers.iter() {
                request = request.header(*key, *value);
            }

            match request.form(&form_data).send().await {
                Ok(response) => {
                    match response.text().await {
                        Ok(text) => {
                            // Check for suggestions in general
                            if text.contains("inline-suggestion-view")
                                || text.contains("suggestionText")
                            {
                                return Some(text);
                            }
                        }
                        Err(_) => {}
                    }
                }
                Err(_) => {}
            }
        }
        None
    }

    async fn parse_suggestions_from_ajax(
        &self,
        ajax_response: &str,
        line_number: u32,
        file_path: &str,
        include_done: bool,
    ) -> Option<String> {
        // First try to extract inline suggestion content directly from HTML (shows proper diff)
        if ajax_response.contains("inline-suggestion-view") {
            // Parse the HTML and extract the suggestion content
            let mut response = ajax_response;
            if response.starts_with("for (;;);") {
                response = &response[9..];
            }

            if let Ok(data) = serde_json::from_str::<serde_json::Value>(response) {
                if let Some(payload) = data.get("payload") {
                    if let Some(changeset_html) = payload.get("changeset") {
                        if let Some(html_str) = changeset_html.as_str() {
                            // Extract suggestion from the inline-suggestion-view
                            debug!("Extracting inline suggestion from HTML");
                            if let Some(suggestion) = self.extract_inline_suggestion(html_str) {
                                info!("Successfully extracted inline suggestion");
                                return Some(suggestion);
                            }
                        }
                    }
                }
            }
        }

        // Fallback: try to extract suggestions from JSON format (may only show final state)
        debug!("Attempting to extract suggestion from JSON format");
        if let Some(suggestion) = self.extract_suggestion_from_json(ajax_response) {
            info!("Successfully extracted suggestion from JSON");
            return Some(suggestion);
        }

        // Try to extract diff content from the changeset
        if let Some(diff_content) = self.extract_diff_from_changeset(ajax_response) {
            return Some(format!(
                "**Code changes:**\n\n```diff\n{}\n```",
                diff_content
            ));
        }

        let mut response = ajax_response;

        // The AJAX response starts with for (;;); followed by JSON
        if response.starts_with("for (;;);") {
            response = &response[9..]; // Remove the for (;;); prefix
        }

        match serde_json::from_str::<serde_json::Value>(response) {
            Ok(data) => {
                trace!("Successfully parsed AJAX response JSON");
                // Look for HTML content in the JSON response
                if let Some(payload) = data.get("payload") {
                    if let Some(changeset_html) = payload.get("changeset") {
                        if let Some(html_str) = changeset_html.as_str() {
                            // Parse HTML for suggestions
                            let document = Html::parse_document(html_str);
                            return self
                                .find_suggestions_in_html(
                                    &document,
                                    line_number,
                                    file_path,
                                    include_done,
                                )
                                .await;
                        }
                    }
                }
            }
            Err(e) => {
                debug!("Response is not JSON ({}), treating as HTML", e);
                // Not JSON, treat as HTML
                let document = Html::parse_document(response);
                return self
                    .find_suggestions_in_html(&document, line_number, file_path, include_done)
                    .await;
            }
        }

        None
    }

    fn extract_suggestion_from_json(&self, json_response: &str) -> Option<String> {
        // Strip the "for (;;);" prefix that Phabricator adds for security
        let clean_json = if json_response.starts_with("for (;;);") {
            &json_response[9..]
        } else {
            json_response
        };

        // Parse the JSON response to extract suggestionText
        match serde_json::from_str::<serde_json::Value>(clean_json) {
            Ok(json) => {
                trace!("Successfully parsed suggestion JSON");
                // Look for suggestionText in the JSON structure
                if let Some(suggestion_text) = self.find_suggestion_text_recursive(&json) {
                    if !suggestion_text.trim().is_empty() {
                        return Some(format!(
                            "**Suggested changes:**\n\n```diff\n{}\n```",
                            suggestion_text.trim()
                        ));
                    }
                }
            }
            Err(e) => {
                debug!("Failed to parse suggestion JSON: {}", e);
            }
        }

        // Fallback: try to extract suggestionText using regex that handles escaped content
        let re = regex::Regex::new(r#""suggestionText":"((?:[^"\\]|\\.)*)""#).unwrap();
        if let Some(captures) = re.captures(json_response) {
            if let Some(suggestion_match) = captures.get(1) {
                let suggestion_text = suggestion_match
                    .as_str()
                    .replace("\\n", "\n")
                    .replace("\\t", "\t")
                    .replace("\\u003e", ">")
                    .replace("\\u003c", "<")
                    .replace("\\/", "/")
                    .replace("\\\"", "\"")
                    .replace("\\\\", "\\");

                if !suggestion_text.trim().is_empty() {
                    return Some(format!(
                        "**Suggested changes:**\n\n```diff\n{}\n```",
                        suggestion_text.trim()
                    ));
                }
            }
        } else {
            debug!("No suggestionText found using regex");
        }

        None
    }

    fn find_suggestion_text_recursive(&self, value: &serde_json::Value) -> Option<String> {
        match value {
            serde_json::Value::Object(map) => {
                // Check if this object has suggestionText
                if let Some(suggestion_text) = map.get("suggestionText") {
                    if let Some(text) = suggestion_text.as_str() {
                        if !text.trim().is_empty() {
                            // Look for suggestions that contain actual changes
                            if text.contains("uuuu") || text.contains("-") || text.contains("+") {
                                return Some(text.to_string());
                            }
                        }
                    }
                }

                // Recursively search in all object values
                for (_key, val) in map {
                    if let Some(result) = self.find_suggestion_text_recursive(val) {
                        return Some(result);
                    }
                }
            }
            serde_json::Value::Array(arr) => {
                // Recursively search in all array elements
                for val in arr {
                    if let Some(result) = self.find_suggestion_text_recursive(val) {
                        return Some(result);
                    }
                }
            }
            _ => {}
        }
        None
    }

    async fn find_suggestions_in_html(
        &self,
        document: &Html,
        _line_number: u32,
        _file_path: &str,
        include_done: bool,
    ) -> Option<String> {
        // Look for inline-suggestion-view elements
        if let Ok(suggestion_selector) = Selector::parse(".inline-suggestion-view") {
            let suggestions: Vec<_> = document.select(&suggestion_selector).collect();

            // Extract suggestions from available suggestion elements
            for suggestion in suggestions.iter() {
                // Check if this suggestion is marked as "done"
                let is_done = self.is_suggestion_done(suggestion);
                if is_done && !include_done {
                    continue;
                }

                // Extract the suggestion content from the table structure
                if let Some(suggestion_text) = self.extract_suggestion_from_table(suggestion) {
                    return Some(suggestion_text);
                }
            }
        }

        None
    }

    fn extract_diff_from_changeset(&self, ajax_response: &str) -> Option<String> {
        let mut response = ajax_response;

        // Remove the for (;;); prefix if present
        if response.starts_with("for (;;);") {
            response = &response[9..];
        }

        // Parse JSON response
        match serde_json::from_str::<serde_json::Value>(response) {
            Ok(data) => {
                trace!("Successfully parsed changeset diff JSON");
                if let Some(payload) = data.get("payload") {
                    if let Some(changeset_html) = payload.get("changeset") {
                        if let Some(html_str) = changeset_html.as_str() {
                            // Parse HTML and extract diff content
                            debug!("Parsing HTML for diff content extraction");
                            let document = Html::parse_document(html_str);

                            // Look for diff rows that show changes
                            if let Ok(diff_selector) = Selector::parse("tr") {
                                let mut diff_lines = Vec::new();

                                for row in document.select(&diff_selector) {
                                    // Look for cells with old (removed) content
                                    if let Ok(old_selector) = Selector::parse("td.old") {
                                        if let Some(old_cell) = row.select(&old_selector).next() {
                                            let text = old_cell
                                                .text()
                                                .collect::<String>()
                                                .trim()
                                                .to_string();
                                            if !text.is_empty() {
                                                diff_lines.push(format!("- {}", text));
                                            }
                                        }
                                    }

                                    // Look for cells with new (added) content
                                    if let Ok(new_selector) = Selector::parse("td.new") {
                                        if let Some(new_cell) = row.select(&new_selector).next() {
                                            let text = new_cell
                                                .text()
                                                .collect::<String>()
                                                .trim()
                                                .to_string();
                                            if !text.is_empty() {
                                                diff_lines.push(format!("+ {}", text));
                                            }
                                        }
                                    }
                                }

                                if !diff_lines.is_empty() {
                                    return Some(diff_lines.join("\n"));
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                debug!("Failed to parse changeset diff JSON: {}", e);
            }
        }

        None
    }

    /// Extracts suggestion diff content from HTML table showing old/new lines
    fn extract_inline_suggestion(&self, html: &str) -> Option<String> {
        // Look for inline-suggestion-view content
        if let Some(start) = html.find("inline-suggestion-view") {
            // Find the table containing the suggestion
            let search_area = &html[start..];
            if let Some(table_start) = search_area.find("<table") {
                if let Some(table_end) = search_area.find("</table>") {
                    let table_html = &search_area[table_start..table_end + 8];

                    // Parse the table to extract old and new lines
                    let document = Html::parse_document(table_html);
                    let mut diff_lines = Vec::new();

                    if let Ok(row_selector) = Selector::parse("tr") {
                        for row in document.select(&row_selector) {
                            let row_html = row.html();
                            let row_text = row.text().collect::<String>();

                            // Look for old lines (removed) - check for "left old" class
                            if row_html.contains("left old") {
                                // Extract text and clean it up
                                let cleaned = row_text.trim().trim_start_matches("- ").trim();
                                if !cleaned.is_empty()
                                    && !cleaned.contains("break;")
                                    && !cleaned.contains("}")
                                {
                                    diff_lines.push(format!("- {}", cleaned));
                                }
                            }

                            // Look for new lines (added) - check for "right new" class
                            if row_html.contains("right new") {
                                // Extract text and clean it up
                                let cleaned = row_text.trim().trim_start_matches("+ ").trim();
                                if !cleaned.is_empty()
                                    && !cleaned.contains("break;")
                                    && !cleaned.contains("}")
                                {
                                    diff_lines.push(format!("+ {}", cleaned));
                                }
                            }
                        }
                    }

                    if !diff_lines.is_empty() {
                        return Some(diff_lines.join("\n"));
                    }
                }
            }
        }
        None
    }

    fn is_suggestion_done(&self, suggestion_element: &scraper::ElementRef) -> bool {
        // Look for parent inline comment that has "inline-is-done" class
        let mut current = suggestion_element.parent();
        while let Some(parent_node) = current {
            if let Some(parent_element) = parent_node.value().as_element() {
                if parent_element
                    .classes()
                    .any(|class| class == "inline-is-done")
                {
                    return true;
                }
            }
            current = parent_node.parent();
        }
        false
    }

    fn extract_suggestion_from_table(
        &self,
        suggestion_element: &scraper::ElementRef,
    ) -> Option<String> {
        let mut diff_lines = Vec::new();

        // Strategy 1: Try to find table with diff content
        if let Ok(table_selector) = Selector::parse("table") {
            if let Some(table) = suggestion_element.select(&table_selector).next() {
                if let Ok(row_selector) = Selector::parse("tr") {
                    for row in table.select(&row_selector) {
                        // Look for old lines (removed)
                        if let Ok(old_selector) = Selector::parse("td.left.old, td.old, .diff-old")
                        {
                            if let Some(old_cell) = row.select(&old_selector).next() {
                                let text = old_cell.text().collect::<String>().trim().to_string();
                                if !text.is_empty() && text != "-" {
                                    let cleaned = text.trim_start_matches("- ").trim();
                                    if !cleaned.is_empty() {
                                        diff_lines.push(format!("- {}", cleaned));
                                    }
                                }
                            }
                        }

                        // Look for new lines (added)
                        if let Ok(new_selector) = Selector::parse("td.right.new, td.new, .diff-new")
                        {
                            if let Some(new_cell) = row.select(&new_selector).next() {
                                let text = new_cell.text().collect::<String>().trim().to_string();
                                if !text.is_empty() && text != "+" {
                                    let cleaned = text.trim_start_matches("+ ").trim();
                                    if !cleaned.is_empty() {
                                        diff_lines.push(format!("+ {}", cleaned));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if !diff_lines.is_empty() {
            Some(diff_lines.join("\n"))
        } else {
            None
        }
    }

    async fn get_csrf_token(&self, revision_id: u32) -> Option<String> {
        let review_url = format!("{}/D{}", self.base_url, revision_id);

        match self.client.get(&review_url).send().await {
            Ok(response) => {
                if let Ok(html) = response.text().await {
                    let document = Html::parse_document(&html);

                    // Look for CSRF token in meta tag
                    let meta_selector = Selector::parse("meta[name='csrf-token']").ok()?;
                    if let Some(meta) = document.select(&meta_selector).next() {
                        return meta.value().attr("content").map(|s| s.to_string());
                    }

                    // Look for CSRF token in script tags
                    let script_selector = Selector::parse("script").ok()?;
                    let csrf_regex = Regex::new(r#"csrf["']?\s*:\s*["']([^"']+)"#).ok()?;

                    for script in document.select(&script_selector) {
                        if let Some(script_content) = script.text().next() {
                            if script_content.to_lowercase().contains("csrf") {
                                if let Some(captures) = csrf_regex.captures(script_content) {
                                    return captures.get(1).map(|m| m.as_str().to_string());
                                }
                            }
                        }
                    }
                }
            }
            Err(_) => {}
        }

        None
    }

    async fn get_user_info(&mut self, user_phid: &str) -> String {
        if let Some(cached) = self.user_cache.get(user_phid) {
            return cached.clone();
        }

        let url = format!("{}/api/user.search", self.base_url);
        let params = [
            ("api.token", self.api_token.as_str()),
            ("constraints[phids][0]", user_phid),
        ];

        match self.client.post(&url).form(&params).send().await {
            Ok(response) => {
                if let Ok(result) = response.json::<UserSearchResult>().await {
                    if let Some(_) = result.error_code {
                        self.user_cache
                            .insert(user_phid.to_string(), user_phid.to_string());
                        return user_phid.to_string();
                    }

                    if let Some(data) = result.result {
                        if let Some(user_data) = data.data.first() {
                            let fields = &user_data.fields;
                            let real_name = fields.real_name.as_deref().unwrap_or("");
                            let username = fields.username.as_deref().unwrap_or("");

                            let display_name = if !real_name.is_empty() {
                                if !username.is_empty() {
                                    format!("{} ({})", real_name, username)
                                } else {
                                    real_name.to_string()
                                }
                            } else if !username.is_empty() {
                                username.to_string()
                            } else {
                                user_phid.to_string()
                            };

                            self.user_cache
                                .insert(user_phid.to_string(), display_name.clone());
                            return display_name;
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Failed to fetch user info for {}: {}", user_phid, e);
            }
        }

        self.user_cache
            .insert(user_phid.to_string(), user_phid.to_string());
        user_phid.to_string()
    }

    async fn get_revision_phid(&self, diff_id: u32) -> Result<String> {
        let url = format!("{}/api/differential.revision.search", self.base_url);
        let params = [
            ("api.token", self.api_token.as_str()),
            ("constraints[ids][0]", &diff_id.to_string()),
        ];

        info!(
            "Fetching revision PHID for diff_id={} from: {}",
            diff_id, url
        );
        debug!("Request params: {:?}", params);

        let response = self
            .client
            .post(&url)
            .form(&params)
            .send()
            .await
            .context(format!("Failed to send request to {}", url))?;

        let status = response.status();
        info!("Response status: {}", status);

        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "<no response body>".to_string());
            error!("HTTP error {}: {}", status, error_text);
            anyhow::bail!("HTTP error {}: {}", status, error_text);
        }

        let response_text = response
            .text()
            .await
            .context("Failed to read response body")?;
        debug!(
            "Response body (first 500 chars): {}",
            &response_text.chars().take(500).collect::<String>()
        );

        let result: RevisionSearchResult =
            serde_json::from_str(&response_text).context(format!(
                "Failed to parse JSON response. Response was: {}",
                response_text
            ))?;

        if let Some(error_code) = result.error_code {
            anyhow::bail!(
                "API Error: {} - {}",
                error_code,
                result.error_info.unwrap_or_default()
            );
        }

        let data = result.result.context("No result data")?;
        let revision_data = data.data.first().context("No revision found")?;

        Ok(revision_data.phid.clone())
    }

    async fn get_revision_phid_with_progress(
        &self,
        diff_id: u32,
        pb: &ProgressBar,
    ) -> Result<String> {
        let url = format!("{}/api/differential.revision.search", self.base_url);
        let params = [
            ("api.token", self.api_token.as_str()),
            ("constraints[ids][0]", &diff_id.to_string()),
        ];

        pb.set_message("Making API request...");
        let response = self.client.post(&url).form(&params).send().await?;
        pb.inc(1);

        pb.set_message("Parsing response...");
        let result: RevisionSearchResult = response.json().await?;
        pb.inc(1);

        if let Some(error_code) = result.error_code {
            anyhow::bail!(
                "API Error: {} - {}",
                error_code,
                result.error_info.unwrap_or_default()
            );
        }

        let data = result.result.context("No result data")?;
        let revision_data = data.data.first().context("No revision found")?;

        Ok(revision_data.phid.clone())
    }

    async fn get_transactions(&self, object_phid: &str) -> Result<Vec<TransactionData>> {
        let url = format!("{}/api/transaction.search", self.base_url);
        let params = [
            ("api.token", self.api_token.as_str()),
            ("objectIdentifier", object_phid),
        ];

        info!(
            "Fetching transactions for object_phid={} from: {}",
            object_phid, url
        );
        debug!("Request params: {:?}", params);

        let response = self
            .client
            .post(&url)
            .form(&params)
            .send()
            .await
            .context(format!("Failed to send request to {}", url))?;

        let status = response.status();
        info!("Response status: {}", status);

        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "<no response body>".to_string());
            error!("HTTP error {}: {}", status, error_text);
            anyhow::bail!("HTTP error {}: {}", status, error_text);
        }

        let response_text = response
            .text()
            .await
            .context("Failed to read response body")?;
        debug!(
            "Response body (first 500 chars): {}",
            &response_text.chars().take(500).collect::<String>()
        );

        let result: TransactionSearchResult =
            serde_json::from_str(&response_text).context(format!(
                "Failed to parse JSON response. Response was: {}",
                response_text
            ))?;

        if let Some(error_code) = result.error_code {
            anyhow::bail!(
                "API Error: {} - {}",
                error_code,
                result.error_info.unwrap_or_default()
            );
        }

        let data = result.result.context("No result data")?;
        Ok(data.data)
    }

    async fn get_transactions_with_progress(
        &self,
        object_phid: &str,
        pb: &ProgressBar,
    ) -> Result<Vec<TransactionData>> {
        let url = format!("{}/api/transaction.search", self.base_url);
        let params = [
            ("api.token", self.api_token.as_str()),
            ("objectIdentifier", object_phid),
        ];

        pb.set_message("Making transactions API request...");
        let response = self.client.post(&url).form(&params).send().await?;
        pb.inc(1);

        pb.set_message("Parsing transactions response...");
        let result: TransactionSearchResult = response.json().await?;
        pb.inc(1);

        if let Some(error_code) = result.error_code {
            anyhow::bail!(
                "API Error: {} - {}",
                error_code,
                result.error_info.unwrap_or_default()
            );
        }

        let data = result.result.context("No result data")?;
        Ok(data.data)
    }

    fn format_timestamp(&self, timestamp: u64) -> String {
        let dt = DateTime::from_timestamp(timestamp as i64, 0).unwrap_or_default();
        dt.format("%Y-%m-%d %H:%M:%S").to_string()
    }

    async fn extract_comments(&mut self, transactions: Vec<TransactionData>) -> CommentsData {
        self.extract_comments_with_progress(transactions, &ProgressBar::hidden(), false)
            .await
    }

    async fn extract_comments_with_progress(
        &mut self,
        transactions: Vec<TransactionData>,
        pb: &ProgressBar,
        include_done: bool,
    ) -> CommentsData {
        let mut comments_data = CommentsData {
            general_comments: Vec::new(),
            inline_comments: Vec::new(),
            review_actions: Vec::new(),
        };

        let total_transactions = transactions.len();
        for (i, transaction) in transactions.into_iter().enumerate() {
            pb.set_message(format!(
                "Processing transaction {}/{}",
                i + 1,
                total_transactions
            ));
            let author_phid = transaction.author_phid.as_deref().unwrap_or("unknown");
            let author_name = self.get_user_info(author_phid).await;
            let date = self.format_timestamp(transaction.date_created);

            match transaction.transaction_type.as_deref().unwrap_or("unknown") {
                "comment" => {
                    for comment in transaction.comments {
                        let mut content = comment.content.raw.unwrap_or_default();
                        if content.is_empty() {
                            content = "*[Empty comment]*".to_string();
                        }

                        comments_data.general_comments.push(Comment {
                            author: author_name.clone(),
                            author_phid: author_phid.to_string(),
                            date: date.clone(),
                            date_timestamp: transaction.date_created,
                            content,
                            transaction_id: transaction.id.to_string(),
                            comment_id: comment.id.to_string(),
                        });
                    }
                }
                "inline" => {
                    let fields = transaction.fields.unwrap_or(serde_json::Value::Null);
                    for comment in transaction.comments {
                        let mut content = comment.content.raw.unwrap_or_default();
                        if content.is_empty() {
                            // Try to get suggestion content from web interface
                            let line_number =
                                fields.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                            let file_path =
                                fields.get("path").and_then(|v| v.as_str()).unwrap_or("");

                            if line_number > 0 && !file_path.is_empty() {
                                if let Some(suggestion) = self
                                    .fetch_suggestion_from_web(
                                        self.current_revision_id.unwrap_or(0),
                                        line_number,
                                        file_path,
                                        include_done,
                                    )
                                    .await
                                {
                                    content = suggestion;
                                } else {
                                    content = "*[Empty inline comment - likely contains a code suggestion that cannot be extracted via API]*".to_string();
                                }
                            } else {
                                content = "*[Empty inline comment - likely contains a code suggestion that cannot be extracted via API]*".to_string();
                            }
                        }

                        let file_path = fields
                            .get("path")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let line_number =
                            fields.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        let line_length =
                            fields.get("length").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
                        let diff_id = fields
                            .get("diff")
                            .and_then(|v| v.get("id"))
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "".to_string());
                        let is_done = fields
                            .get("isDone")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);

                        // Skip "done" inline comments unless explicitly requested
                        if is_done && !include_done {
                            continue;
                        }

                        comments_data.inline_comments.push(InlineComment {
                            author: author_name.clone(),
                            author_phid: author_phid.to_string(),
                            date: date.clone(),
                            date_timestamp: transaction.date_created,
                            content,
                            file_path,
                            line_number,
                            line_length,
                            diff_id,
                            is_done,
                            transaction_id: transaction.id.to_string(),
                            comment_id: comment.id.to_string(),
                        });
                    }
                }
                "request-changes" | "accept" | "reject" | "request-review" => {
                    let mut action_comments = Vec::new();
                    for comment in transaction.comments {
                        let content = comment.content.raw.unwrap_or_default();
                        if !content.is_empty() {
                            action_comments.push(content);
                        }
                    }

                    comments_data.review_actions.push(ReviewAction {
                        author: author_name.clone(),
                        author_phid: author_phid.to_string(),
                        date: date.clone(),
                        action: transaction
                            .transaction_type
                            .clone()
                            .unwrap_or_else(|| "unknown".to_string()),
                        comments: action_comments,
                        transaction_id: transaction.id.to_string(),
                    });
                }
                _ => {}
            }

            // Increment progress for each transaction processed
            pb.inc(1);
        }

        comments_data
    }

    fn format_as_markdown(&self, comments_data: CommentsData, diff_id: u32) -> String {
        let mut md_lines = Vec::new();

        // Header with clickable URL
        md_lines.push(format!(
            "# Phabricator Review Comments - {}/D{}",
            self.base_url, diff_id
        ));
        md_lines.push(String::new());

        // General Comments - sorted chronologically
        if !comments_data.general_comments.is_empty() {
            md_lines.push("## General Comments".to_string());
            md_lines.push(String::new());

            let mut sorted_comments = comments_data.general_comments.clone();
            sorted_comments.sort_by_key(|c| c.date_timestamp);

            for comment in &sorted_comments {
                md_lines.push(format!(
                    "### Comment by {} ({})",
                    comment.author, comment.date
                ));
                md_lines.push(String::new());
                md_lines.push(comment.content.clone());
                md_lines.push(String::new());
                md_lines.push("---".to_string());
                md_lines.push(String::new());
            }
        }

        // Inline Comments - sorted chronologically first, then by file and line
        if !comments_data.inline_comments.is_empty() {
            md_lines.push("## Inline Comments".to_string());
            md_lines.push(String::new());

            // Sort all inline comments chronologically first
            let mut sorted_inline_comments = comments_data.inline_comments.clone();
            sorted_inline_comments
                .sort_by_key(|c| (c.date_timestamp, c.file_path.clone(), c.line_number));

            // Group by file while preserving chronological order within each file
            let mut files: HashMap<String, Vec<&InlineComment>> = HashMap::new();
            for comment in &sorted_inline_comments {
                files
                    .entry(comment.file_path.clone())
                    .or_default()
                    .push(comment);
            }

            // Sort files by the earliest comment timestamp in each file
            let mut file_entries: Vec<_> = files.into_iter().collect();
            file_entries.sort_by_key(|(_, comments)| {
                comments.iter().map(|c| c.date_timestamp).min().unwrap_or(0)
            });

            for (file_path, file_comments) in file_entries {
                md_lines.push(format!("### File: `{}`", file_path));
                md_lines.push(String::new());

                for comment in file_comments {
                    let line_info = if comment.line_length > 1 {
                        format!(
                            "Line {}-{}",
                            comment.line_number,
                            comment.line_number + comment.line_length - 1
                        )
                    } else {
                        format!("Line {}", comment.line_number)
                    };

                    let done_marker = if comment.is_done { " [DONE]" } else { "" };
                    md_lines.push(format!(
                        "#### {} - {} ({}){}",
                        line_info, comment.author, comment.date, done_marker
                    ));
                    md_lines.push(String::new());

                    if !comment.content.is_empty() {
                        md_lines.push(comment.content.clone());
                    } else {
                        md_lines.push("*[No comment text]*".to_string());
                    }

                    md_lines.push(String::new());
                    md_lines.push("---".to_string());
                    md_lines.push(String::new());
                }
            }
        }

        md_lines.join("\n")
    }

    async fn extract_and_format(&mut self, diff_id: u32, include_done: bool) -> Result<String> {
        self.current_revision_id = Some(diff_id);

        // First, get basic info to calculate progress steps
        let phid = self.get_revision_phid(diff_id).await?;
        let transactions = self.get_transactions(&phid).await?;

        // Now create progress bar based on actual transaction count + 1 for formatting
        let total_steps = transactions.len() as u64 + 1;
        let pb = ProgressBar::new(total_steps);
        pb.set_style(
            ProgressStyle::default_bar()
                .template(
                    "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}",
                )?
                .progress_chars("#>-"),
        );

        // Enable steady tick for animation
        pb.enable_steady_tick(std::time::Duration::from_millis(100));

        pb.set_message(format!("Processing {} transactions...", transactions.len()));
        let comments_data = self
            .extract_comments_with_progress(transactions, &pb, include_done)
            .await;

        pb.set_message("Formatting as Markdown...");
        let markdown = self.format_as_markdown(comments_data, diff_id);
        pb.inc(1);

        pb.finish_with_message("Done!");

        // Clear the progress bar before outputting results
        pb.finish_and_clear();

        Ok(markdown)
    }

    fn extract_diff_id_from_url(&self, url: &str) -> Option<u32> {
        debug!("Extracting diff ID from URL: {}", url);
        let re = Regex::new(r"/D(\d+)(?:\?|$|#)").ok()?;
        let captures = re.captures(url)?;
        let diff_id = captures.get(1)?.as_str().parse().ok();
        if let Some(id) = diff_id {
            debug!("Extracted diff ID: {}", id);
        }
        diff_id
    }
}

fn parse_diff_id(diff_id_str: &str) -> Option<u32> {
    // Handle both "12345" and "D12345" formats
    let cleaned = diff_id_str.trim_start_matches('D').trim_start_matches('d');
    cleaned.parse().ok()
}

#[tokio::main]
async fn main() -> Result<()> {
    // Enable backtrace for better error context
    std::env::set_var("RUST_BACKTRACE", "1");

    // Initialize logger with RUST_LOG environment variable or default to info
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    info!("Starting phab-comments-to-md");

    let args = Args::parse();
    debug!("Parsed arguments: {:?}", args);

    // Get token from args or environment variable
    let token = args.token
        .or_else(|| std::env::var("PHABRICATOR_TOKEN").ok())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Phabricator API token required. Either:\n\
                 1. Use --token <TOKEN>\n\
                 2. Set PHABRICATOR_TOKEN environment variable\n\
                 \n\
                 Get your token at: https://phabricator.services.mozilla.com/settings/user/<username>/page/apitokens/"
            )
        })?;

    // Get base URL from args or environment variable
    let env_base_url = std::env::var("PHABRICATOR_BASE_URL").ok();

    // Determine diff ID and base URL
    let (diff_id, base_url) = if let Some(url) = args.url {
        info!("Processing URL: {}", url);
        let extractor = PhabricatorCommentExtractor::new(String::new(), token.clone());
        let diff_id = extractor
            .extract_diff_id_from_url(&url)
            .context("Could not extract diff ID from URL")?;
        info!("Extracted diff_id: {}", diff_id);

        // Extract base URL from the provided URL
        let parsed_url = Url::parse(&url)?;
        let base_url = format!(
            "{}://{}",
            parsed_url.scheme(),
            parsed_url.host_str().unwrap_or("")
        );

        (diff_id, base_url)
    } else if let Some(diff_id_str) = args.diff_id {
        let diff_id = parse_diff_id(&diff_id_str).context("Invalid diff ID format")?;

        let base_url = args
            .base_url
            .or(env_base_url)
            .unwrap_or_else(|| "https://phabricator.services.mozilla.com".to_string());

        (diff_id, base_url)
    } else {
        anyhow::bail!(
            "Either --url or --diff-id must be provided. Use --help for more information."
        );
    };

    // Create extractor and process
    info!("Creating extractor with base_url: {}", base_url);
    let mut extractor = PhabricatorCommentExtractor::new(base_url, token);

    info!(
        "Starting extraction for diff_id: {}, include_done: {}",
        diff_id, args.include_done
    );
    let markdown = match extractor
        .extract_and_format(diff_id, args.include_done)
        .await
    {
        Ok(md) => {
            info!("Successfully extracted and formatted comments");
            md
        }
        Err(e) => {
            error!("Failed to extract and format: {:?}", e);
            return Err(e);
        }
    };

    // Output
    if let Some(output_path) = args.output {
        fs::write(&output_path, &markdown)?;
        eprintln!("Comments extracted and saved to {}", output_path);
    } else {
        println!("{}", markdown);
    }

    Ok(())
}
