---
name: currency
description: Fetch current and historical exchange rates between currencies. Use when the user asks for a conversion or needs FX data for a calculation.
always: false
---

# currency

Frankfurter (https://frankfurter.dev) is a free, no-key rate API sourcing data from the European Central Bank. Updated daily, covers the 30+ currencies the ECB tracks.

## Base URL

```
https://api.frankfurter.dev/v1/
```

## Latest rates

```
GET /v1/latest?base=USD&symbols=EUR,GBP,JPY
```

Response:
```
{
  "amount": 1.0,
  "base": "USD",
  "date": "2026-04-18",
  "rates": {"EUR": 0.92, "GBP": 0.78, "JPY": 151.4}
}
```

Default `base` is EUR; default `symbols` is all available.

## Convert a specific amount

```
GET /v1/latest?amount=250&from=USD&to=EUR
```

Returns converted value directly.

## Historical rate

```
GET /v1/2024-01-15?base=USD&symbols=EUR
```

Any date from 1999-01-04 onwards (EUR introduction).

## Time series

```
GET /v1/2025-01-01..2025-12-31?base=USD&symbols=EUR
```

Returns `rates: {"2025-01-01": {"EUR": ...}, ...}` — daily business-day values.

## Supported currencies

```
GET /v1/currencies
```

Returns a dict of `CODE: "Full Name"`. The ECB tracks majors (USD, EUR, GBP, JPY, CHF, CNY, CAD, AUD, NZD, SEK, NOK, DKK, PLN, CZK, HUF, etc.) and a handful of Asian currencies. **Does not cover:** most crypto, or obscure third-tier currencies. For those, use a paid provider.

## Rules

- Rates are end-of-day ECB reference rates, not live market rates. For trading decisions the user needs a live-data provider, not this.
- Weekends have no new data — Friday's rate is the most recent until Monday.
- No rate data for future dates, and the earliest is 1999-01-04.

## Failure modes

- 404 on a date → weekend or holiday; ECB didn't publish. Fall back to the preceding business day.
- 422 with `not found` → currency code isn't in ECB's set. Check `/v1/currencies` for what's available.
- Rates look wrong → double-check `base` parameter; default is EUR, not USD.

## Alternatives

- **Live / crypto**: exchangerate-api.com (free tier with key), CoinGecko (crypto, free).
- **Higher precision (more currencies, intraday)**: Fixer.io, currencylayer — paid.
