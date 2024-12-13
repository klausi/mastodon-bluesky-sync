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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionWithoutResolution {
    pub handle: String,
}

fn detect_facets_without_resolution(text: &str) -> Vec<FacetWithoutResolution> {
    let mut facets = Vec::new();
    // links
    {
        let re = RE_URL.get_or_init(|| Regex::new(r"https?:\/\/[\S]+").expect("invalid regex"));
        for capture in re.captures_iter(text) {
            let m = capture.get(0).expect("invalid capture");
            let mut uri = m.as_str().to_string();
            let mut index = ByteSliceData {
                byte_end: m.end(),
                byte_start: m.start(),
            };
            // strip ending puncuation
            if (RE_ENDING_PUNCTUATION
                .get_or_init(|| Regex::new(r"[.,;:!?]$").expect("invalid regex"))
                .is_match(&uri))
                || (uri.ends_with(')') && !uri.contains('('))
            {
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
        for capture in re.captures_iter(text) {
            if let Some(tag) = capture.get(2) {
                // strip ending punctuation and any spaces
                let tag = RE_TRAILING_PUNCTUATION
                    .get_or_init(|| Regex::new(r"\p{P}+$").expect("invalid regex"))
                    .replace(tag.as_str(), "");
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

// Shorten links if necessary so that the text stays below the 300 character
// limit on Bluesky.
pub fn get_rich_text(text: &str) -> RichText {
    let mut richtext = detect_facets(text);
    if richtext.grapheme_len() <= 300 {
        return richtext;
    }
    if let Some(ref facets) = richtext.facets {
        // Start replacing links from the end of the text.
        let mut reversed_facets = facets.clone();
        reversed_facets.reverse();
        for facet in reversed_facets {
            for feature in &facet.features {
                if let Union::Refs(MainFeaturesItem::Link(link)) = feature {
                    // If the link is longer than 23 characters, shorten it.
                    let uri = &link.uri;
                    let uri_length = uri.graphemes(true).count();
                    if uri_length > 23 {
                        let text_length = richtext.grapheme_len();
                        let overflow = text_length - uri_length + 23;
                        let link_part = if overflow > 300 {
                            // Text will still be too long, shorten to the minimum of 23 characters.
                            uri.chars().take(22).collect::<String>()
                        } else {
                            uri.chars()
                                .take(300 - (text_length - uri_length) - 1)
                                .collect::<String>()
                        };
                        // Replace the link with a shortened version.
                        richtext.insert(facet.index.byte_start + link_part.len(), "…");
                        richtext.delete(
                            facet.index.byte_start + link_part.len() + "…".len(),
                            facet.index.byte_end + "…".len(),
                        );
                        // If the text is short enough we can stop already.
                        if richtext.grapheme_len() <= 300 {
                            return richtext;
                        }
                    }
                }
            }
        }
    }
    richtext
}

#[cfg(test)]
pub mod tests {
    use crate::bluesky_richtext::get_rich_text;

    // Test that short text should stay unchanged.
    #[test]
    fn test_short_text_unchanged() {
        let text = "Test toot with a link http://example.com/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let richtext = get_rich_text(text);
        assert_eq!(
            richtext.text,
            "Test toot with a link http://example.com/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }

    // Test URL shortening.
    #[test]
    fn test_shorten_url() {
        let text = "Test toot with long link http://example.com/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let richtext = get_rich_text(text);
        assert_eq!(
            richtext.text,
            "Test toot with long link http://example.com/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa…"
        );
    }

    #[test]
    // Test that only links starting with https:// or http:// are detected.
    fn test_link_detection() {
        let text = "♻️ bensaufley.com: This is awful from start to finish. The documentation of this guy's descent into hate is really chilling, to me. It's a story we seem to be seeing more and more, and to hear the personal side of this, from a warm and collaborative friend to this secret … villain … it's just so sad, and so scary.\n\n💬 lizthegrey.com:… https://mastodon.social/@klausi/113511471780554214";
        let richtext = get_rich_text(text);
        assert_eq!(
            richtext.text,
            "♻️ bensaufley.com: This is awful from start to finish. The documentation of this guy's descent into hate is really chilling, to me. It's a story we seem to be seeing more and more, and to hear the personal side of this, from a warm and collaborative friend to this secret … villain … it's just so sad, and so scary.\n\n💬 lizthegrey.com:… https://mastodon.socia…"
        );
    }
}
