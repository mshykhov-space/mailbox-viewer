const BLOCK_TAGS: [&str; 8] = ["p", "div", "tr", "li", "h1", "h2", "h3", "blockquote"];

pub fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Converts an HTML part to readable plain text. Formatting is dropped on purpose:
/// the public page must never render remote markup or trackers.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut chars = html.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '<' {
            out.push(ch);
            continue;
        }
        let mut tag = String::new();
        for next in chars.by_ref() {
            if next == '>' {
                break;
            }
            tag.push(next);
        }
        let closing = tag.trim_start().starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect();
        match name.as_str() {
            "br" => out.push('\n'),
            "style" | "script" | "head" | "title" => skip_until_close(&mut chars, &name),
            other if !closing && BLOCK_TAGS.contains(&other) => out.push('\n'),
            _ => {}
        }
    }
    collapse_blank_lines(&decode_entities(&out))
}

fn skip_until_close(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, name: &str) {
    let needle = format!("</{name}");
    let mut window = String::new();
    for ch in chars.by_ref() {
        window.push(ch);
        if window.len() > needle.len() {
            window.remove(0);
        }
        if window.eq_ignore_ascii_case(&needle) {
            for next in chars.by_ref() {
                if next == '>' {
                    break;
                }
            }
            return;
        }
    }
}

fn decode_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        let parsed = [
            ("&nbsp;", " "),
            ("&amp;", "&"),
            ("&lt;", "<"),
            ("&gt;", ">"),
            ("&quot;", "\""),
            ("&#39;", "'"),
            ("&apos;", "'"),
        ]
        .iter()
        .find_map(|(entity, replacement)| {
            tail.starts_with(entity).then_some((*entity, *replacement))
        });
        match parsed {
            Some((entity, replacement)) => {
                out.push_str(replacement);
                rest = &tail[entity.len()..];
            }
            None => match numeric_entity(tail) {
                Some((consumed, decoded)) => {
                    out.push(decoded);
                    rest = &tail[consumed..];
                }
                None => {
                    out.push('&');
                    rest = &tail[1..];
                }
            },
        }
    }
    out.push_str(rest);
    out
}

fn numeric_entity(tail: &str) -> Option<(usize, char)> {
    let rest = tail.strip_prefix("&#")?;
    let end = rest.find(';')?;
    let digits = &rest[..end];
    let value = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse::<u32>().ok()?,
    };
    let ch = char::from_u32(value)?;
    Some((end + 3, ch))
}

fn collapse_blank_lines(input: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for line in input.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() && lines.last().is_some_and(|last| last.trim().is_empty()) {
            continue;
        }
        lines.push(trimmed);
    }
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

pub fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let mut out: String = value.chars().take(limit).collect();
    out.push_str("\n[...]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_markup_and_keeps_text() {
        let html = "<html><head><style>body{}</style></head><body><p>Hello</p><div>World &amp; co</div><script>evil()</script></body></html>";
        assert_eq!(html_to_text(html), "Hello\nWorld & co");
    }

    #[test]
    fn breaks_lines_on_br() {
        assert_eq!(html_to_text("a<br>b<br/>c"), "a\nb\nc");
    }

    #[test]
    fn decodes_numeric_entities() {
        assert_eq!(html_to_text("a&#8199;b&#x41;c"), "a\u{2007}bAc");
    }

    #[test]
    fn escapes_markup() {
        assert_eq!(escape_html("<b>&'\""), "&lt;b&gt;&amp;&#39;&quot;");
    }

    #[test]
    fn truncates_long_bodies() {
        assert_eq!(truncate("abcdef", 3), "abc\n[...]");
        assert_eq!(truncate("abc", 3), "abc");
    }
}
