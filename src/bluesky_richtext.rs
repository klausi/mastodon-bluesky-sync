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
use scraper::{ElementRef, Html, Node};
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

// Build RichText while preserving Mastodon anchors (<a href="...">text</a>)
// as clickable link facets.
fn detect_facets_with_mastodon_links(text: &str) -> RichText {
    if !text.contains("<a ") {
        return detect_facets(text);
    }

    let fragment = Html::parse_fragment(text);
    let mut plain_text = String::new();
    let mut anchor_facets = Vec::new();

    for child in fragment.tree.root().children() {
        append_text_and_anchor_facets(child, &mut plain_text, &mut anchor_facets);
    }

    // Preserve existing automatic URL and hashtag detection.
    let mut richtext = detect_facets(&plain_text);
    let mut merged_facets = richtext.facets.take().unwrap_or_default();
    merged_facets.extend(anchor_facets);

    if merged_facets.is_empty() {
        richtext.facets = None;
    } else {
        // Keep deterministic order and remove duplicates caused by url-like link text.
        merged_facets.sort_by_key(|facet| (facet.index.byte_start, facet.index.byte_end));
        merged_facets.dedup_by(|left, right| {
            if left.index.byte_start != right.index.byte_start
                || left.index.byte_end != right.index.byte_end
            {
                return false;
            }

            let left_link = left.features.iter().find_map(|feature| {
                if let Union::Refs(MainFeaturesItem::Link(link)) = feature {
                    Some(link.uri.as_str())
                } else {
                    None
                }
            });
            let right_link = right.features.iter().find_map(|feature| {
                if let Union::Refs(MainFeaturesItem::Link(link)) = feature {
                    Some(link.uri.as_str())
                } else {
                    None
                }
            });
            left_link.is_some() && left_link == right_link
        });
        richtext.facets = Some(merged_facets);
    }

    richtext
}

// Traverse parsed nodes, copying visible text and creating link facets for
// preserved external anchor tags.
fn append_text_and_anchor_facets(
    node: ego_tree::NodeRef<'_, Node>,
    plain_text: &mut String,
    anchor_facets: &mut Vec<bsky_sdk::api::app::bsky::richtext::facet::Main>,
) {
    if let Some(element) = ElementRef::wrap(node)
        && element.value().name() == "a"
    {
        let href = element
            .attr("href")
            .map(|value| html_escape::decode_html_entities(value).trim().to_string())
            .unwrap_or_default();
        let is_external = href.starts_with("http://") || href.starts_with("https://");
        let start = plain_text.len();
        for child in node.children() {
            append_plain_text(child, plain_text);
        }
        let end = plain_text.len();
        if is_external && end > start {
            anchor_facets.push(
                bsky_sdk::api::app::bsky::richtext::facet::MainData {
                    features: vec![Union::Refs(MainFeaturesItem::Link(Box::new(
                        LinkData { uri: href }.into(),
                    )))],
                    index: ByteSliceData {
                        byte_start: start,
                        byte_end: end,
                    }
                    .into(),
                }
                .into(),
            );
        }
        return;
    }

    if ElementRef::wrap(node).is_some() {
        for child in node.children() {
            append_text_and_anchor_facets(child, plain_text, anchor_facets);
        }
        return;
    }

    append_plain_text(node, plain_text);
}

// Recursively collect only rendered text from a node subtree.
fn append_plain_text(node: ego_tree::NodeRef<'_, Node>, plain_text: &mut String) {
    if let Some(_element) = ElementRef::wrap(node) {
        for child in node.children() {
            append_plain_text(child, plain_text);
        }
        return;
    }

    if let Node::Text(text) = node.value() {
        plain_text.push_str(text);
    }
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
    let mut richtext = detect_facets_with_mastodon_links(text);
    if let Some(ref facets) = richtext.facets {
        // Start replacing links from the end of the text.
        let mut reversed_facets = facets.clone();
        reversed_facets.reverse();
        for facet in reversed_facets {
            for feature in &facet.features {
                if let Union::Refs(MainFeaturesItem::Link(link)) = feature {
                    let visible_text = richtext
                        .text
                        .get(facet.index.byte_start..facet.index.byte_end)
                        .unwrap_or("");
                    if !visible_text.starts_with("https://") && !visible_text.starts_with("http://")
                    {
                        continue;
                    }

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
    use bsky_sdk::api::app::bsky::richtext::facet::MainFeaturesItem;
    use bsky_sdk::api::types::Union;

    use crate::bluesky_richtext::get_rich_text;

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
    fn test_get_rich_text_with_mastodon_links() {
        let text = "Read <a href=\"https://example.com/path\">this article</a> now.";
        let richtext = get_rich_text(text);

        assert_eq!(richtext.text, "Read this article now.");
        let facets = richtext.facets.expect("expected link facet");
        assert_eq!(facets.len(), 1);
        assert_eq!(facets[0].index.byte_start, 5);
        assert_eq!(facets[0].index.byte_end, 17);
        if let Union::Refs(MainFeaturesItem::Link(link)) = &facets[0].features[0] {
            assert_eq!(link.uri, "https://example.com/path");
        } else {
            panic!("expected link facet");
        }
    }
}
