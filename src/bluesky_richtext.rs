// Forked from Atrium - we only want to detect links starting with http.
use bsky_sdk::{
    api::{
        app::bsky::richtext::facet::{
            ByteSlice, ByteSliceData, Link, LinkData, MainFeaturesItem, Tag, TagData,
        },
        types::Union,
    },
    rich_text::RichText,
};
use regex::Regex;
use std::sync::OnceLock;
use unicode_segmentation::UnicodeSegmentation;

static RE_URL: OnceLock<Regex> = OnceLock::new();
static RE_ENDING_PUNCTUATION: OnceLock<Regex> = OnceLock::new();
static RE_TRAILING_PUNCTUATION: OnceLock<Regex> = OnceLock::new();
static RE_TAG: OnceLock<Regex> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetWithoutResolution {
    pub features: Vec<FacetFeaturesItem>,
    pub index: ByteSlice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FacetFeaturesItem {
    Link(Box<Link>),
    Tag(Box<Tag>),
}

fn detect_facets_without_resolution(text: &str) -> Vec<FacetWithoutResolution> {
    let mut facets = Vec::new();
    // links
    {
        let url_regex =
            RE_URL.get_or_init(|| Regex::new(r"https?:\/\/[\S]+").expect("invalid regex"));
        let dot_regex =
            RE_ENDING_PUNCTUATION.get_or_init(|| Regex::new(r"[.,;:!?]$").expect("invalid regex"));
        for capture in url_regex.captures_iter(text) {
            let m = capture.get(0).expect("invalid capture");
            let mut uri = m.as_str().to_string();
            let mut index = ByteSliceData {
                byte_end: m.end(),
                byte_start: m.start(),
            };
            // strip ending puncuation
            if (dot_regex.is_match(&uri)) || (uri.ends_with(')') && !uri.contains('(')) {
                uri.pop();
                index.byte_end -= 1;
            }
            facets.push(FacetWithoutResolution {
                features: vec![FacetFeaturesItem::Link(Box::new(LinkData { uri }.into()))],
                index: index.into(),
            });
        }
    }
    // tags
    {
        let re = RE_TAG.get_or_init(|| {
          Regex::new(
              r"(?:^|\s)([#＃])([^\s\u00AD\u2060\u200A\u200B\u200C\u200D\u20e2]*[^\d\s\p{P}\u00AD\u2060\u200A\u200B\u200C\u200D\u20e2]+[^\s\u00AD\u2060\u200A\u200B\u200C\u200D\u20e2]*)?",
          )
          .expect("invalid regex")
        });
        let trail_regex =
            RE_TRAILING_PUNCTUATION.get_or_init(|| Regex::new(r"\p{P}+$").expect("invalid regex"));
        for capture in re.captures_iter(text) {
            if let Some(tag) = capture.get(2) {
                // strip ending punctuation and any spaces
                let tag = trail_regex.replace(tag.as_str(), "");
                // look-around, including look-ahead and look-behind, is not supported in `regex`
                if tag.starts_with('\u{fe0f}') {
                    continue;
                }
                if tag.len() > 64 {
                    continue;
                }
                let leading = capture.get(1).expect("invalid capture");
                let index = ByteSliceData {
                    byte_end: leading.end() + tag.len(),
                    byte_start: leading.start(),
                }
                .into();
                facets.push(FacetWithoutResolution {
                    features: vec![FacetFeaturesItem::Tag(Box::new(
                        TagData { tag: tag.into() }.into(),
                    ))],
                    index,
                });
            }
        }
    }
    facets
}

pub fn get_link_uris(text: &str) -> Vec<String> {
    detect_facets_without_resolution(text)
        .into_iter()
        .flat_map(|facet| facet.features.into_iter())
        .filter_map(|feature| match feature {
            FacetFeaturesItem::Link(link) => Some(link.uri.clone()),
            FacetFeaturesItem::Tag(_) => None,
        })
        .collect()
}

fn detect_facets(text: &str) -> RichText {
    let facets_without_resolution = detect_facets_without_resolution(text);
    let facets = if facets_without_resolution.is_empty() {
        None
    } else {
        let mut facets = Vec::new();
        for facet_without_resolution in facets_without_resolution {
            let mut features = Vec::new();
            for feature in facet_without_resolution.features {
                match feature {
                    FacetFeaturesItem::Link(link) => {
                        features.push(Union::Refs(MainFeaturesItem::Link(link)));
                    }
                    FacetFeaturesItem::Tag(tag) => {
                        features.push(Union::Refs(MainFeaturesItem::Tag(tag)));
                    }
                }
            }
            facets.push(
                bsky_sdk::api::app::bsky::richtext::facet::MainData {
                    features,
                    index: facet_without_resolution.index,
                }
                .into(),
            );
        }
        Some(facets)
    };
    RichText {
        text: text.into(),
        facets,
    }
}

// Shorten links so that the text stays compact and links look good on Bluesky.
pub fn get_rich_text(text: &str) -> RichText {
    let mut richtext = detect_facets(text);
    if let Some(ref facets) = richtext.facets {
        // Start replacing links from the end of the text.
        let mut reversed_facets = facets.clone();
        reversed_facets.reverse();
        for facet in reversed_facets {
            for feature in &facet.features {
                if let Union::Refs(MainFeaturesItem::Link(link)) = feature {
                    let uri = &link.uri;
                    // Strip protocol prefix for display.
                    let protocol_len = if uri.starts_with("https://") {
                        8usize
                    } else if uri.starts_with("http://") {
                        7usize
                    } else {
                        0usize
                    };
                    let display_uri = &uri[protocol_len..];
                    let www_len = if display_uri.starts_with("www.") {
                        4usize
                    } else {
                        0usize
                    };
                    let display_uri = &display_uri[www_len..];
                    let display_uri_length = display_uri.graphemes(true).count();
                    // If the display link is longer than 23 characters, shorten it.
                    if display_uri_length > 23 {
                        let link_part = display_uri.chars().take(22).collect::<String>();
                        // Replace the link with a shortened version (no protocol, no www).
                        richtext.insert(
                            facet.index.byte_start + protocol_len + www_len + link_part.len(),
                            "…",
                        );
                        richtext.delete(
                            facet.index.byte_start
                                + protocol_len
                                + www_len
                                + link_part.len()
                                + "…".len(),
                            facet.index.byte_end + "…".len(),
                        );
                    }
                    // Delete the protocol prefix and www.
                    if protocol_len > 0 {
                        richtext.delete(
                            facet.index.byte_start,
                            facet.index.byte_start + protocol_len + www_len,
                        );
                    }
                }
            }
        }
    }
    richtext
}

#[cfg(test)]
pub mod tests {
    use crate::bluesky_richtext::{get_link_uris, get_rich_text};

    // Test URL shortening.
    #[test]
    fn test_shorten_url() {
        let text = "Test toot with long link http://example.com/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let richtext = get_rich_text(text);
        assert_eq!(
            richtext.text,
            "Test toot with long link example.com/aaaaaaaaaa…"
        );
    }

    // Test www removal.
    #[test]
    fn test_shorten_url_www() {
        let text = "Test toot with long link http://www.example.com/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let richtext = get_rich_text(text);
        assert_eq!(
            richtext.text,
            "Test toot with long link example.com/aaaaaaaaaa…"
        );
    }

    #[test]
    // Test that only links starting with https:// or http:// are detected.
    fn test_link_detection() {
        let text = "♻️ bensaufley.com: This is awful from start to finish. The documentation of this guy's descent into hate is really chilling, to me. It's a story we seem to be seeing more and more, and to hear the personal side of this, from a warm and collaborative friend to this secret … villain … it's just so sad, and so scary.\n\n💬 lizthegrey.com:… https://mastodon.social/@klausi/113511471780554214";
        let richtext = get_rich_text(text);
        assert_eq!(
            richtext.text,
            "♻️ bensaufley.com: This is awful from start to finish. The documentation of this guy's descent into hate is really chilling, to me. It's a story we seem to be seeing more and more, and to hear the personal side of this, from a warm and collaborative friend to this secret … villain … it's just so sad, and so scary.\n\n💬 lizthegrey.com:… mastodon.social/@klaus…"
        );
    }

    #[test]
    fn test_get_link_uris_preserves_order_and_strips_punctuation() {
        let text = "First https://example.com/one, then http://example.com/two! #tag";
        assert_eq!(
            get_link_uris(text),
            vec![
                "https://example.com/one".to_string(),
                "http://example.com/two".to_string(),
            ]
        );
    }
}
