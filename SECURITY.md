# Security policy

## Reporting a vulnerability

Please report privately first, to **hello@saphojuice.com**, or through GitHub's
[private vulnerability reporting](https://github.com/saphojuice/speedcheck/security/advisories/new)
on this repository. Do not open a public issue for a security problem.

Include what you did, what happened, and what you expected. A proof of concept helps but is not
required. If you would like credit, say so and how you want to be named.

**Response times.** First reply within 3 working days. An assessment, with either a fix plan or a
reason we disagree, within 14 days. If a fix ships, the advisory is published with it.

There is no bug bounty. This is a small project and paying properly is not something it can
currently promise, so it does not promise it.

## In scope

- **The probe**, `speedcheck/` in this repository: the binary that measures the machine.
- **The install scripts**, `scripts/check` and `scripts/check.ps1`, and the double-click
  launchers. The download, hash verification and cleanup path especially.
- **The browser probe**, `browser/fit.js`.
- **The endpoints on saphojuice.com** that this code talks to: `/v1/receipt`, `/v1/delete`,
  `/v1/contact`, `/v1/og`, and the share pages at `/r/<id>`.

Things we would particularly like to hear about:

- A way to make the install scripts run something whose hash was not verified.
- A way to read, alter or delete someone else's result.
- A way to get stored content to execute in another visitor's browser.
- A way to poison the public aggregate with fabricated measurements.
- A way to recover a person or a device from data that is meant to be class-level only.

## Out of scope

- Anything about the models themselves. They come from their own publishers under their own
  licenses; we link to them and estimate their speed.
- Denial of service by volume. The endpoints are rate limited; if you find a way past the limits
  cheaply, that *is* in scope, but simply sending a lot of traffic is not.
- Reports from automated scanners with no demonstrated impact.
- Missing headers or configuration on a page with no data and no state, unless you can show what
  it leads to.

## What the design already assumes

Stated so you can attack the right things rather than rediscover the intent:

- Receipts contain no personal data by design. No names, no accounts, nothing typed, no serials,
  no location. IP addresses are hashed with a secret for rate limiting and never stored with a
  result.
- `class_hash` is **public** and shared by every machine of the same model. It is deliberately not
  accepted as proof of ownership anywhere.
- Deleting a result requires the `delete_token` returned once at submission, or the submitter's
  own `install_id`.
- Receipts that fail a physics plausibility check, or that were measured in degraded conditions,
  are kept but excluded from every public aggregate.
- The published binaries are built by GitHub Actions from this repository, with build provenance
  attestations. A binary whose attestation does not verify did not come from this source.
