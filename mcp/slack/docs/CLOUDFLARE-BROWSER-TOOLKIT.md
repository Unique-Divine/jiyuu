# Cloudflare Browser Toolkit

Use Cloudflare Browser Rendering from this repo to run resilient page checks and captures when local browser automation is blocked.

## Prerequisites

- `CLOUDFLARE_ACCOUNT_ID` set
- `CLOUDFLARE_API_TOKEN` **or** `CF_TERRAFORM_TOKEN` set
- Token needs Browser Rendering access
- Account API tokens are supported (recommended for this workflow)

## Verify Access

```bash
npm run cf:browser -- verify
```

The verifier checks account-token auth (`/accounts/{account_id}/tokens/verify`) first and falls back to user-token verification automatically.

## Quick Commands

Rendered HTML:

```bash
npm run cf:browser -- content "https://example.com"
```

Markdown extract:

```bash
npm run cf:browser -- markdown "https://example.com"
```

Link extraction:

```bash
npm run cf:browser -- links "https://example.com"
```

Selector scrape:

```bash
npm run cf:browser -- scrape "https://example.com" --selectors "h1,.card a"
```

Screenshot:

```bash
npm run cf:browser -- screenshot "https://example.com" --out ./tmp/example.png --type png
```

PDF:

```bash
npm run cf:browser -- pdf "https://example.com" --out ./tmp/example.pdf
```

Structured JSON:

```bash
npm run cf:browser -- json "https://example.com" --schema '{"title":"string","links":["string"]}'
```

## Notes

- This toolkit is for page rendering, extraction, and evidence capture.
- It does not manage third-party account logins for posting workflows.
