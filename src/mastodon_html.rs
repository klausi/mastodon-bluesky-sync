use ego_tree::NodeRef;
use scraper::{ElementRef, Html, Node};

// Parse Mastodon HTML to plain text and collect quote-inline link if present.
pub(crate) fn parse_html_and_extract_inline_quote(html: &str) -> (String, Option<String>) {
    let fragment = Html::parse_fragment(html);
    let mut output = String::new();
    let mut inline_quote_link = None;

    for child in fragment.tree.root().children() {
        append_html_text(child, &mut output, &mut inline_quote_link);
    }

    (output, inline_quote_link)
}

// Returns true if an element has the given CSS class name.
fn element_has_class(element: &ElementRef<'_>, class_name: &str) -> bool {
    element
        .attr("class")
        .is_some_and(|classes| classes.split_whitespace().any(|class| class == class_name))
}

// Find first anchor href in a node subtree.
fn find_first_anchor_href(node: NodeRef<'_, Node>) -> Option<String> {
    if let Some(element) = ElementRef::wrap(node)
        && element.value().name() == "a"
    {
        return element
            .attr("href")
            .map(|href| html_escape::decode_html_entities(href).trim().to_string());
    }

    for child in node.children() {
        if let Some(href) = find_first_anchor_href(child) {
            return Some(href);
        }
    }
    None
}

// Check if an element contains any descendants with class="invisible".
fn element_contains_invisible_spans(element: &ElementRef<'_>) -> bool {
    if element_has_class(element, "invisible") {
        return true;
    }

    for child in element.children() {
        if let Some(child_element) = ElementRef::wrap(child)
            && element_contains_invisible_spans(&child_element)
        {
            return true;
        }
    }
    false
}

// Walk an HTML node tree and write plain text while preserving line breaks.
// Also skips quote-inline marker paragraphs and captures their first link.
// Skips spans marked with class="invisible" used by Mastodon for link display formatting.
fn append_html_text(
    node: NodeRef<'_, Node>,
    output: &mut String,
    inline_quote_link: &mut Option<String>,
) {
    if let Some(element) = ElementRef::wrap(node) {
        if element.value().name() == "p" && element_has_class(&element, "quote-inline") {
            if inline_quote_link.is_none() {
                *inline_quote_link = find_first_anchor_href(node);
            }
            return;
        }
        if element_has_class(&element, "invisible") {
            return;
        }
        match element.value().name() {
            "br" => {
                output.push('\n');
            }
            "p" => {
                if node
                    .prev_sibling()
                    .and_then(ElementRef::wrap)
                    .is_some_and(|previous| previous.value().name() == "p")
                {
                    output.push_str("\n\n");
                }
                for child in node.children() {
                    append_html_text(child, output, inline_quote_link);
                }
            }
            "a" => {
                output.push_str(&anchor_text(element));
            }
            _ => {
                for child in node.children() {
                    append_html_text(child, output, inline_quote_link);
                }
            }
        }
        return;
    }

    if let Node::Text(text) = node.value() {
        output.push_str(text);
    }
}

// Convert an anchor element to plain text and preserve external links as tags.
fn anchor_text(anchor: ElementRef<'_>) -> String {
    let mut text = String::new();
    let mut ignored_inline_quote_link = None;
    for child in anchor.children() {
        append_html_text(child, &mut text, &mut ignored_inline_quote_link);
    }

    let text = html_escape::decode_html_entities(&text).to_string();
    let mut text_trimmed = text.trim().to_string();

    // If the anchor contains invisible spans, append ellipsis to indicate truncation
    if element_contains_invisible_spans(&anchor) {
        text_trimmed.push('…');
    }

    let href = anchor
        .attr("href")
        .map(|value| html_escape::decode_html_entities(value).trim().to_string())
        .unwrap_or_default();

    if text_trimmed.is_empty() && !text_trimmed.contains('…') {
        return href;
    }

    let class = anchor
        .attr("class")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    let is_mention_or_hashtag = class.contains("mention") || class.contains("hashtag");
    let is_external_link = href.starts_with("http://") || href.starts_with("https://");

    if is_external_link && !is_mention_or_hashtag {
        format!("<a href=\"{href}\">{}</a>", text_trimmed)
    } else {
        text_trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::parse_html_and_extract_inline_quote;

    #[test]
    fn mastodon_html_link_appends_href_after_link_text() {
        let html = "<p>Read <a href=\"https://example.com/path?x=1&amp;y=2\" rel=\"nofollow\">this article</a> now.</p>";
        assert_eq!(
            parse_html_and_extract_inline_quote(html).0,
            "Read <a href=\"https://example.com/path?x=1&y=2\">this article</a> now."
        );
    }

    #[test]
    fn mastodon_html_mention_link_does_not_append_href() {
        let html = "<p>Hello <span class=\"h-card\"><a href=\"https://hachyderm.io/@mekkaokereke\" class=\"u-url mention\">@<span>mekkaokereke</span></a></span></p>";
        assert_eq!(
            parse_html_and_extract_inline_quote(html).0,
            "Hello @mekkaokereke"
        );
    }

    #[test]
    fn mastodon_html_link_with_invisible_spans() {
        let html = "<p>Check this <a href=\"https://example.com/path\"><span class=\"invisible\">https://</span><span class=\"ellipsis\">example.com/</span><span class=\"invisible\">path</span></a> out.</p>";
        assert_eq!(
            parse_html_and_extract_inline_quote(html).0,
            "Check this <a href=\"https://example.com/path\">example.com/…</a> out."
        );
    }
}
