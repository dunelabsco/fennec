---
name: currency
description: Fetch current and historical exchange rates between currencies. Use when the user asks for a conversion or needs FX data for a calculation.
always: false
---

# currency

Frankfurter (https://frankfurter.dev) is a free, no-key rate API sourcing data from the European Central Bank and other public banks. Covers 30+ currencies. Current primary API is **v2** on `api.frankfurter.dev`.

## Base URL

```
https://api.frankfurter.dev/v2/
```

(The legacy v1 paths — `/v1/latest`, `/v1/<date>`, etc. — still respond today, and the older `api.frankfurter.app` domain redirects to them. Prefer v2 for new code; v1 stays around for backward compatibility.)

## Latest rates

```
GET /v2/rates?base=USD&quotes=EUR,GBP,JPY
```

Parameters:
- `base` — base currency code. Omit for the API's default (EUR).
- `quotes` — comma-separated targets. Omit to get all available.
- `providers` — optional; restrict to specific data sources (e.g. `ecb`). Omit for the default aggregated view.

Response:
```json
{
  "base": "USD",
  "date": "2026-04-18",
  "rates": {"EUR": 0.92, "GBP": 0.78, "JPY": 151.4}
}
```

## Historical rate (single date)

```
GET /v2/rates?date=2024-01-15&base=USD&quotes=EUR
```

Any date from 1999-01-04 onwards (earliest ECB data).

## Time series

```
GET /v2/rates?from=2025-01-01&to=2025-12-31&base=USD&quotes=EUR
```

Returns daily business-day values. Add `group=monthly` (or `weekly`, `yearly`) to downsample.

## Single currency pair (compact shape)

```
GET /v2/rate/USD/EUR
GET /v2/rate/USD/EUR?date=2024-01-15
```

## Supported currencies

```
GET /v2/currencies
GET /v2/currencies?scope=all       # include legacy / retired currencies
```

Returns `{ "CODE": "Full Name", ... }`. Covers USD, EUR, GBP, JPY, CHF, CNY, CAD, AUD, NZD, SEK, NOK, DKK, PLN, CZK, HUF, plus several others. **Does not cover:** most crypto, or obscure third-tier currencies.

## Data providers list

```
GET /v2/providers
```

Shows which central banks Frankfurter aggregates from (ECB is the default).

## Param rename note

v1 used `symbols=...` for the target currency list. v2 renamed that to `quotes=...`. Everything else is roughly analogous but reorganised around a single `/rates` endpoint with query params instead of multiple path shapes.

## Rules

- Rates are end-of-day reference rates, not live market rates. For live or intraday data, use a paid provider (Fixer, currencylayer, exchangerate-api).
- Weekends and public holidays have no new data — the prior business day's rate is returned until new data lands.
- No rate data for future dates; earliest is 1999-01-04.
- No auth, no key — but be a polite client. Narrow responses with `quotes=` rather than pulling every currency.

## Failure modes

- 404 on a specific date → the source didn't publish that day (weekend / holiday). Retry with the previous business day.
- 422 `currency not found` → code isn't in Frankfurter's set. Check `/v2/currencies?scope=all` for what's available.
- Rates look wrong → double-check `base`; if you don't pass it, it defaults to EUR, not USD.

## Alternatives

- **Live / crypto / intraday**: exchangerate-api.com (free tier with key), CoinGecko (crypto, free).
- **Higher precision**: Fixer.io, currencylayer — paid tiers for commercial use.
