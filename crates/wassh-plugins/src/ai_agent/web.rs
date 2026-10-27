//! Web fetch and web search.
//!
//! Port of the networking half of `tools.ts`: `web_fetch` strips a page down to
//! text, `web_search` asks Bing (API or page), Brave, Google or DuckDuckGo and
//! returns the same `[n] title / URL / snippet` shape the reference formats.

use serde_json::Value;

use super::tools::{html_to_text, MAX_FETCH_CHARS};

/// How long a fetch may take before it is abandoned.
pub const FETCH_TIMEOUT_SECONDS: u64 = 20;
/// How long a search request may take.
pub const SEARCH_TIMEOUT_SECONDS: u64 = 15;
/// The browser agent string the reference sends.
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                              (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36 WaSSH-Agent/1.0";

/// One search hit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    /// Page title.
    pub title: String,
    /// Page address.
    pub url: String,
    /// One-line summary.
    pub snippet: String,
}

/// A prepared HTTP agent with the reference's timeout.
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(FETCH_TIMEOUT_SECONDS)))
        .build()
        .into()
}

/// Read a response body as text.
fn body_text(response: ureq::http::Response<ureq::Body>) -> Result<(String, String), String> {
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = response
        .into_body()
        .read_to_string()
        .map_err(|error| format!("reading the body: {error}"))?;
    Ok((content_type, body))
}

/// `web_fetch`: fetch `url` and return its text.
pub fn web_fetch(url: &str, max_chars: Option<u64>) -> Result<String, String> {
    let limit = max_chars.map_or(16_000, |value| value as usize).clamp(1_000, MAX_FETCH_CHARS);
    let trimmed = url.trim();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err(format!(
            "Only HTTP and HTTPS URLs are supported (got {})",
            trimmed.split(':').next().unwrap_or("")
        ));
    }
    let response = agent()
        .get(trimmed)
        .header("user-agent", USER_AGENT)
        .header(
            "accept",
            "text/html,application/xhtml+xml,application/json,text/plain;q=0.9,*/*;q=0.8",
        )
        .call()
        .map_err(|error| format!("Failed to fetch URL {trimmed}: {error}"))?;
    let (content_type, raw) = body_text(response)?;
    let content = if content_type.contains("html") || raw.get(..1000).is_some_and(|head| {
        head.to_lowercase().contains("<html")
    }) {
        html_to_text(&raw)
    } else {
        raw
    };
    if content.trim().is_empty() {
        return Ok("(Empty response body)".to_owned());
    }
    if content.chars().count() > limit {
        let kept: String = content.chars().take(limit).collect();
        return Ok(format!(
            "{kept}\n\n[Content truncated at {limit} characters of {}]",
            content.chars().count()
        ));
    }
    Ok(content)
}

/// A `query`-parameter URL.
fn with_query(base: &str, query: &str, limit: usize) -> String {
    format!("{base}{}", percent_encode(query, limit))
}

/// Percent-encode the handful of characters a query string needs.
fn percent_encode(query: &str, limit: usize) -> String {
    let _ = limit;
    let mut encoded = String::with_capacity(query.len() * 3);
    for byte in query.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char)
            }
            b' ' => encoded.push_str("%20"),
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// Strip tags from a fragment.
fn plain(fragment: &str) -> String {
    let text = html_to_text(fragment);
    text.replace('\n', " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Decode the `uddg=` redirect DuckDuckGo returns in its HTML results.
fn unwrap_ddg(url: &str) -> String {
    let Some(index) = url.find("uddg=") else {
        return url.to_owned();
    };
    let rest = &url[index + 5..];
    let encoded = rest.split('&').next().unwrap_or(rest);
    let mut decoded = String::with_capacity(encoded.len());
    let bytes = encoded.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(value) = u8::from_str_radix(hex, 16) {
                decoded.push(value as char);
                i += 3;
                continue;
            }
        }
        decoded.push(bytes[i] as char);
        i += 1;
    }
    decoded
}

/// Parse Bing's HTML result page.
pub fn parse_bing(html: &str, limit: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();
    for block in html.split("<li class=\"b_algo\"").skip(1) {
        if results.len() >= limit {
            break;
        }
        let block = block.split("</li>").next().unwrap_or(block);
        let Some(link_start) = block.find("<h2") else {
            continue;
        };
        let heading = &block[link_start..];
        let Some(href) = heading.find("href=\"") else {
            continue;
        };
        let after = &heading[href + 6..];
        let Some(end) = after.find('"') else {
            continue;
        };
        let url = after[..end].to_owned();
        // The title is between `>` and `</a>` in the same anchor.
        let title = after
            .find('>')
            .and_then(|open| after[open + 1..].find("</a>").map(|close| &after[open + 1..open + 1 + close]))
            .map(plain)
            .unwrap_or_default();
        let snippet = block
            .find("<p")
            .and_then(|open| {
                let tail = &block[open..];
                tail.find('>').and_then(|gt| {
                    tail[gt + 1..]
                        .find("</p>")
                        .map(|close| &tail[gt + 1..gt + 1 + close])
                })
            })
            .map(plain)
            .unwrap_or_default();
        if !url.is_empty() {
            results.push(SearchResult { title, url, snippet });
        }
    }
    results
}

/// Parse DuckDuckGo's HTML result page.
pub fn parse_duckduckgo(html: &str, limit: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();
    for block in html.split("result__body").skip(1) {
        if results.len() >= limit {
            break;
        }
        let Some(link_start) = block.find("result__a") else {
            continue;
        };
        let heading = &block[link_start..];
        let Some(href) = heading.find("href=\"") else {
            continue;
        };
        let after = &heading[href + 6..];
        let Some(end) = after.find('"') else {
            continue;
        };
        let url = unwrap_ddg(&after[..end]);
        let title = after
            .find('>')
            .and_then(|open| after[open + 1..].find("</a>").map(|close| &after[open + 1..open + 1 + close]))
            .map(plain)
            .unwrap_or_default();
        let snippet = block
            .find("result__snippet")
            .and_then(|start| {
                let tail = &block[start..];
                tail.find('>').and_then(|gt| {
                    tail[gt + 1..]
                        .find("</a>")
                        .map(|close| &tail[gt + 1..gt + 1 + close])
                })
            })
            .map(plain)
            .unwrap_or_default();
        if !url.is_empty() {
            results.push(SearchResult { title, url, snippet });
        }
    }
    results
}

/// `web_search`: query the configured provider.
pub fn web_search(
    query: &str,
    provider: Option<&str>,
    api_key: Option<&str>,
    limit: Option<u64>,
) -> Result<String, String> {
    let count = limit.map_or(5, |value| value as usize).clamp(1, 10);
    let provider = provider.unwrap_or("bing").to_lowercase();
    let results = match provider.as_str() {
        "brave" => search_brave(query, count, api_key)?,
        "google" => search_google(query, count, api_key)?,
        "duckduckgo" => search_duckduckgo(query, count)?,
        _ => search_bing(query, count, api_key)?,
    };
    if results.is_empty() {
        return Ok(format!("No results found for query: \"{query}\""));
    }
    let formatted = results
        .iter()
        .enumerate()
        .map(|(index, result)| {
            let snippet = if result.snippet.is_empty() {
                String::new()
            } else {
                format!("{}\n", result.snippet)
            };
            format!("[{}] {}\nURL: {}\n{snippet}", index + 1, result.title, result.url)
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!(
        "Search results for \"{query}\" ({} results):\n\n{formatted}",
        results.len()
    ))
}

/// Bing, through the API when a key is given.
fn search_bing(query: &str, limit: usize, api_key: Option<&str>) -> Result<Vec<SearchResult>, String> {
    if let Some(key) = api_key.filter(|key| !key.is_empty()) {
        let url = with_query("https://api.bing.microsoft.com/v7.0/search?q=", query, limit)
            + &format!("&count={limit}");
        let response = agent()
            .get(&url)
            .header("ocp-apim-subscription-key", key)
            .call()
            .map_err(|error| format!("Bing API error: {error}"))?;
        let (_, body) = body_text(response)?;
        let json: Value =
            serde_json::from_str(&body).map_err(|error| format!("Bing API answered nonsense: {error}"))?;
        let mut results = Vec::new();
        if let Some(items) = json["webPages"]["value"].as_array() {
            for item in items.iter().take(limit) {
                results.push(SearchResult {
                    title: item["name"].as_str().unwrap_or_default().to_owned(),
                    url: item["url"].as_str().unwrap_or_default().to_owned(),
                    snippet: item["snippet"].as_str().unwrap_or_default().to_owned(),
                });
            }
        }
        return Ok(results);
    }
    let url = format!("https://www.bing.com/search?q={}", percent_encode(query, limit));
    let response = agent()
        .get(&url)
        .header("user-agent", USER_AGENT)
        .header("accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .call()
        .map_err(|error| format!("Bing returned {error}"))?;
    let (_, body) = body_text(response)?;
    Ok(parse_bing(&body, limit))
}

/// Brave, which needs a key.
fn search_brave(query: &str, limit: usize, api_key: Option<&str>) -> Result<Vec<SearchResult>, String> {
    let Some(key) = api_key.filter(|key| !key.is_empty()) else {
        return Err(
            "Brave search requires an API key. Please configure webSearchApiKey in settings.".to_owned(),
        );
    };
    let url = format!(
        "https://api.search.brave.com/res/v1/web/search?q={}&count={limit}",
        percent_encode(query, limit)
    );
    let response = agent()
        .get(&url)
        .header("accept", "application/json")
        .header("x-subscription-token", key)
        .call()
        .map_err(|error| format!("Brave Search API error: {error}"))?;
    let (_, body) = body_text(response)?;
    let json: Value =
        serde_json::from_str(&body).map_err(|error| format!("Brave answered nonsense: {error}"))?;
    let mut results = Vec::new();
    if let Some(items) = json["web"]["results"].as_array() {
        for item in items.iter().take(limit) {
            results.push(SearchResult {
                title: item["title"].as_str().unwrap_or_default().to_owned(),
                url: item["url"].as_str().unwrap_or_default().to_owned(),
                snippet: item["description"].as_str().unwrap_or_default().to_owned(),
            });
        }
    }
    Ok(results)
}

/// Google Custom Search, key spelled `KEY:CX`.
fn search_google(query: &str, limit: usize, api_key: Option<&str>) -> Result<Vec<SearchResult>, String> {
    let Some(raw) = api_key.filter(|key| !key.is_empty()) else {
        return Err("Google search requires an API key in format \"KEY:CX\" (Custom Search Engine ID)."
            .to_owned());
    };
    let mut parts = raw.splitn(2, ':');
    let (Some(key), Some(cx)) = (parts.next(), parts.next()) else {
        return Err("Google search API key must be in format \"API_KEY:ENGINE_CX\".".to_owned());
    };
    if key.is_empty() || cx.is_empty() {
        return Err("Google search API key must be in format \"API_KEY:ENGINE_CX\".".to_owned());
    }
    let url = format!(
        "https://www.googleapis.com/customsearch/v1?key={}&cx={}&q={}&num={limit}",
        percent_encode(key, limit),
        percent_encode(cx, limit),
        percent_encode(query, limit)
    );
    let response = agent()
        .get(&url)
        .call()
        .map_err(|error| format!("Google Search API error: {error}"))?;
    let (_, body) = body_text(response)?;
    let json: Value =
        serde_json::from_str(&body).map_err(|error| format!("Google answered nonsense: {error}"))?;
    let mut results = Vec::new();
    if let Some(items) = json["items"].as_array() {
        for item in items.iter().take(limit) {
            results.push(SearchResult {
                title: item["title"].as_str().unwrap_or_default().to_owned(),
                url: item["link"].as_str().unwrap_or_default().to_owned(),
                snippet: item["snippet"].as_str().unwrap_or_default().to_owned(),
            });
        }
    }
    Ok(results)
}

/// DuckDuckGo's HTML endpoint, no key needed.
fn search_duckduckgo(query: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(SEARCH_TIMEOUT_SECONDS)))
        .build();
    let agent: ureq::Agent = agent.into();
    let body = format!("q={}", percent_encode(query, limit));
    let response = agent
        .post("https://html.duckduckgo.com/html/")
        .header("user-agent", USER_AGENT)
        .header("content-type", "application/x-www-form-urlencoded")
        .send(body)
        .map_err(|error| format!("DuckDuckGo returned {error}"))?;
    let (_, html) = body_text(response)?;
    Ok(parse_duckduckgo(&html, limit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_percent_encoded() {
        assert_eq!(percent_encode("a b&c", 5), "a%20b%26c");
        assert_eq!(percent_encode("plain-name_1.0~", 5), "plain-name_1.0~");
    }

    #[test]
    fn bing_results_are_parsed() {
        let html = r#"<ol><li class="b_algo"><h2><a href="https://a.example/x">Alpha <b>one</b></a></h2>
            <p>First snippet</p></li>
            <li class="b_algo"><h2><a href="https://b.example/y">Beta</a></h2><p>Second</p></li></ol>"#;
        let results = parse_bing(html, 5);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://a.example/x");
        assert_eq!(results[0].title, "Alpha one");
        assert_eq!(results[0].snippet, "First snippet");
        assert_eq!(results[1].title, "Beta");
        // The limit is honoured.
        assert_eq!(parse_bing(html, 1).len(), 1);
    }

    #[test]
    fn duckduckgo_results_are_unwrapped() {
        let html = r#"<div class="result__body"><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.org%2Fa&amp;rut=1">Title here</a>
            <a class="result__snippet">A snippet</a></div>"#;
        let results = parse_duckduckgo(html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://example.org/a");
        assert_eq!(results[0].title, "Title here");
        assert_eq!(results[0].snippet, "A snippet");
    }

    #[test]
    fn a_bad_scheme_is_refused_before_any_request() {
        let error = web_fetch("ftp://example.org/file", None).unwrap_err();
        assert!(error.contains("Only HTTP and HTTPS"), "{error}");
    }

    #[test]
    fn brave_and_google_need_keys() {
        assert!(web_search("rust", Some("brave"), None, None).unwrap_err().contains("API key"));
        assert!(web_search("rust", Some("google"), None, None).unwrap_err().contains("KEY:CX"));
        assert!(
            web_search("rust", Some("google"), Some("onlykey"), None)
                .unwrap_err()
                .contains("API_KEY:ENGINE_CX")
        );
    }

}
