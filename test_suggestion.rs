use std::fs;

fn main() {
    let json_content = fs::read_to_string("/tmp/changeset_8450617.json").unwrap();
    
    // Strip the "for (;;);" prefix
    let clean_json = if json_content.starts_with("for (;;);") {
        &json_content[9..]
    } else {
        &json_content
    };
    
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(clean_json) {
        println\!("JSON parsed successfully");
        println\!("Looking for suggestionText...");
        find_suggestion_text_recursive(&json);
    } else {
        println\!("Failed to parse JSON");
    }
    
    // Also try regex approach
    let re = regex::Regex::new(r#""suggestionText":"((?:[^"\\]|\\.)*)""#).unwrap();
    if let Some(captures) = re.captures(&json_content) {
        if let Some(suggestion_match) = captures.get(1) {
            let suggestion_text = suggestion_match.as_str()
                .replace("\\n", "\n")
                .replace("\\t", "\t")
                .replace("\\u003e", ">")
                .replace("\\u003c", "<")
                .replace("\\/", "/")
                .replace("\\\"", "\"")
                .replace("\\\\", "\\");
            println\!("Found suggestion via regex: {:?}", suggestion_text);
        }
    } else {
        println\!("No suggestionText found via regex");
    }
}

fn find_suggestion_text_recursive(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            // Check if this object has suggestionText
            if let Some(suggestion_text) = map.get("suggestionText") {
                if let Some(text) = suggestion_text.as_str() {
                    if \!text.trim().is_empty() {
                        println\!("Found suggestionText: {:?}", text);
                        return Some(text.to_string());
                    }
                }
            }
            
            // Recursively search in all object values
            for (key, val) in map {
                if let Some(result) = find_suggestion_text_recursive(val) {
                    println\!("Found suggestionText in key {:?}: {:?}", key, result);
                    return Some(result);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            // Recursively search in all array elements
            for val in arr {
                if let Some(result) = find_suggestion_text_recursive(val) {
                    return Some(result);
                }
            }
        }
        _ => {}
    }
    None
}
EOF < /dev/null