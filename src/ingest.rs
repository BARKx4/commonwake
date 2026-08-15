use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use chrono::Utc;
use feed_rs::model::Entry;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use url::{Host, Url};

use crate::{
    crypto::sha256_hex,
    db::IngestedObservation,
    error::{CommonwakeError, Result},
    model::SourceView,
    node::CommonwakeNode,
};

const MAX_FEED_BYTES: usize = 8 * 1024 * 1024;
const MAX_FEED_ENTRIES: usize = 1_000;
const MAX_METADATA_VALUES: usize = 64;
pub(crate) const MAX_TITLE_CHARS: usize = 500;
pub(crate) const MAX_SUMMARY_CHARS: usize = 2_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestReport {
    pub sources_attempted: usize,
    pub sources_succeeded: usize,
    pub observations_added: usize,
    pub source_results: Vec<SourceIngestResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceIngestResult {
    pub source_id: String,
    pub feed_url: String,
    pub observations_seen: usize,
    pub observations_accepted: usize,
    pub error: Option<String>,
}

impl CommonwakeNode {
    pub async fn ingest_all(&self) -> Result<IngestReport> {
        let sources = self.db.ingestible_sources()?;
        let mut report = IngestReport {
            sources_attempted: sources.len(),
            sources_succeeded: 0,
            observations_added: 0,
            source_results: Vec::new(),
        };

        for source in sources {
            let result = self.ingest_source(&source).await;
            match result {
                Ok((seen, accepted)) => {
                    self.db.mark_source_fetch(&source.source_id, true)?;
                    report.sources_succeeded += 1;
                    report.observations_added += accepted;
                    report.source_results.push(SourceIngestResult {
                        source_id: source.source_id,
                        feed_url: source.feed_url,
                        observations_seen: seen,
                        observations_accepted: accepted,
                        error: None,
                    });
                }
                Err(error) => {
                    self.db.mark_source_fetch(&source.source_id, false)?;
                    report.source_results.push(SourceIngestResult {
                        source_id: source.source_id,
                        feed_url: source.feed_url,
                        observations_seen: 0,
                        observations_accepted: 0,
                        error: Some(error.to_string()),
                    });
                }
            }
        }
        Ok(report)
    }

    async fn ingest_source(&self, source: &SourceView) -> Result<(usize, usize)> {
        let url = Url::parse(&source.feed_url)
            .map_err(|_| CommonwakeError::Validation("stored feed URL is invalid".into()))?;
        validate_fetch_url(&url)?;
        let client = feed_client(&url).await?;
        let response = client.get(url).send().await?.error_for_status()?;
        if response.status().is_redirection() {
            return Err(CommonwakeError::Validation(
                "feed redirects are disabled; review and propose the final feed URL".into(),
            ));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_FEED_BYTES as u64)
        {
            return Err(CommonwakeError::Validation(format!(
                "feed exceeds {MAX_FEED_BYTES} bytes"
            )));
        }
        let bytes = read_bounded_response(response, MAX_FEED_BYTES).await?;
        self.ingest_feed_bytes(source, &bytes)
    }

    pub fn ingest_feed_bytes(&self, source: &SourceView, bytes: &[u8]) -> Result<(usize, usize)> {
        if bytes.len() > MAX_FEED_BYTES {
            return Err(CommonwakeError::Validation(format!(
                "feed exceeds {MAX_FEED_BYTES} bytes"
            )));
        }
        let feed = feed_rs::parser::parse(bytes)
            .map_err(|error| CommonwakeError::Validation(format!("feed parse failed: {error}")))?;
        if feed.entries.len() > MAX_FEED_ENTRIES {
            return Err(CommonwakeError::Validation(format!(
                "feed contains more than {MAX_FEED_ENTRIES} entries"
            )));
        }
        let retrieved_at = Utc::now();
        let language = feed
            .language
            .clone()
            .or_else(|| source.languages.first().cloned());
        let seen = feed.entries.len();
        let mut accepted = 0;

        for entry in &feed.entries {
            let Some(canonical_url) = entry_url(entry) else {
                continue;
            };
            let title = entry.title.as_ref().map_or_else(
                || "Untitled observation".into(),
                |text| truncate(&text.content, MAX_TITLE_CHARS),
            );
            let summary = entry.summary.as_ref().map_or_else(String::new, |text| {
                truncate(&text.content, MAX_SUMMARY_CHARS)
            });
            let published_at = entry
                .published
                .or(entry.updated)
                .map(|time| time.with_timezone(&Utc));
            let raw_metadata = json!({
                "feed_id": feed.id,
                "entry_id": entry.id,
                "authors": entry.authors.iter().take(MAX_METADATA_VALUES)
                    .map(|author| truncate(&author.name, 200)).collect::<Vec<_>>(),
                "categories": entry.categories.iter().take(MAX_METADATA_VALUES)
                    .map(|category| truncate(&category.term, 200)).collect::<Vec<_>>(),
            });
            let document_hash = sha256_hex(
                serde_jcs::to_vec(&json!({
                    "canonical_url": canonical_url,
                    "title": title,
                    "summary": summary,
                    "published_at": published_at,
                    "raw_metadata": raw_metadata,
                }))
                .map_err(|error| {
                    CommonwakeError::Internal(format!("canonical feed item failed: {error}"))
                })?,
            );
            let observation = IngestedObservation {
                source_id: source.source_id.clone(),
                canonical_url,
                title,
                summary,
                published_at,
                retrieved_at,
                language: language.clone(),
                document_hash,
                raw_metadata,
            };
            let before = self.db.current_head()?.0;
            let result = self.db.append_observation(&self.identity, &observation)?;
            if result.sequence > before {
                accepted += 1;
            }
        }

        Ok((seen, accepted))
    }
}

async fn read_bounded_response(mut response: reqwest::Response, maximum: usize) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(CommonwakeError::Validation(format!(
            "feed exceeds {maximum} bytes"
        )));
    }
    let capacity = response.content_length().unwrap_or(0).min(maximum as u64) as usize;
    let mut bytes = Vec::with_capacity(capacity);
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > maximum {
            return Err(CommonwakeError::Validation(format!(
                "feed exceeds {maximum} decoded bytes"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn feed_client(url: &Url) -> Result<Client> {
    let mut builder = Client::builder()
        .user_agent(format!("commonwake/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();

    if let Host::Domain(domain) = url
        .host()
        .ok_or_else(|| CommonwakeError::Validation("feed URL has no host".into()))?
    {
        let port = url
            .port_or_known_default()
            .ok_or_else(|| CommonwakeError::Validation("feed URL has no resolvable port".into()))?;
        let addresses: Vec<SocketAddr> = tokio::net::lookup_host((domain, port))
            .await
            .map_err(|error| {
                CommonwakeError::Validation(format!("feed hostname resolution failed: {error}"))
            })?
            .collect();
        if addresses.is_empty() {
            return Err(CommonwakeError::Validation(
                "feed hostname resolved to no addresses".into(),
            ));
        }
        for address in &addresses {
            reject_private_ip(address.ip())?;
        }
        builder = builder.resolve_to_addrs(domain, &addresses);
    }

    builder.build().map_err(CommonwakeError::from)
}

fn validate_fetch_url(url: &Url) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(CommonwakeError::Validation(
            "feed fetch only permits HTTP and HTTPS".into(),
        ));
    }
    let Some(host) = url.host() else {
        return Err(CommonwakeError::Validation("feed URL has no host".into()));
    };
    match host {
        Host::Domain(domain) => {
            let domain = domain.to_ascii_lowercase();
            if matches!(domain.as_str(), "localhost" | "localhost.localdomain")
                || domain.ends_with(".local")
                || domain.ends_with(".internal")
            {
                return Err(CommonwakeError::Validation(
                    "feed URL targets a local hostname".into(),
                ));
            }
        }
        Host::Ipv4(address) => reject_private_ip(IpAddr::V4(address))?,
        Host::Ipv6(address) => reject_private_ip(IpAddr::V6(address))?,
    }
    Ok(())
}

fn reject_private_ip(address: IpAddr) -> Result<()> {
    let disallowed = match address {
        IpAddr::V4(ip) => is_non_public_v4(ip),
        IpAddr::V6(ip) => ip.to_ipv4().map_or_else(
            || {
                ip.is_loopback()
                    || ip.is_unspecified()
                    || ip.is_unique_local()
                    || ip.is_unicast_link_local()
                    || ip.is_multicast()
                    || is_non_public_v6_special(ip)
            },
            is_non_public_v4,
        ),
    };
    if disallowed {
        return Err(CommonwakeError::Validation(
            "feed URL targets a non-public IP address".into(),
        ));
    }
    Ok(())
}

fn is_non_public_v6_special(ip: std::net::Ipv6Addr) -> bool {
    let segments = ip.segments();
    // Deprecated site-local, documentation, benchmarking, 6to4, and the
    // well-known NAT64 prefix are unsuitable public collector destinations.
    // 6to4/NAT64 can otherwise encode an IPv4 private target in an apparently
    // global IPv6 literal.
    (segments[0] & 0xffc0) == 0xfec0
        || (segments[0] == 0x2001 && matches!(segments[1], 0x0002 | 0x0db8))
        || segments[0] == 0x2002
        || (segments[0] == 0x0064
            && segments[1] == 0xff9b
            && segments[2..6].iter().all(|segment| *segment == 0))
}

fn is_non_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (64..=127).contains(&b))
        || (a == 192 && b == 0)
        || (a == 198 && (18..=19).contains(&b))
        || a >= 240
}

fn entry_url(entry: &Entry) -> Option<String> {
    entry
        .links
        .iter()
        .filter(|link| link.rel.as_deref().is_none_or(|rel| rel == "alternate"))
        .find_map(|link| normalize_document_url(&link.href))
        .or_else(|| {
            entry
                .links
                .iter()
                .find_map(|link| normalize_document_url(&link.href))
        })
        .or_else(|| normalize_document_url(&entry.id))
}

fn normalize_document_url(value: &str) -> Option<String> {
    let mut url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    url.set_fragment(None);
    Some(url.to_string())
}

fn truncate(value: &str, maximum: usize) -> String {
    let mut output: String = value.chars().take(maximum).collect();
    if value.chars().count() > maximum {
        output.push('…');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_local_fetch_targets() {
        assert!(validate_fetch_url(&Url::parse("http://127.0.0.1/feed").unwrap()).is_err());
        assert!(validate_fetch_url(&Url::parse("http://service.local/feed").unwrap()).is_err());
        assert!(validate_fetch_url(&Url::parse("https://example.org/feed").unwrap()).is_ok());
        assert!(reject_private_ip("100.64.0.1".parse().unwrap()).is_err());
        assert!(reject_private_ip("198.18.0.1".parse().unwrap()).is_err());
        assert!(reject_private_ip("::ffff:127.0.0.1".parse().unwrap()).is_err());
        assert!(reject_private_ip("::127.0.0.1".parse().unwrap()).is_err());
        assert!(reject_private_ip("2001:db8::1".parse().unwrap()).is_err());
        assert!(reject_private_ip("2002:c0a8:101::".parse().unwrap()).is_err());
        assert!(reject_private_ip("64:ff9b::7f00:1".parse().unwrap()).is_err());
        assert!(reject_private_ip("93.184.216.34".parse().unwrap()).is_ok());
        assert_eq!(
            normalize_document_url("https://example.org/article#section").as_deref(),
            Some("https://example.org/article")
        );
        assert!(normalize_document_url("javascript:alert(1)").is_none());
        assert!(normalize_document_url("https://agent:secret@example.org/").is_none());
    }
}
