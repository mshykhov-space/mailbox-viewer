# Mailbox Viewer

Public, revocable, read-only view of one mailbox alias. The service reads a single Gmail
mailbox over the Gmail API, filters messages by the alias in the `To` header, and shows
them as plain text behind a secret link.

Typical use: many iCloud aliases forward to one Gmail account. A share link exposes only
the messages addressed to one alias. Revoking the share closes the link immediately.

## How it works

- Gmail API scope `gmail.readonly`, one refresh token, one mailbox.
- A message belongs to an alias when `to:"<alias>"` matches. iCloud forwarding keeps the
  original `To` header, so the receiving Gmail sees the alias.
- Each share stores `sha256(token)`; the raw token exists only in the returned URL.
- Public page shows subject, sender, date and the plain-text body. HTML is stripped; the
  page never renders remote markup.

## Run locally

```sh
cp .env.example .env
cargo run
```

Create a share:

```sh
curl -X POST localhost:8080/api/shares \
  -H "x-admin-key: $ADMIN_API_KEY" \
  -H 'content-type: application/json' \
  -d '{"alias":"example@icloud.com","label":"Steam","expires_in_days":7}'
```

Open the returned `url`.

## Admin API

All admin routes require the `x-admin-key` header.

| Method | Path | Purpose |
| --- | --- | --- |
| `POST` | `/api/shares` | Create a share. Body: `alias`, optional `label`, optional `expires_in_days` (1-3650). Returns `token` and `url`. |
| `GET` | `/api/shares` | List shares. Tokens are never returned. |
| `POST` | `/api/shares/{id}/rotate` | Issue a new token. The old link stops working. |
| `DELETE` | `/api/shares/{id}` | Revoke the share. The link answers `410 Gone`. |

`GET /health` is public.

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `ADMIN_API_KEY` | required, 24+ chars | Admin key. |
| `GOOGLE_CLIENT_ID` | required | OAuth client. |
| `GOOGLE_CLIENT_SECRET` | required | OAuth secret. |
| `GOOGLE_REFRESH_TOKEN` | required | Token for the mailbox. |
| `HTTP_PORT` | `8080` | Listen port. |
| `STATE_DB_PATH` | `/state/mailbox-viewer.db` | SQLite state. |
| `PUBLIC_BASE_URL` | `http://localhost:8080` | Prefix for returned links. |
| `CACHE_TTL_SECONDS` | `30` | Gmail result cache. |
| `MESSAGE_LIMIT` | `25` | Messages per alias and page. |

## Deploy

Deployment lives in `smhomelab-deploy` as the Helm chart `services/mailbox-viewer`.
Argo CD creates the application from `values-<env>.yaml`. Secrets come from Doppler.
