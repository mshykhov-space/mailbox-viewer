use time::OffsetDateTime;
use time::macros::format_description;

use crate::gmail::Message;
use crate::store::Share;
use crate::text::escape_html;

const STYLE: &str = r#"
:root { color-scheme: light dark; }
* { box-sizing: border-box; }
body { margin: 0; background: #f4f5f7; color: #16181d; font: 15px/1.55 -apple-system, "Segoe UI", Roboto, sans-serif; }
main { max-width: 860px; margin: 0 auto; padding: 24px 16px 64px; }
header.page { display: flex; flex-wrap: wrap; gap: 8px 16px; align-items: baseline; padding: 16px 0 8px; border-bottom: 1px solid #d8dbe1; }
header.page h1 { margin: 0; font-size: 19px; }
header.page .alias { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 18px; }
header.page .label { padding: 2px 8px; border: 1px solid #d8dbe1; border-radius: 4px; font-size: 12px; color: #555c69; }
header.page .meta { margin-left: auto; font-size: 12px; color: #6b7280; }
.notice { margin: 16px 0; padding: 10px 12px; border-left: 3px solid #d97706; background: #fff7ed; font-size: 13px; }
article { margin: 16px 0; padding: 14px 16px; background: #fff; border: 1px solid #e1e4ea; border-radius: 8px; }
article .head { display: flex; flex-wrap: wrap; gap: 4px 12px; align-items: baseline; }
article .from { font-weight: 600; }
article .date { margin-left: auto; font-size: 12px; color: #6b7280; white-space: nowrap; }
article .subject { margin: 4px 0 10px; font-weight: 500; }
article pre { margin: 0; padding: 10px 12px; background: #f8f9fb; border-radius: 6px; font: 13px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; white-space: pre-wrap; overflow-wrap: anywhere; }
article .empty { color: #6b7280; font-size: 13px; }
article .code { margin-left: 10px; padding: 2px 8px; border-radius: 4px; background: #dbeafe; color: #1e3a8a; font: 600 13px/1.4 ui-monospace, SFMono-Regular, Menlo, monospace; letter-spacing: 1px; }
article .body > summary, article .more > summary { cursor: pointer; font-size: 12px; color: #6b7280; margin: 2px 0 8px; }
article .more { margin-top: 8px; }
.state { margin: 48px 0; text-align: center; }
.state h1 { font-size: 20px; margin: 0 0 8px; }
.state p { margin: 0; color: #555c69; }
@media (prefers-color-scheme: dark) {
  body { background: #14161a; color: #e6e8ec; }
  header.page { border-color: #2a2f37; }
  header.page .alias, header.page .meta, article .date, article .empty { color: #9aa3b2; }
  header.page .label { border-color: #2a2f37; color: #9aa3b2; }
  article { background: #1b1e24; border-color: #2a2f37; }
  article pre { background: #12151a; }
  article .code { background: #1e3a8a; color: #dbeafe; }
  .notice { background: #2a2013; }
}
"#;

pub fn layout(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<meta name=\"robots\" content=\"noindex, nofollow, noarchive\">\
<meta http-equiv=\"refresh\" content=\"30\">\
<title>{title}</title><style>{STYLE}</style></head><body><main>{body}</main></body></html>",
        title = escape_html(title)
    )
}

pub fn inbox_page(share: &Share, messages: &[Message], stale: bool) -> String {
    let mut cards = String::new();
    if messages.is_empty() {
        cards.push_str("<div class=\"state\"><h1>No messages</h1><p>Nothing has arrived at this address yet.</p></div>");
    }
    for (index, message) in messages.iter().enumerate() {
        cards.push_str(&message_card(message, index == 0));
    }
    let notice = if stale {
        "<p class=\"notice\">Gmail is unavailable right now. Showing the last saved messages.</p>"
    } else {
        ""
    };
    let alias = share.alias.trim();
    let label = share.label.trim();
    let title = if label.is_empty() || label.eq_ignore_ascii_case(alias) {
        format!("<h1 class=\"alias\">{}</h1>", escape_html(alias))
    } else {
        format!(
            "<h1 class=\"alias\">{}</h1><span class=\"label\">{}</span>",
            escape_html(alias),
            escape_html(label)
        )
    };
    let header = format!(
        "<header class=\"page\">{title}\
<span class=\"meta\">read only · {count} messages</span></header>{notice}",
        count = messages.len()
    );
    layout(
        &format!("Mailbox {}", share.alias),
        &format!("{header}{cards}"),
    )
}

pub fn message_card(message: &Message, expanded: bool) -> String {
    let from = escape_html(&fallback(&message.from, "Unknown sender"));
    let date = escape_html(&format_time(message.received_at));
    let subject = escape_html(&fallback(&message.subject, "(no subject)"));
    let code = extract_code(&message.body)
        .map(|code| {
            format!(
                "<span class=\"code\" title=\"Verification code\">{}</span>",
                escape_html(&code)
            )
        })
        .unwrap_or_default();
    let body = if message.body.trim().is_empty() {
        "<p class=\"empty\">No text part in this message.</p>".to_string()
    } else {
        let (preview, rest) = split_preview(&message.body);
        let mut html = format!("<pre>{}</pre>", escape_html(&preview));
        if let Some(rest) = rest {
            html.push_str(&format!(
                "<details class=\"more\"><summary>Show the rest of the message</summary><pre>{}</pre></details>",
                escape_html(&rest)
            ));
        }
        html
    };
    let open = if expanded { " open" } else { "" };
    format!(
        "<article><div class=\"head\"><span class=\"from\">{from}</span><span class=\"date\">{date}</span></div>\
<div class=\"subject\">{subject}{code}</div>\
<details class=\"body\"{open}><summary>Message text</summary>{body}</details></article>"
    )
}

fn split_preview(body: &str) -> (String, Option<String>) {
    const PREVIEW_LINES: usize = 12;
    const PREVIEW_CHARS: usize = 700;
    let lines: Vec<&str> = body.lines().collect();
    if lines.len() > PREVIEW_LINES || body.chars().count() > PREVIEW_CHARS {
        let mut cut = lines
            .iter()
            .take(PREVIEW_LINES)
            .copied()
            .collect::<Vec<_>>()
            .join("\n");
        if cut.chars().count() > PREVIEW_CHARS {
            cut = cut.chars().take(PREVIEW_CHARS).collect();
        }
        let rest = body
            .char_indices()
            .nth(cut.chars().count())
            .map(|(index, _)| body[index..].to_string())
            .unwrap_or_default();
        (cut, Some(rest))
    } else {
        (body.to_string(), None)
    }
}

fn extract_code(body: &str) -> Option<String> {
    let lines: Vec<&str> = body.lines().map(str::trim).collect();
    let context = |line: &str| {
        let lower = line.to_ascii_lowercase();
        lower.contains("code") || lower.contains("verification") || lower.contains("confirm")
    };
    for (index, line) in lines.iter().enumerate() {
        let Some(token) = standalone_token(line) else {
            continue;
        };
        let near_context = context(line)
            || index
                .checked_sub(1)
                .is_some_and(|prev| context(lines[prev]));
        if near_context && is_code(token) {
            return Some(token.to_string());
        }
    }
    for line in &lines {
        if !context(line) {
            continue;
        }
        for token in tokens(line) {
            if is_code(token) {
                return Some(token.to_string());
            }
        }
    }
    None
}

fn tokens(line: &str) -> Vec<&str> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_alphanumeric() {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_alphanumeric() {
            index += 1;
        }
        let before = start.checked_sub(1).map(|pos| bytes[pos] as char);
        let after = bytes.get(index).map(|byte| *byte as char);
        let glued = |value: Option<char>| matches!(value, Some('#' | '@' | '_' | '.'));
        if !glued(before) && !glued(after) {
            out.push(&line[start..index]);
        }
    }
    out
}

fn standalone_token(line: &str) -> Option<&str> {
    let tokens = tokens(line);
    (tokens.len() == 1).then(|| tokens[0])
}

fn is_code(token: &str) -> bool {
    let digits = token.chars().all(|ch| ch.is_ascii_digit());
    if digits {
        return (4..=8).contains(&token.len());
    }
    let has_digit = token.chars().any(|ch| ch.is_ascii_digit());
    let upper = token
        .chars()
        .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit());
    upper && has_digit && (5..=8).contains(&token.len())
}

pub fn state_page(title: &str, text: &str) -> String {
    layout(
        title,
        &format!(
            "<div class=\"state\"><h1>{}</h1><p>{}</p></div>",
            escape_html(title),
            escape_html(text)
        ),
    )
}

fn fallback(value: &str, replacement: &str) -> String {
    if value.trim().is_empty() {
        replacement.to_string()
    } else {
        value.to_string()
    }
}

fn format_time(timestamp: i64) -> String {
    const FORMAT: &[time::format_description::BorrowedFormatItem<'_>] =
        format_description!("[year]-[month]-[day] [hour]:[minute] UTC");
    OffsetDateTime::from_unix_timestamp(timestamp)
        .ok()
        .and_then(|moment| moment.format(&FORMAT).ok())
        .unwrap_or_else(|| timestamp.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn share() -> Share {
        Share {
            id: "1".into(),
            label: String::new(),
            alias: "a@icloud.com".into(),
            created_at: 0,
            revoked_at: None,
            expires_at: None,
            last_viewed_at: None,
            view_count: 0,
        }
    }

    #[test]
    fn renders_plain_text_and_escapes_markup() {
        let message = Message {
            id: "1".into(),
            thread_id: "t".into(),
            from: "<script>x</script>".into(),
            subject: "Code".into(),
            received_at: 0,
            body: "<b>123456</b>".into(),
        };
        let html = inbox_page(&share(), &[message], false);
        assert!(html.contains("&lt;b&gt;123456&lt;/b&gt;"));
        assert!(!html.contains("<script>x</script>"));
        assert!(html.contains("a@icloud.com"));
    }

    #[test]
    fn shows_empty_state() {
        let html = inbox_page(&share(), &[], false);
        assert!(html.contains("No messages"));
    }
}
