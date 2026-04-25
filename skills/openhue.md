---
name: openhue
description: Control Philips Hue lights through the local Hue bridge LAN API. Use when the user wants to turn lights on/off, set brightness or colour, trigger scenes, or read sensor state. Requires the bridge IP + an authorised username in HUE_BRIDGE_IP and HUE_USERNAME.
always: false
---

# openhue

Philips Hue bridges expose a local REST API over the LAN — no cloud account needed. Authenticate by physically pressing the link button on the bridge during setup. Everything after that stays on the local network.

## First-time setup

### 1. Find the bridge IP

Preferred: `GET https://discovery.meethue.com/` → returns a JSON array of local bridges:
```json
[{"id": "...", "internalipaddress": "192.168.1.42", "port": 443}]
```

Alternatives: check the router's DHCP table, or the official Hue app → Settings → Hue Bridges.

### 2. Register Fennec with the bridge

Press the physical link button on the top of the bridge. Within 30 seconds, send:

```
POST https://<bridge_ip>/api
Content-Type: application/json
Body: {"devicetype": "fennec#<your-machine-name>"}
```

Response (on success):
```json
[{"success": {"username": "<generated-username-string>"}}]
```

If the button wasn't pressed in time, you'll get `{"error": {"type": 101, "description": "link button not pressed"}}`. Press and retry.

### 3. Save the credentials

```
export HUE_BRIDGE_IP="192.168.1.42"
export HUE_USERNAME="<generated-username-string>"
```

The "username" is an opaque token (around 40 characters on modern bridges) and doubles as the API key. Keep it private — anyone on the LAN with it controls the lights.

## TLS note

Hue bridges use a self-signed certificate. There's no fully clean option:

- **Trusted home LAN** (your own router, no untrusted devices): `curl -k` / `http_request` with cert verification disabled is the practical choice and what the official Philips Hue libraries do. Risk: any device on the LAN can MITM bulb commands. Low-impact in practice — they could already join the LAN, find the bridge, and run their own auth.
- **Shared / untrusted network** (coworking, dorm, café): do NOT use `-k`. Fetch the bridge's certificate once (`openssl s_client -connect <BRIDGE_IP>:443 -showcerts`), pin it, and trust only that fingerprint. An attacker on the same network could otherwise intercept your username/API key.
- **Either case:** the username itself is the more important secret — leak it and `-k` becomes irrelevant.

## Base URL

```
https://<HUE_BRIDGE_IP>/api/<HUE_USERNAME>/
```

Older bridges (v1) accept HTTP; v2 bridges require HTTPS.

## Common operations

**List all lights**
```
GET /api/<HUE_USERNAME>/lights
```

Response: dict keyed by light ID (string) with `state`, `name`, `type`, `manufacturername`, `modelid`.

**Turn a light on / off**
```
PUT /api/<HUE_USERNAME>/lights/<id>/state
Body: {"on": true}
```

**Set brightness**
```
PUT /api/<HUE_USERNAME>/lights/<id>/state
Body: {"on": true, "bri": 128}
```
`bri` is 0–254 (not 255).

**Set colour (hue / saturation)**
```
PUT /api/<HUE_USERNAME>/lights/<id>/state
Body: {"on": true, "bri": 254, "hue": 8000, "sat": 200}
```
`hue` is 0–65535 (so 8000 ≈ 44°, orange), `sat` is 0–254.

**Set colour by CIE xy (more accurate across bulb models)**
```json
{"on": true, "xy": [0.5, 0.4]}
```

**Set colour temperature (warm / cool white bulbs)**
```json
{"on": true, "ct": 250}
```
`ct` is mired: 153 (cool 6500 K) to 500 (warm 2000 K).

**List groups (rooms)**
```
GET /api/<HUE_USERNAME>/groups
```

**Control a group (e.g. whole room at once)**
```
PUT /api/<HUE_USERNAME>/groups/<id>/action
Body: {"on": true, "bri": 200}
```

**List scenes**
```
GET /api/<HUE_USERNAME>/scenes
```

**Activate a scene**
```
PUT /api/<HUE_USERNAME>/groups/0/action
Body: {"scene": "<scene-id>"}
```
Group 0 = all lights.

## Transitions

Add `transitiontime` (in deciseconds — `10` = 1 second) to smooth changes:
```json
{"on": true, "bri": 50, "transitiontime": 20}
```
Default is 4 (400 ms).

## Rules

- Commands are asynchronous. The bridge responds OK even if the bulb is unreachable; check state after if reliability matters.
- Don't hammer the bridge. Keep to 10 commands/second/bridge — older bridges crash above that.
- Multi-light changes are faster via groups than looping over individual lights.
- The "username" is a shared secret on the LAN. Don't commit it.

## Failure modes

- `101 link button not pressed` during setup → press the button, retry within 30 s.
- `1 unauthorized user` → `HUE_USERNAME` is wrong or the bridge was factory-reset.
- `3 resource not available` → wrong light / group / scene ID. List to confirm.
- Bridge unreachable → check the IP (DHCP may have renewed), or ping it directly.
- Bulb won't respond to commands → it's powered off at the switch or out of ZigBee range. The bridge still accepts the command silently.
