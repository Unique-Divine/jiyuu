# Test prompts for `tailscale-serve`

Use these prompts to review whether the skill gives practical, evidence-driven
Tailscale Serve help.

## Test 1: HTTPS fails but localhost works

Prompt:

```text
I'm running Next on http://127.0.0.1:3000 and `tailscale serve status` shows
https://devbox.example-tailnet.ts.net -> http://127.0.0.1:3000, but Chrome says
ERR_SSL_PROTOCOL_ERROR for the `.ts.net` URL. `curl http://127.0.0.1:3000`
returns 200. What should I check?
```

Expected result:

- Separates local HTTP target from Serve HTTPS frontend.
- Suggests command `tailscale cert` and admin-console HTTPS certificate checks.
- Uses command `curl -vk` or `openssl s_client` to verify the TLS failure.
- Does not blame Next.js before checking Tailscale cert support.

## Test 2: Phone cannot open preview

Prompt:

```text
I want to open my local Vite app from my iPhone through Tailscale Serve. My app
is on port 5173 on my WSL machine. The phone is on Tailscale but not on my home
wifi. What exact flow should I use?
```

Expected result:

- Explains that the phone opens the HTTPS `.ts.net` URL, not `localhost`.
- Checks that the phone and serving machine are in the same tailnet.
- Handles WSL/Windows localhost ambiguity.
- Provides `curl`, `tailscale serve`, and `tailscale serve status` commands.

## Test 3: App loads but assets break

Prompt:

```text
My Tailscale Serve URL opens the page, but the JS chunks and websocket calls are
still going to localhost:3000 and failing on my other laptop. How do I debug
this?
```

Expected result:

- Identifies this as an app/browser URL problem after Serve is working.
- Suggests checking absolute URLs, WebSocket origins, and dev-origin settings.
- Does not focus on certificate setup once HTTPS page load is confirmed.
