---
name: spotify-player
description: Control Spotify playback (play, pause, skip, search, queue, get currently playing) via the Spotify Web API. Use when the user wants music control from chat. Requires OAuth 2.0 PKCE setup; access token in SPOTIFY_ACCESS_TOKEN env var.
always: false
---

# spotify-player

Spotify's Web API covers playback control, search, and library queries. Personal use requires OAuth 2.0 with PKCE (no client secret on the client — safe for public apps). Access tokens are short-lived (1 hour); refresh tokens keep things working long-term.

## First-time setup

1. Create an app at https://developer.spotify.com/dashboard:
   - Redirect URI: `http://localhost:8080/callback` (any localhost works).
   - Note the **Client ID** (no client secret needed with PKCE).
2. Run an OAuth 2.0 PKCE flow once:

### PKCE steps

- Generate a `code_verifier`: random string, 43–128 chars of `A-Z a-z 0-9 _ . - ~`.
- Compute `code_challenge` = base64url(sha256(code_verifier)).
- Build auth URL:
  ```
  https://accounts.spotify.com/authorize
    ?response_type=code
    &client_id=<CLIENT_ID>
    &redirect_uri=http://localhost:8080/callback
    &scope=user-read-playback-state%20user-modify-playback-state%20user-read-currently-playing%20user-read-private
    &code_challenge=<code_challenge>
    &code_challenge_method=S256
    &state=<random>
  ```
- User approves, redirected to `http://localhost:8080/callback?code=...`.
- Exchange:
  ```
  POST https://accounts.spotify.com/api/token
  Content-Type: application/x-www-form-urlencoded

  grant_type=authorization_code
  code=<code>
  redirect_uri=http://localhost:8080/callback
  client_id=<CLIENT_ID>
  code_verifier=<code_verifier>
  ```
- Response: `{"access_token": "...", "refresh_token": "...", "expires_in": 3600}`.

3. Save:
   ```
   export SPOTIFY_CLIENT_ID="..."
   export SPOTIFY_ACCESS_TOKEN="..."     # short-lived
   export SPOTIFY_REFRESH_TOKEN="..."    # long-lived
   ```

## Refresh a stale token

```
POST https://accounts.spotify.com/api/token
Content-Type: application/x-www-form-urlencoded

grant_type=refresh_token
refresh_token=<SPOTIFY_REFRESH_TOKEN>
client_id=<SPOTIFY_CLIENT_ID>
```

Response has a new `access_token` (and possibly a rotated `refresh_token` — store it).

## Auth header (every API call)

```
Authorization: Bearer <SPOTIFY_ACCESS_TOKEN>
```

## Scopes

| Scope | What it unlocks |
|---|---|
| `user-read-private` | Profile info (country, product tier) |
| `user-read-email` | Email |
| `user-read-playback-state` | Current playback, devices |
| `user-modify-playback-state` | Play, pause, skip, seek, volume |
| `user-read-currently-playing` | What's playing right now |
| `user-top-read` | Top artists/tracks |
| `user-library-read` / `user-library-modify` | Saved tracks / albums |
| `playlist-read-private` / `playlist-modify-private` / `playlist-modify-public` | Playlist CRUD |

Pick the narrowest set the user needs. For "play / pause / skip / search", the ones in the setup example (`user-read-playback-state`, `user-modify-playback-state`, `user-read-currently-playing`, `user-read-private`) are sufficient.

## Common operations

**Current user**
```
GET https://api.spotify.com/v1/me
```

**Currently playing**
```
GET https://api.spotify.com/v1/me/player/currently-playing
```

**Full playback state (device, volume, progress)**
```
GET https://api.spotify.com/v1/me/player
```

**Play (resume current, or start a URI / context)**
```
PUT https://api.spotify.com/v1/me/player/play
Body: {}                                                         # resume
Body: {"uris": ["spotify:track:<trackId>"]}                       # single track
Body: {"context_uri": "spotify:album:<albumId>"}                  # start an album / playlist
Body: {"context_uri": "spotify:playlist:<playlistId>", "offset": {"position": 3}}
```

**Pause**
```
PUT https://api.spotify.com/v1/me/player/pause
```

**Skip forward / back**
```
POST https://api.spotify.com/v1/me/player/next
POST https://api.spotify.com/v1/me/player/previous
```

**Volume (0–100)**
```
PUT https://api.spotify.com/v1/me/player/volume?volume_percent=50
```

**Seek (milliseconds)**
```
PUT https://api.spotify.com/v1/me/player/seek?position_ms=30000
```

**Search (no auth scope beyond valid token)**
```
GET https://api.spotify.com/v1/search?q=<query>&type=track,artist,album&limit=10
```

`type` is comma-separated — pick any of `track`, `artist`, `album`, `playlist`, `show`, `episode`, `audiobook`.

**Queue a track**
```
POST https://api.spotify.com/v1/me/player/queue?uri=spotify:track:<trackId>
```

**Transfer playback to another device**
```
PUT https://api.spotify.com/v1/me/player
Body: {"device_ids": ["<device_id>"], "play": true}
```

List devices first: `GET https://api.spotify.com/v1/me/player/devices`.

## Premium requirement

Most playback-control endpoints (play, pause, skip, volume, seek) require a **Spotify Premium** account. Free accounts get search + read-only state but cannot be remote-controlled.

## Rules

- Access tokens last 1 hour. Refresh automatically on 401 before retrying.
- Playback commands only work when the user has an active device. If `GET /me/player` returns `null` or `device` is absent, tell the user to open Spotify on one of their devices (phone, desktop app, web player) and try again.
- Don't hammer `play` / `pause` — Spotify rate-limits this path specifically.
- URIs and IDs: URIs look like `spotify:track:3n3Ppam7vgaVa1iaRUc9Lp`. IDs are the last segment. Most endpoints accept either, context endpoints need the full URI.

## Failure modes

- `401 Unauthorized` → token stale; refresh and retry.
- `403 Forbidden` with `PREMIUM_REQUIRED` → the user is on Free; offer search / state only.
- `404 NO_ACTIVE_DEVICE` → no Spotify client is currently active. Ask the user to open the app.
- `429` → rate-limited; back off with `Retry-After`.
- `402 PAYMENT_REQUIRED` on playback — alternative form of "needs Premium".
