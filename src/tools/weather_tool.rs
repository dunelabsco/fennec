//! Weather tool — current conditions + short forecast via Open-Meteo.
//!
//! Open-Meteo (https://open-meteo.com) is free, requires no API key, and
//! exposes both geocoding (city → lat/lon) and forecast endpoints. Users
//! ask by city name; the tool geocodes, then fetches the weather.

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

use super::traits::{Tool, ToolResult};

const GEOCODE_URL: &str = "https://geocoding-api.open-meteo.com/v1/search";
const FORECAST_URL: &str = "https://api.open-meteo.com/v1/forecast";

pub struct WeatherTool {
    client: reqwest::Client,
}

impl Default for WeatherTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WeatherTool {
    pub fn new() -> Self {
        Self {
            client: super::http::shared_client(),
        }
    }

    async fn geocode(&self, city: &str) -> Result<Option<(f64, f64, String, String)>> {
        let resp: Value = self
            .client
            .get(GEOCODE_URL)
            .query(&[("name", city), ("count", "1")])
            .timeout(Duration::from_secs(10))
            .send()
            .await?
            .json()
            .await?;

        if let Some(arr) = resp.get("results").and_then(|v| v.as_array()) {
            if let Some(first) = arr.first() {
                let lat = first.get("latitude").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let lon = first.get("longitude").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let name = first
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let country = first
                    .get("country")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                return Ok(Some((lat, lon, name, country)));
            }
        }
        Ok(None)
    }

    async fn forecast(&self, lat: f64, lon: f64, units: &str) -> Result<Value> {
        let (temp_unit, wind_unit) = match units {
            "imperial" => ("fahrenheit", "mph"),
            _ => ("celsius", "kmh"),
        };
        let resp: Value = self
            .client
            .get(FORECAST_URL)
            .query(&[
                ("latitude", lat.to_string().as_str()),
                ("longitude", lon.to_string().as_str()),
                ("current_weather", "true"),
                ("daily", "temperature_2m_max,temperature_2m_min,precipitation_sum,weathercode"),
                ("timezone", "auto"),
                ("temperature_unit", temp_unit),
                ("windspeed_unit", wind_unit),
            ])
            .timeout(Duration::from_secs(10))
            .send()
            .await?
            .json()
            .await?;
        Ok(resp)
    }
}

#[async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &str {
        "weather"
    }

    fn description(&self) -> &str {
        "Get current weather + 7-day forecast for a city. Uses Open-Meteo \
         (free, no API key). Supports metric (celsius/kmh) and imperial \
         (fahrenheit/mph) units."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "city": {
                    "type": "string",
                    "description": "City name, optionally with country (e.g. 'Paris' or 'Paris, France')."
                },
                "units": {
                    "type": "string",
                    "enum": ["metric", "imperial"],
                    "description": "Defaults to metric."
                }
            },
            "required": ["city"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let city = match args.get("city").and_then(|v| v.as_str()) {
            Some(c) if !c.is_empty() => c.to_string(),
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("missing required parameter: city".to_string()),
                });
            }
        };
        let units = args
            .get("units")
            .and_then(|v| v.as_str())
            .filter(|u| matches!(*u, "metric" | "imperial"))
            .unwrap_or("metric");

        let geo = match self.geocode(&city).await {
            Ok(Some(g)) => g,
            Ok(None) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("no geocoding match for '{}'", city)),
                });
            }
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("geocoding failed: {}", e)),
                });
            }
        };

        let (lat, lon, city_name, country) = geo;

        let fc = match self.forecast(lat, lon, units).await {
            Ok(v) => v,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("forecast failed: {}", e)),
                });
            }
        };

        let output = format_weather(&city_name, &country, units, &fc);
        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

fn format_weather(city: &str, country: &str, units: &str, fc: &Value) -> String {
    let temp_symbol = if units == "imperial" { "°F" } else { "°C" };
    let wind_symbol = if units == "imperial" { "mph" } else { "km/h" };

    let mut out = format!("Weather for {}{}\n\n", city, if country.is_empty() { String::new() } else { format!(", {}", country) });

    if let Some(cur) = fc.get("current_weather") {
        let temp = cur.get("temperature").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let wind = cur.get("windspeed").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let code = cur.get("weathercode").and_then(|v| v.as_i64()).unwrap_or(-1);
        out.push_str(&format!(
            "Now: {:.1}{} · {} · wind {:.0} {}\n\n",
            temp,
            temp_symbol,
            wmo_code_to_text(code),
            wind,
            wind_symbol,
        ));
    }

    if let Some(daily) = fc.get("daily") {
        let times = daily.get("time").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let maxes = daily
            .get("temperature_2m_max")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mins = daily
            .get("temperature_2m_min")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let precip = daily
            .get("precipitation_sum")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let codes = daily
            .get("weathercode")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        out.push_str("Forecast:\n");
        let n = times.len().min(7);
        for i in 0..n {
            let date = times.get(i).and_then(|v| v.as_str()).unwrap_or("");
            let hi = maxes.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0);
            let lo = mins.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0);
            let pp = precip.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0);
            let code = codes.get(i).and_then(|v| v.as_i64()).unwrap_or(-1);
            out.push_str(&format!(
                "  {}  {:.1}{}/{:.1}{}  precip {:.1}mm  {}\n",
                date,
                hi,
                temp_symbol,
                lo,
                temp_symbol,
                pp,
                wmo_code_to_text(code),
            ));
        }
    }

    out
}

/// Map WMO weather code (https://open-meteo.com/en/docs#weathervariables) to
/// a short human-readable summary.
fn wmo_code_to_text(code: i64) -> &'static str {
    match code {
        0 => "clear",
        1 => "mainly clear",
        2 => "partly cloudy",
        3 => "overcast",
        45 | 48 => "fog",
        51..=55 => "drizzle",
        56 | 57 => "freezing drizzle",
        61..=65 => "rain",
        66 | 67 => "freezing rain",
        71..=75 => "snow",
        77 => "snow grains",
        80..=82 => "rain showers",
        85 | 86 => "snow showers",
        95 => "thunderstorm",
        96 | 99 => "thunderstorm with hail",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wmo_code_mappings() {
        assert_eq!(wmo_code_to_text(0), "clear");
        assert_eq!(wmo_code_to_text(3), "overcast");
        assert_eq!(wmo_code_to_text(45), "fog");
        assert_eq!(wmo_code_to_text(52), "drizzle");
        assert_eq!(wmo_code_to_text(63), "rain");
        assert_eq!(wmo_code_to_text(73), "snow");
        assert_eq!(wmo_code_to_text(95), "thunderstorm");
        assert_eq!(wmo_code_to_text(-1), "unknown");
        assert_eq!(wmo_code_to_text(9999), "unknown");
    }

    #[test]
    fn format_weather_includes_city_and_current() {
        let fc = json!({
            "current_weather": {
                "temperature": 15.5,
                "windspeed": 10.0,
                "weathercode": 2
            },
            "daily": {
                "time": ["2026-04-17", "2026-04-18"],
                "temperature_2m_max": [18.0, 20.0],
                "temperature_2m_min": [10.0, 11.0],
                "precipitation_sum": [0.0, 1.2],
                "weathercode": [1, 63]
            }
        });
        let out = format_weather("Istanbul", "Turkey", "metric", &fc);
        assert!(out.contains("Istanbul"));
        assert!(out.contains("Turkey"));
        assert!(out.contains("15.5°C"));
        assert!(out.contains("partly cloudy"));
        assert!(out.contains("2026-04-17"));
        assert!(out.contains("rain"));
    }

    #[test]
    fn format_weather_imperial_uses_f() {
        let fc = json!({
            "current_weather": {"temperature": 59.0, "windspeed": 5.0, "weathercode": 0}
        });
        let out = format_weather("X", "", "imperial", &fc);
        assert!(out.contains("°F"));
        assert!(out.contains("mph"));
    }

    #[tokio::test]
    async fn execute_rejects_missing_city() {
        let t = WeatherTool::new();
        let r = t.execute(json!({})).await.unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("city"));
    }

    #[test]
    fn is_read_only() {
        let t = WeatherTool::new();
        assert!(t.is_read_only());
    }
}
