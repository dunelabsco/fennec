---
name: sonos
description: Control Sonos speakers on the local LAN — play, pause, volume, group, queue — via the speakers' built-in SOAP API on port 1400. No cloud, no auth beyond being on the same network. Requires SONOS_IP env var (one speaker's IP suffices).
always: false
---

# sonos

Every Sonos speaker exposes a SOAP/UPnP control surface on port 1400 over the local network. No cloud account needed for basic playback control. This skill drives them directly with HTTP POST requests — a bit awkward (SOAP), but works immediately on any LAN.

For automation-grade work (many users, queue management, favorites), prefer the `node-sonos-http-api` shim project (installable on any Raspberry Pi) which wraps this API in a clean REST interface. Also consider the Python `soco` library if the user is scripting.

## First-time setup

1. Find one speaker's IP. Options:
   - Sonos app → **Settings → System → About My System** → each speaker shows its IP.
   - Router DHCP table.
   - SSDP discovery: `M-SEARCH` on UDP port 1900 for `urn:schemas-upnp-org:device:ZonePlayer:1`.
2. Save: `export SONOS_IP="192.168.1.50"`.

Any speaker's IP works — the API can query and control the whole household.

## Base URL

```
http://<SONOS_IP>:1400/
```

**HTTP, not HTTPS.** Speakers don't ship with trusted certs for LAN.

## Key endpoints

| Service | Control endpoint |
|---|---|
| Playback (play, pause, next, URI) | `/MediaRenderer/AVTransport/Control` |
| Volume, mute | `/MediaRenderer/RenderingControl/Control` |
| Zone grouping | `/MediaRenderer/AVTransport/Control` with `SetAVTransportURI` → `x-rincon:...` |
| Queue | `/MediaRenderer/Queue/Control` |
| Device info | `/xml/device_description.xml` (GET — no SOAP) |
| Topology / grouping state | `/ZoneGroupTopology/Control` |

## SOAP request shape

Every control call is:

```
POST http://<SONOS_IP>:1400/MediaRenderer/AVTransport/Control
Content-Type: text/xml; charset="utf-8"
SOAPACTION: "urn:schemas-upnp-org:service:AVTransport:1#<ActionName>"

<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
  <s:Body>
    <u:<ActionName> xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
      <InstanceID>0</InstanceID>
      <!-- action-specific parameters -->
    </u:<ActionName>>
  </s:Body>
</s:Envelope>
```

`InstanceID` is always `0` on Sonos (single virtual instance per speaker).

## Common actions

**Play**
```xml
<u:Play xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
  <InstanceID>0</InstanceID>
  <Speed>1</Speed>
</u:Play>
```
SOAPACTION: `"urn:schemas-upnp-org:service:AVTransport:1#Play"`

**Pause**
```xml
<u:Pause xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
  <InstanceID>0</InstanceID>
</u:Pause>
```

**Next / Previous**
```xml
<u:Next xmlns:u="..."><InstanceID>0</InstanceID></u:Next>
<u:Previous xmlns:u="..."><InstanceID>0</InstanceID></u:Previous>
```

**Set volume** (on RenderingControl, not AVTransport)
```xml
<u:SetVolume xmlns:u="urn:schemas-upnp-org:service:RenderingControl:1">
  <InstanceID>0</InstanceID>
  <Channel>Master</Channel>
  <DesiredVolume>25</DesiredVolume>
</u:SetVolume>
```
SOAPACTION: `"urn:schemas-upnp-org:service:RenderingControl:1#SetVolume"`. `DesiredVolume` 0–100.

**Group two speakers** (make `<SONOS_IP_B>` follow `<SONOS_IP_A>`)

Get A's UUID first:
```
GET http://<SONOS_IP_A>:1400/status/zp → returns XML with <LocalUID>RINCON_...</LocalUID>
```

Then on B, set `CurrentURI` to `x-rincon:<RINCON_UUID>`:
```xml
<u:SetAVTransportURI xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
  <InstanceID>0</InstanceID>
  <CurrentURI>x-rincon:RINCON_XXXXXXXXXXXX01400</CurrentURI>
  <CurrentURIMetaData></CurrentURIMetaData>
</u:SetAVTransportURI>
```

**Ungroup** (play something on B standalone) — send `SetAVTransportURI` with a stream URL or queue URI.

## Read-only status

No SOAP needed for state:

```
GET http://<SONOS_IP>:1400/status/zp         # this speaker's zone info (XML)
GET http://<SONOS_IP>:1400/status/topology   # whole household grouping
```

For transport state (playing / paused / title / artist):
```xml
<u:GetPositionInfo xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
  <InstanceID>0</InstanceID>
</u:GetPositionInfo>
```
Response contains `TrackMetaData` (DIDL-Lite XML) with current song info.

## Rules

- **HTTP on the LAN.** Don't try HTTPS; speakers don't run a public-facing TLS stack.
- SOAP is verbose but stable. If hand-writing the XML is painful, route through `node-sonos-http-api` (trivial HTTP wrapper) instead.
- Volume changes are per-speaker even when grouped. To set the whole group's volume, iterate members, or send `SetGroupVolume` / `SetRelativeGroupVolume` to the group coordinator on the `GroupRenderingControl` service (endpoint `/MediaRenderer/GroupRenderingControl/Control`, SOAPACTION `urn:schemas-upnp-org:service:GroupRenderingControl:1#SetGroupVolume`) — these actions don't exist on `RenderingControl`.
- Unknown `CurrentURI` values (non-stream, non-`x-rincon:`) return 500 with a SOAP fault. Verify URIs before sending.
- Don't poll aggressively (every < 1 s); speakers are not servers. Subscribe to UPnP events if continuous status is needed.

## Failure modes

- HTTP connection refused → wrong IP, speaker powered off, or DHCP shuffled addresses. Re-discover.
- `500 Internal Server Error` with SOAP fault body → malformed XML or invalid parameter. The `<UPnPError><errorCode>` inside the body identifies the issue; 402 (Invalid args) is most common.
- Speaker plays but no sound → playing at volume 0, or audio line-in switched. Check `GetVolume` + current transport state.
- Group commands succeed but no grouping change → the target speaker's UUID is wrong, or both speakers are already in a group. Check `/status/topology` first.

## Related

- Easier alternatives that speak to the same underlying API:
  - `node-sonos-http-api` — simple HTTP bridge (`GET /<room>/play`, `GET /<room>/volume/25`).
  - `soco` (Python) — object-model wrapper; `from soco import SoCo`.
- `home-assistant` skill — if the user already runs Home Assistant, use its Sonos integration instead of raw SOAP.
