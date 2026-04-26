---
name: translation
description: Translate text between languages via DeepL's API. Use when the user asks to translate a message, document snippet, or needs a language bridge. Requires DEEPL_API_KEY.
always: false
---

# translation

DeepL offers high-quality machine translation with a free tier (500,000 characters/month). Preferred over Google Translate for European language quality; both are options depending on key availability.

## First-time setup

1. Sign up at https://www.deepl.com/pro-api — pick the **Free** plan.
2. Grab the **Auth Key** from your account page.
3. Save it: `export DEEPL_API_KEY="<key>:fx"` (free keys end in `:fx`; pro keys don't).

## Endpoint selection

The endpoint differs by plan:

| Plan | Endpoint |
|---|---|
| Free (key ends in `:fx`) | `https://api-free.deepl.com/v2/` |
| Pro | `https://api.deepl.com/v2/` |

Detect at runtime: if the key ends in `:fx`, hit the free endpoint.

## Required header

```
Authorization: DeepL-Auth-Key <DEEPL_API_KEY>
```

Note the scheme: `DeepL-Auth-Key`, **not** `Bearer`.

## Translate

```
POST <endpoint>/translate
Content-Type: application/x-www-form-urlencoded
(or application/json)

text=<text to translate>
target_lang=EN
source_lang=DE            # optional; DeepL auto-detects if omitted
formality=default         # optional: default|more|less|prefer_more|prefer_less
```

JSON body shape:
```json
{
  "text": ["Line 1", "Line 2"],
  "target_lang": "EN",
  "formality": "default"
}
```

Response:
```json
{"translations": [{"detected_source_language": "DE", "text": "..."}]}
```

## Language codes

`EN`, `EN-US`, `EN-GB`, `DE`, `FR`, `ES`, `IT`, `JA`, `ZH`, `PT-BR`, `PT-PT`, `RU`, `KO`, `TR`, etc. Use uppercase. For a full current list:

```
GET <endpoint>/languages?type=target
GET <endpoint>/languages?type=source
```

## Usage limits

```
GET <endpoint>/usage
```
Returns `{character_count, character_limit}`. Check before large jobs.

Free: 500,000 chars/month (resets on subscription cycle). Pro: billed per character above the monthly allowance.

## Tips

- Quote and punctuation are preserved; markdown is partially preserved but not guaranteed — review for code blocks and links.
- For HTML input, pass `tag_handling=html` so DeepL doesn't mangle tags.
- `formality` only affects languages that distinguish (German, French, Spanish, Italian, Polish, Portuguese, Russian, Dutch, Japanese). Silently ignored for English.
- For very long inputs, split by paragraph — DeepL handles chunked `text[]` arrays in one request.

## Alternatives

- **Google Translate** (Cloud Translation API): `POST https://translation.googleapis.com/language/translate/v2` with `?key=<KEY>`, body `{q, target, source}`. Uses a simple API key (not OAuth). Pay-as-you-go after free tier. Env var: `GOOGLE_TRANSLATE_API_KEY`.
- **LibreTranslate**: self-hosted, open-source. Useful when data must stay local.

Pick by: volume (DeepL free suffices for most personal use), language quality (DeepL is stronger on European languages), privacy (LibreTranslate self-hosted).

## Failure modes

- `403 Forbidden` → key invalid or hit wrong endpoint (free key on pro URL or vice versa).
- `456 Quota Exceeded` → hit the monthly character limit. Check `/usage`.
- `429` → rate-limited; pause a few seconds.
- `400` with `message` → bad `target_lang` code, or unsupported language pair.
