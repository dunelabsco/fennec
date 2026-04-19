---
name: healthcheck
description: Probe URLs for status, latency, and basic content checks. Use when the user wants to know whether a site / API / service is up, how fast it responds, or to monitor on a schedule.
always: false
---

# healthcheck

No dedicated tool; this is a pattern using `http_request` + `cron` + `memory_store`.

## One-shot check

```
http_request(method: "GET", url: "https://example.com/")
```

Report back:
- HTTP status code.
- Response latency (tool returns this or shell `time curl ... -o /dev/null -w '%{time_total}s'` gives it).
- Content check: presence of an expected string (e.g. "OK" on `/health`), or a JSON field equals an expected value.

## Scheduled monitoring

Pair with `cron`:

```
cron(schedule: "*/5 * * * *", prompt: "Healthcheck https://example.com/health; if status != 200 or latency > 2s, alert me.")
```

The cron origin routes the alert back to the user's chat.

## What to check

Pick the tightest check that still catches real problems:

| Level | Check | When |
|---|---|---|
| Reachability | HTTP 2xx response at all | Monitoring a domain is alive |
| Health endpoint | `/health` or `/healthz` returns `"ok"` / `{"status":"healthy"}` | Service publishes one |
| Deep check | Real request path; content contains expected text | Need to know the app is actually functioning |
| TLS | Cert expiry (via `openssl s_client` over shell) | Long-lived production sites |

## Tracking state

To avoid alerting on every check:

1. Store the last known state in memory under `healthcheck:<url>` — the last status + timestamp.
2. Alert only on transitions (was-OK → now-failing), or after N consecutive failures, not every failure.
3. Clear the state on recovery.

## Latency budget

- < 500 ms: healthy.
- 500 ms – 2 s: slow, worth mentioning.
- > 2 s: concerning; investigate.
- Timeout (typically 15–30 s): treat as down.

Tune to the service — an API should be fast; a PDF generator legitimately takes longer.

## TLS expiry check via shell

```
echo | openssl s_client -servername example.com -connect example.com:443 2>/dev/null \
  | openssl x509 -noout -dates
```
Output gives `notBefore=...` and `notAfter=...`. Flag if `notAfter` is within two weeks.

## Failure modes

- DNS resolution failure → the hostname is wrong or the domain is dead.
- Connection refused → port is open but nothing is listening.
- Connection timeout → server is unreachable or firewall blocks.
- TLS handshake error → cert expired, hostname mismatch, or protocol mismatch.
- 5xx → server-side bug; content of response body usually has the real error.
- 4xx → the request itself is wrong (missing auth, wrong path).

## Rules

- Don't DDoS a site you're monitoring. Five-minute intervals are plenty for most services.
- When a check fails, include the URL, status code, and elapsed time in the alert — don't just say "it's down".
- When the user asks "is X up?" once-off, just check; don't schedule without asking.
