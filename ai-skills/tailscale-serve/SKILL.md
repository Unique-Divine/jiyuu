---
name: tailscale-serve
description: >-
  Troubleshoot, document, and operate Tailscale Serve for local development
  previews, HTTPS `.ts.net` URLs, MagicDNS, admin-console HTTPS certificate setup,
  WSL or localhost proxy targets, phone testing over a tailnet, and curl/OpenSSL
  diagnosis of Serve TLS failures. Use when a user says Tailscale Serve, `tailscale
  serve`, `.ts.net`, MagicDNS, HTTPS certs, SSL protocol error, TLS alert, Funnel
  versus Serve, localhost preview, Next.js preview over Tailscale, or sharing a dev
  server with a phone or another tailnet device.
---

# Tailscale Serve

Use this skill when helping a user expose a local HTTP service to devices in
their tailnet through command `tailscale serve`. The common mental model is:

1. The browser opens the HTTPS `.ts.net` URL.
2. Tailscale Serve accepts HTTPS on the serving machine, usually on port `443`.
3. Tailscale Serve terminates TLS with a certificate for the MagicDNS hostname.
4. Tailscale Serve proxies the request to the local target, such as
   `http://127.0.0.1:3000`.

The target URL is local to the machine running command `tailscale serve`; it is
not the URL that the phone or remote device should open.

## Machine administration

Manage tailnet machines at:

```text
https://login.tailscale.com/admin/machines
```

To open it from WSL, use `wslview`, not macOS's `open` command:

```shell
wslview 'https://login.tailscale.com/admin/machines'
```

To revoke a machine's access, use its `...` menu and choose **Remove**.

## Admin console prerequisites

Check these before spending time on the web app:

- The serving machine and client device are in the same tailnet.
- Tailscale is connected on both devices.
- The Tailscale admin console has MagicDNS enabled.
- The Tailscale admin console has HTTPS Certificates enabled under the DNS
  page.
- The user understands the public-ledger implication: enabling HTTPS
  certificates publishes the machine name and tailnet DNS name through the
  certificate transparency system.
- ACLs allow the client device or user to reach the serving machine.

Admin console path:

```shell
Admin Console -> DNS -> MagicDNS
Admin Console -> DNS -> HTTPS Certificates -> Enable HTTPS
```

If the user is not a tailnet admin or owner, ask them to have an admin enable
the HTTPS certificate setting. Without that setting, Serve can still store an
HTTPS config, but the HTTPS endpoint can fail during the TLS handshake.

## Standard serve flow

Use this flow for a local HTTP dev server on port `3000`.

```shell
curl -v http://127.0.0.1:3000/

tailscale status
tailscale ip -4

tailscale serve reset
tailscale serve --bg --https=443 http://127.0.0.1:3000
tailscale serve status
```

Open the exact HTTPS URL from command `tailscale serve status`, such as:

```text
https://machine-name.tailnet-name.ts.net/
```

Do not open `https://127.0.0.1:3000` or `https://machine-name.ts.net:3000`
unless the local service itself speaks HTTPS on port `3000`. Development
servers like Next.js usually speak plain HTTP on port `3000`.

Short syntax also works in recent Tailscale versions:

```shell
tailscale serve 3000
tailscale serve http://localhost:3000
tailscale serve http://127.0.0.1:3000
```

Prefer the explicit `--https=443` form in notes and debugging because it makes
the listener and proxy target obvious.

## Certificate checks

When the HTTPS URL fails before any HTTP response appears, check certificate
support directly.

```shell
tailscale cert "$(tailscale status --json | jq -r '.Self.DNSName' | sed 's/[.]$//')"
```

Or test a known hostname:

```shell
tailscale cert machine-name.tailnet-name.ts.net
```

To avoid writing cert files while checking support, use temporary output paths:

```shell
tailscale cert \
  --cert-file=/tmp/ts-serve-test.crt \
  --key-file=/tmp/ts-serve-test.key \
  machine-name.tailnet-name.ts.net
```

Important failure:

```text
500 Internal Server Error: your Tailscale account does not support getting TLS certs
```

This means the HTTPS Serve URL cannot complete TLS. Fix the tailnet certificate
setting or account support before debugging the app.

## Curl and OpenSSL diagnosis

Compare the local target with the Serve frontend.

```shell
curl -v http://127.0.0.1:3000/
curl -vk https://machine-name.tailnet-name.ts.net/
openssl s_client \
  -connect machine-name.tailnet-name.ts.net:443 \
  -servername machine-name.tailnet-name.ts.net \
  -brief
```

Interpretation:

- Local `curl` returns `200`, but HTTPS `curl` fails during TLS: Tailscale
  Serve or certificate setup is the problem, not the web app.
- HTTPS `curl` reaches HTTP and returns `502` or a proxy error: Serve is active,
  but the proxy target is wrong or down.
- HTTPS `curl` returns app HTML, but the browser page is broken: inspect app
  URLs, CORS/dev-origin config, WebSocket URLs, and absolute `localhost` links.
- OpenSSL error `tlsv1 alert internal error` or browser error
  `ERR_SSL_PROTOCOL_ERROR` can happen when Serve is configured for HTTPS but the
  tailnet cannot issue or load the certificate.

Also check DNS resolution:

```shell
getent hosts machine-name.tailnet-name.ts.net
tailscale status
```

The hostname should resolve to the serving machine's Tailscale IP, usually in
the `100.64.0.0/10` CGNAT range.

## WSL and localhost

Be precise about where command `tailscale serve` runs.

- If Tailscale and the dev server both run inside WSL, target
  `http://127.0.0.1:3000` usually means the WSL dev server.
- If Tailscale runs on Windows and the dev server runs inside WSL, target
  `http://127.0.0.1:3000` means Windows localhost, not WSL localhost.
- If the local target is not reachable from the same OS/network namespace as
  command `tailscale serve`, Serve cannot proxy it.

For Next.js in WSL, bind the dev server to all interfaces when needed:

```shell
next dev -H 0.0.0.0 -p 3000
```

Then test candidate targets from the same shell where command
`tailscale serve` runs:

```shell
curl -v http://127.0.0.1:3000/
curl -v http://localhost:3000/
curl -v http://10.255.255.254:3000/
```

Use the target that works from the Tailscale Serve process.

## Next.js development previews

For Next.js, separate three issues:

- The local HTTP target works if command `curl -v http://127.0.0.1:3000/`
  returns HTML or a useful HTTP response.
- The Serve HTTPS frontend works if command
  `curl -vk https://machine.tailnet.ts.net/` completes TLS and returns HTTP.
- The browser app works if all follow-up asset, data, WebSocket, and API
  requests use reachable URLs.

Warning text like this is not the same as a TLS failure:

```text
Cross origin request detected from 127.0.0.1 to /_next/* resource.
```

If the page loads over HTTPS but Next.js dev assets or data requests are blocked,
add the `.ts.net` origin to the Next.js dev-origin configuration when the
project supports it:

```ts
const nextConfig = {
  allowedDevOrigins: ["https://machine-name.tailnet-name.ts.net"],
}
```

If the browser reports an SSL protocol error before the page loads, focus on
Tailscale HTTPS certificates and Serve status instead.

## Phone testing checklist

Use this checklist when the goal is to open a local preview from a phone:

1. Connect the phone to the same Tailscale tailnet.
2. Confirm the phone is online in command `tailscale status` or in the admin
   console.
3. Open only the HTTPS `.ts.net` URL from command `tailscale serve status`.
4. Do not open a `127.0.0.1`, `localhost`, or LAN IP URL on the phone unless the
   phone is supposed to reach a service on itself or the LAN.
5. If the HTTPS URL fails on both the phone and the serving machine, test
   command `tailscale cert` before debugging the app.

## Serve versus Funnel

Use command `tailscale serve` for tailnet-only access. Use command
`tailscale funnel` only when the user intentionally wants to expose the service
to the public internet.

Serve is the right default for local previews shared with a phone or coworker
that is already connected to the same tailnet.

## Report format

When answering a troubleshooting request, keep the report short and structured:

```markdown
## Diagnosis
<one or two sentences about which hop is failing>

## Evidence
- Local target: <curl result>
- Serve frontend: <curl/OpenSSL result>
- Certificate check: <tailscale cert result>

## Fix
<admin console or command steps>

## Verify
<commands and expected result>
```

Lead with the failing hop. Avoid repeating setup commands when evidence shows a
specific blocker such as missing HTTPS certificate support.
