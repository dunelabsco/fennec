---
name: weather
description: Use the `weather` tool for city-based current weather and 7-day forecasts. Use when the user asks about weather anywhere.
always: false
---

# weather

Fennec ships a built-in `weather` tool backed by Open-Meteo (free, no API key). Prefer it over raw `web_fetch` against weather APIs.

## Calling it

```
weather(city: "Paris", units: "metric")
```

- `city` — required. City name, optionally with country or region disambiguation: `"Paris"`, `"Paris, France"`, `"Springfield, Missouri"`.
- `units` — optional, `"metric"` (default, celsius + kmh) or `"imperial"` (fahrenheit + mph).

Returns current conditions + a 7-day forecast, with Open-Meteo weather codes translated into plain-English descriptions.

## When to ask for input

- User says "here" or doesn't name a location → ask for one. Do not guess based on IP or timezone.
- Ambiguous name ("Springfield", "Portland", "Paris" when not obvious) → ask which one, or pass the disambiguating region.

## Presenting the answer

- Match the user's unit preference if they expressed one; otherwise default to metric (unless the user's locale strongly suggests imperial — US, Liberia, Myanmar). When unsure, offer both inline once.
- Most chat contexts only need the current conditions plus today's high/low. Don't dump the full 7 days unless asked.
- One or two lines is usually enough: `"Paris: light rain, 8°C, feels like 5°C. Today 4–11°C."`
- Attach a one-phrase descriptor ("clear and cool", "heavy rain likely") when the weather code alone isn't obvious.

## When `weather` is not enough

The built-in tool covers the common case. For niche needs — air quality, marine, pollen, historical weather — fall back to `http_request` against a specific provider's API, or `web_fetch` against a human-readable page.

## Failure modes

- `no matches for <city>` → city not resolvable. Ask the user for a more specific name (add country/region) or coordinates.
- Network error → say so explicitly. Do not invent numbers.
- Tool returns but values look wrong (e.g. -999, or 0 for every hour) → treat as an upstream glitch, retry once, then report a transient failure.
