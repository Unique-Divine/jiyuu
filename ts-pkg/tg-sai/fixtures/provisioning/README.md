# Custody Provisioning Readback

`custody-live.json` contains sanitized Telegram readback from an authorized
throwaway supergroup on November 5, 2026. The custody account created the group.
The bot and required team admin were promoted, and both default ordinary members
joined. The worker initially reported an incomplete setup despite these successful
changes because Telegram returned the implicit admin permission `other: true`.
The original permission profiles expected `other: false`. This fixture preserves
that observed response so offline tests can catch the mismatch.

The snapshot was captured while that final audit failure was being investigated.
It proves membership and permissions at capture time. It does not prove that the
cloud setup had reached `ready`, that a webhook delivered, or that an optional
member invitation fallback was exercised. The one observed invite is Telegram's
default primary link, without approval or expiry. It is not a fallback invitation.

`captureProvisioningGolden.ts` reads the exact configured custody setup and exact
group ID. It checks account identity, group ownership, supergroup type, and the
creation marker before collecting evidence. Dialog enumeration only populates
the client cache and accepts an exact numeric ID match. No title matching or
Telegram mutation is used. Member lookups resolve pinned default-member handles
and verify their numeric identities before reading membership.

The sanitizer copies only fields consumed by the provisioning adapter. Account
IDs, handles, titles, descriptions, creation markers, raw objects, credentials,
and usable invite links are omitted. Role labels replace participant identities.
Invite approval, revocation, expiry, and usage properties remain available for
comparison. A temporary session store is removed after capture.

The tests pass captured member shapes through the actual runtime normalizer,
compare every configured permission, and explicitly reject extra bot powers.
Synthetic tests separately cover absent and restricted members because those
states did not occur in this live snapshot.
