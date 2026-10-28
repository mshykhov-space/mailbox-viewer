use time::OffsetDateTime;
use time::macros::format_description;

use crate::gmail::Message;
use crate::store::Share;
use crate::text::{escape_html, truncate};

const STYLE: &str = r#"
:root { color-scheme: light dark; }
* { box-sizing: border-box; }
body { margin: 0; background: #f4f5f7; color: #16181d; font: 15px/1.55 -apple-system, "Segoe UI", Roboto, sans-serif; }
main { max-width: 860px; margin: 0 auto; padding: 24px 16px 64px; }
header.page { display: flex; flex-wrap: wrap; gap: 8px 16px; align-items: baseline; padding: 16px 0 8px; border-bottom: 1px solid #d8dbe1; }
header.page h1 { margin: 0; font-size: 19px; }
header.page .alias { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 14px; color: #555c69; }
header.page .meta { margin-left: auto; font-size: 12px; color: #6b7280; }
.notice { margin: 16px 0; padding: 10px 12px; border-left: 3px solid #d97706; background: #fff7ed; font-size: 13px; }
article { margin: 16px 0; padding: 14px 16px; background: #fff; border: 1px solid #e1e4ea; border-radius: 8px; }
article .head { display: flex; flex-wrap: wrap; gap: 4px 12px; align-items: baseline; }
article .from { font-weight: 600; }
article .date { margin-left: auto; font-size: 12px; color: #6b7280; white-space: nowrap; }
article .subject { margin: 4px 0 10px; font-weight: 500; }
article pre { margin: 0; padding: 10px 12px; background: #f8f9fb; border-radius: 6px; font: 13px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; white-space: pre-wrap; overflow-wrap: anywhere; }
article .empty { color: #6b7280; font-size: 13px; }
.state { margin: 48px 0; text-align: center; }
.state h1 { font-size: 20px; margin: 0 0 8px; }
.state p { margin: 0; color: #555c69; }
@media (prefers-color-scheme: dark) {
  body { background: #14161a; color: #e6e8ec; }
  header.page { border-color: #2a2f37; }
  header.page .alias, header.page .meta, article .date, article .empty { color: #9aa3b2; }
  article { background: #1b1e24; border-color: #2a2f37; }
  article pre { background: #12151a; }
  .notice { background: #2a2013; }
}
"#;

pub fn layout(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<meta name=\"robots\" content=\"noindex, nofollow, noarchive\">\
<title>{title}</title><style>{STYLE}</style></head><body><main>{body}</main></body></html>",
        title = escape_html(title)
    )
}

pub fn inbox_page(share: &Share, messages: &[Message], stale: bool) -> String {
    let mut cards = String::new();
    if messages.is_empty() {
        cards.push_str("<div class=\"state\"><h1>No messages</h1><p>Nothing has arrived at this address yet.</p></div>");
    }
    for message in messages {
        cards.push_str(&message_card(message));
    }
    let notice = if stale {
        "<p class=\"notice\">Gmail is unavailable right now. Showing the last saved messages.</p>"
    } else {
        ""
    };
    let label = label_or_alias(share);
    let title = if label.eq_ignore_ascii_case(&share.alias) {
        format!("<h1>{}</h1>", escape_html(label))
    } else {
        format!(
            "<h1>{}</h1><span class=\"alias\">{}</span>",
            escape_html(label),
            escape_html(&share.alias)
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

pub fn message_card(message: &Message) -> String {
    let body = if message.body.trim().is_empty() {
        "<p class=\"empty\">No text part in this message.</p>".to_string()
    } else {
        format!(
            "<pre>{}</pre>",
            escape_html(&truncate(&message.body, 20_000))
        )
    };
    format!(
        "<article><div class=\"head\"><span class=\"from\">{from}</span><span class=\"date\">{date}</span></div>\
<div class=\"subject\">{subject}</div>{body}</article>",
        from = escape_html(&fallback(&message.from, "Unknown sender")),
        date = escape_html(&format_time(message.received_at)),
        subject = escape_html(&fallback(&message.subject, "(no subject)"))
    )
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

fn label_or_alias(share: &Share) -> &str {
    if share.label.trim().is_empty() {
        &share.alias
    } else {
        &share.label
    }
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
