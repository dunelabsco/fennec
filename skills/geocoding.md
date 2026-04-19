---
name: geocoding
description: Convert place names to coordinates (forward) or coordinates to place names (reverse) via OpenStreetMap Nominatim. Use when you need lat/long for a lookup or a human-readable address for a pin.
always: false
---

# geocoding

OpenStreetMap's Nominatim service is free, no-key, and covers the whole world. Use `http_request` or `web_fetch`.

## Base URL

```
https://nominatim.openstreetmap.org/
```

## Required header

```
User-Agent: fennec/0.1 (<your-email-or-url>)
```

Nominatim rate-limits and may block generic / empty user-agents. Set a real one.

## Forward geocoding (name → coords)

```
GET /search?q=<place>&format=json&limit=5&addressdetails=1
```

Response: array of hits with `lat`, `lon`, `display_name`, `type`, `class`, and — with `addressdetails=1` — a structured `address` object (road, city, country, country_code).

Narrow with:
- `countrycodes=us,ca` — ISO two-letter codes.
- `viewbox=<min_lon>,<min_lat>,<max_lon>,<max_lat>&bounded=1` — restrict to a box.
- `featuretype=city|street|settlement` — type filter.

## Reverse geocoding (coords → name)

```
GET /reverse?lat=<lat>&lon=<lon>&format=json&zoom=18&addressdetails=1
```

`zoom` controls specificity: 3 (country), 8 (state), 12 (city), 16 (street), 18 (building).

## Rate limit

**1 request per second, strict.** Pause between calls. For bulk geocoding of many places, use a self-hosted Nominatim or a paid provider — don't burst the public instance.

## Tips

- `display_name` is the full formatted address, comma-separated. Readable.
- Ambiguous names return multiple hits — show the user the top 3 with `display_name` and let them pick.
- Coordinate order is `lat, lon` in the response but `lon, lat` in the `viewbox` parameter — read carefully.
- For places with diacritics, UTF-8 encode the query.

## Failure modes

- Empty `[]` → place not found. Try removing modifiers, or add country to disambiguate.
- 403 / 429 → you hit the rate limit or the user-agent was rejected. Slow down; set a descriptive UA.
- Reverse lookup on ocean coordinates returns an empty object with no `address` — handle gracefully.
- Old / renamed places may return stale matches (OSM data catches up on a delay).

## Alternatives if Nominatim is throttling you

- Open-Meteo's geocoding (covers the same need for city lookups, used by the `weather` tool internally): `https://geocoding-api.open-meteo.com/v1/search?name=<city>`.
- Commercial (paid, faster): Mapbox, Google Maps Geocoding, LocationIQ (has a free tier with a key).
