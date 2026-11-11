//! Searching the public registries for an image to run.
//!
//! Docker Hub and Quay answer public JSON searches, so the pane can offer names instead of
//! asking the user to remember them. The search runs here rather than on the host: both
//! answers are public, and a host without `curl` or `wget` can still be asked for a
//! container. GitHub's registry has no public search, so its images are typed in by hand.

use std::time::Duration;

use serde_json::Value;

/// Docker Hub's repository search.
const HUB_SEARCH: &str = "https://hub.docker.com/v2/search/repositories/";
/// Docker Hub's tags of one repository.
const HUB_TAGS: &str = "https://hub.docker.com/v2/repositories/";
/// Quay's repository find.
const QUAY_SEARCH: &str = "https://quay.io/api/v1/find/repositories";
/// Quay's tags of one repository.
const QUAY_TAGS: &str = "https://quay.io/api/v1/repository/";
/// Tags asked for when looking for one to run.
const TAG_PAGE: &str = "50";
/// The tag Docker uses when none is named.
const LATEST: &str = "latest";
/// Where a bare name lives on Docker Hub.
const HUB_LIBRARY: &str = "library/";
/// The one registry whose paths are spelled out in a reference.
const QUAY_HOST: &str = "quay.io/";
/// Results asked for per registry.
const PAGE_SIZE: &str = "20";
/// Longest a search may take before it is abandoned.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Who the registries see asking.
const USER_AGENT: &str = "WaSSH-DockerManager/1.0";

/// One image a registry knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// What to pull, e.g. `nginx` or `quay.io/nginx/nginx-unprivileged`.
    pub reference: String,
    /// One-line description; empty when the registry has none.
    pub description: String,
    /// Stars, where the registry counts them.
    pub stars: u64,
    /// Whether the registry marks the image official.
    pub official: bool,
}

/// Ask every registry this pane knows about, Hub first.
pub fn search(query: &str) -> Result<Vec<Hit>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let mut hits = Vec::new();
    let mut failure = None;
    for answer in [hub(query), quay(query)] {
        match answer {
            Ok(mut found) => hits.append(&mut found),
            Err(error) => failure = failure.or(Some(error)),
        }
    }
    match failure {
        // One registry answering is enough; silence is the only real failure.
        Some(error) if hits.is_empty() => Err(error),
        _ => Ok(hits),
    }
}

/// Docker Hub's answer.
pub fn hub(query: &str) -> Result<Vec<Hit>, String> {
    let body = get(HUB_SEARCH, &[("query", query), ("page_size", PAGE_SIZE)])?;
    Ok(parse_hub(&body))
}

/// Quay's answer.
pub fn quay(query: &str) -> Result<Vec<Hit>, String> {
    let body = get(QUAY_SEARCH, &[("query", query)])?;
    Ok(parse_quay(&body))
}

/// `GET` a search endpoint and return its body.
fn get(url: &str, params: &[(&str, &str)]) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .into();
    let mut request = agent.get(url).header("user-agent", USER_AGENT);
    for (key, value) in params {
        request = request.query(key, value);
    }
    let response = request
        .call()
        .map_err(|error| format!("The registry search failed: {error}"))?;
    response
        .into_body()
        .read_to_string()
        .map_err(|error| format!("Reading the registry answer: {error}"))
}

/// Read Docker Hub's repository list.
pub fn parse_hub(text: &str) -> Vec<Hit> {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let Some(results) = value.get("results").and_then(Value::as_array) else {
        return Vec::new();
    };
    results
        .iter()
        .filter_map(|entry| {
            let reference = text_of(entry, "repo_name");
            if reference.is_empty() {
                return None;
            }
            Some(Hit {
                reference,
                description: text_of(entry, "short_description"),
                stars: entry.get("star_count").and_then(Value::as_u64).unwrap_or_default(),
                official: entry
                    .get("is_official")
                    .and_then(Value::as_bool)
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// Read Quay's repository list.
pub fn parse_quay(text: &str) -> Vec<Hit> {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let Some(results) = value.get("results").and_then(Value::as_array) else {
        return Vec::new();
    };
    results
        .iter()
        .filter_map(|entry| {
            let namespace = entry
                .get("namespace")
                .map(|namespace| text_of(namespace, "name"))
                .unwrap_or_default();
            let name = text_of(entry, "name");
            if name.is_empty() {
                return None;
            }
            let reference = if namespace.is_empty() {
                format!("quay.io/{name}")
            } else {
                format!("quay.io/{namespace}/{name}")
            };
            Some(Hit {
                reference,
                description: text_of(entry, "description"),
                stars: 0,
                official: namespace == "quay.io" || namespace == "library",
            })
        })
        .collect()
}

/// The tag to run for `reference` when the user has not chosen one.
///
/// Docker assumes `latest`, which many images never publish: `eclipse/mosquitto` has only
/// version tags, so a plain pull of it fails with "not found" even though a search found
/// it. The newest tag the registry lists is the sensible stand-in.
pub fn default_tag(reference: &str) -> Result<String, String> {
    Ok(pick_tag(&tags(reference)?))
}

/// The tag to run, given the tags a repository has newest first.
pub fn pick_tag(tags: &[String]) -> String {
    if tags.iter().any(|tag| tag == LATEST) {
        LATEST.to_owned()
    } else {
        tags.first().cloned().unwrap_or_default()
    }
}

/// The tags `reference` has, newest first.
pub fn tags(reference: &str) -> Result<Vec<String>, String> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(path) = reference.strip_prefix(QUAY_HOST) {
        let url = format!("{QUAY_TAGS}{path}/tag/");
        let body = get(&url, &[("limit", TAG_PAGE), ("onlyActiveTags", "true")])?;
        return Ok(names(&body, "tags"));
    }
    let path = if reference.contains('/') {
        reference.to_owned()
    } else {
        format!("{HUB_LIBRARY}{reference}")
    };
    let url = format!("{HUB_TAGS}{path}/tags");
    let body = get(&url, &[("page_size", TAG_PAGE), ("ordering", "last_updated")])?;
    Ok(names(&body, "results"))
}

/// The `name` of every entry in one of the registries' list answers.
fn names(text: &str, key: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let Some(entries) = value.get(key).and_then(Value::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let name = text_of(entry, "name");
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

/// A trimmed string field, empty when it is missing or not a string.
fn text_of(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_hub_results_become_references() {
        let hits = parse_hub(
            r#"{"count":2,"results":[
                {"repo_name":"nginx","short_description":"Official build of Nginx.",
                 "star_count":21419,"is_official":true},
                {"repo_name":"nginx/nginx-ingress","short_description":null,
                 "star_count":124,"is_official":false}]}"#,
        );
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].reference, "nginx");
        assert_eq!(hits[0].description, "Official build of Nginx.");
        assert_eq!(hits[0].stars, 21_419);
        assert!(hits[0].official);
        assert_eq!(hits[1].reference, "nginx/nginx-ingress");
        assert_eq!(hits[1].description, "");
        assert!(!hits[1].official);
    }

    #[test]
    fn quay_results_carry_their_registry() {
        let hits = parse_quay(
            r#"{"results":[
                {"kind":"repository","name":"nginx-unprivileged",
                 "namespace":{"name":"nginx","kind":"organization"},
                 "description":"Nginx on port 8080.","is_public":true}]}"#,
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].reference, "quay.io/nginx/nginx-unprivileged");
        assert_eq!(hits[0].description, "Nginx on port 8080.");
        assert!(!hits[0].official);
    }

    #[test]
    fn the_tag_to_run_prefers_latest_and_falls_back_to_the_newest() {
        // `eclipse/mosquitto` publishes no latest at all, which is why a plain pull of it
        // answers "not found" while a search finds it.
        assert_eq!(pick_tag(&["1.4.8".to_owned(), "1.4.4".to_owned()]), "1.4.8");
        assert_eq!(pick_tag(&["2.0".to_owned(), "latest".to_owned()]), "latest");
        assert_eq!(pick_tag(&[]), "");
        assert_eq!(
            names(r#"{"count":2,"results":[{"name":"1.4.8"},{"name":"1.4.4"}]}"#, "results"),
            vec!["1.4.8".to_owned(), "1.4.4".to_owned()]
        );
        assert!(
            names(r#"{"tags":[{"name":"trixie-perl"}]}"#, "tags")
                == vec!["trixie-perl".to_owned()],
            "Quay lists its tags under a tags key"
        );
        assert!(names("not json", "results").is_empty());
    }

    #[test]
    fn answers_that_are_not_lists_say_nothing() {
        assert!(parse_hub("Not Found").is_empty());
        assert!(parse_hub(r#"{"results":null}"#).is_empty());
        assert!(parse_quay("").is_empty());
        assert!(parse_quay(r#"{"results":[{"name":""}]}"#).is_empty());
        assert!(search("   ").unwrap().is_empty());
    }
}
