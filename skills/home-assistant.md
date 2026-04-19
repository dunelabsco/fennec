---
name: home-assistant
description: Read state and call services on a Home Assistant instance via its REST API — smart-home switches, sensors, climate, scripts, automations. Use for any smart-home action the user has set up in HA. Requires HOMEASSISTANT_URL and HOMEASSISTANT_TOKEN env vars.
always: false
---

# home-assistant

Home Assistant (HA) is the open-source smart-home hub that supports thousands of devices. Its REST API exposes everything HA knows about — states, services, events. No cloud dependency; works on the LAN.

## First-time setup

1. Find the user's HA URL. Typical shapes:
   - Local: `http://homeassistant.local:8123` or `http://<ip>:8123`.
   - Nabu Casa Cloud: `https://<user>.ui.nabu.casa`.
2. Generate a Long-Lived Access Token (LLT):
   - HA web UI → click the user's profile (bottom-left) → **Security** → scroll to **Long-lived access tokens** → **Create token**.
   - Copy the token immediately (shown once).
3. Save:
   ```
   export HOMEASSISTANT_URL="http://homeassistant.local:8123"
   export HOMEASSISTANT_TOKEN="..."
   ```

Token has no expiry unless the user deletes it.

## Auth (every request)

```
Authorization: Bearer <HOMEASSISTANT_TOKEN>
Content-Type: application/json
```

## Verify the token

```
GET $HOMEASSISTANT_URL/api/
```
Returns `{"message": "API running."}` when auth works.

## States — reading the world

**All entities**
```
GET $HOMEASSISTANT_URL/api/states
```

Returns an array. Each entity: `entity_id`, `state`, `attributes`, `last_changed`, `last_updated`.

**One entity**
```
GET $HOMEASSISTANT_URL/api/states/<entity_id>
```

Examples of `entity_id`: `light.living_room`, `sensor.outside_temperature`, `switch.kettle`, `climate.thermostat`, `media_player.living_room_tv`.

## Services — controlling the world

```
POST $HOMEASSISTANT_URL/api/services/<domain>/<service>
Body: {"entity_id": "light.living_room", ...other fields...}
```

Common combos:

**Turn a light on with brightness + colour**
```
POST /api/services/light/turn_on
{"entity_id": "light.living_room", "brightness": 200, "rgb_color": [255, 180, 100]}
```

**Toggle**
```
POST /api/services/switch/toggle
{"entity_id": "switch.kettle"}
```

**Thermostat**
```
POST /api/services/climate/set_temperature
{"entity_id": "climate.thermostat", "temperature": 21}
```

**Media player (play / pause / volume)**
```
POST /api/services/media_player/media_play_pause
{"entity_id": "media_player.living_room_tv"}

POST /api/services/media_player/volume_set
{"entity_id": "media_player.living_room_tv", "volume_level": 0.4}
```

**Run a script / automation**
```
POST /api/services/script/morning_routine

POST /api/services/automation/trigger
{"entity_id": "automation.goodnight"}
```

**Send a notification**
```
POST /api/services/notify/<notifier_name>
{"message": "...", "title": "..."}
```
Notifier names are configured in HA (`notify.mobile_app_iphone`, etc.).

## Fire an event

```
POST $HOMEASSISTANT_URL/api/events/<event_type>
Body: {"optional": "data"}
```

Used to trigger automations listening for custom events.

## History

```
GET $HOMEASSISTANT_URL/api/history/period/<start-iso>?filter_entity_id=<entity_id>&end_time=<end-iso>
```

Returns state changes over the given window. Useful for "when did X last happen?" questions.

## Rules

- **Don't push irreversible actions without confirmation.** Locking doors, arming alarms, unlocking — always confirm. Turning lights off is usually fine.
- Entity IDs are snake_case. Names in the UI ("Living Room Light") are friendly names; the API uses the ID form.
- Service data fields (brightness, rgb_color, temperature, etc.) vary per domain. Check `/api/services` for the full schema per service.
- For frequent polling (e.g. temperature logging), prefer HA's own automation / logbook rather than hammering the REST API.
- WebSocket API (`/api/websocket`) is faster for streaming state changes, but REST is simpler for one-off queries.

## Failure modes

- `401 Unauthorized` → token invalid or deleted.
- `404` on `/states/<entity_id>` → entity doesn't exist. List `/states` and grep.
- `400 Invalid request` on a service call → required field missing (brightness without entity_id, etc.). HA's error body names the issue.
- Response `200` but no visible effect → the service call reached HA but the device isn't responding. Read the entity's state after — it may report `unavailable`.
- `503 Service Unavailable` → HA is restarting or under load. Retry after a few seconds.
