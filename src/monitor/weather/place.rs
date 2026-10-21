// SPDX-License-Identifier: GPL-3.0-only

//! Where the weather is: a CEP, by AwesomeAPI or else BrasilAPI, or a
//! city's name, by Open-Meteo's geocoding.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::sources::get;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Place {
    /// The neighborhood and the city, or the city.
    pub name: String,
    /// The state and country, smaller.
    pub detail: String,
    pub lat: f64,
    pub lon: f64,
}

impl Default for Place {
    /// CEP 30330-330: São Pedro, Belo Horizonte.
    fn default() -> Self {
        Place {
            name: "São Pedro, Belo Horizonte".into(),
            detail: "MG".into(),
            lat: -19.9484,
            lon: -43.9372,
        }
    }
}

/// The digits of a CEP, when that's what was typed: 8 of them, with or
/// without the dash.
pub fn cep(text: &str) -> Option<String> {
    let text = text.trim();
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    let only_cep = text.chars().all(|c| c.is_ascii_digit() || c == '-' || c == '.' || c == ' ');
    (only_cep && digits.len() == 8).then_some(digits)
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// "Neighborhood, City", or the city alone.
fn name_of(neighborhood: Option<String>, city: Option<String>) -> Option<String> {
    match (neighborhood, city) {
        (Some(n), Some(c)) => Some(format!("{n}, {c}")),
        (None, Some(c)) => Some(c),
        _ => None,
    }
}

pub fn parse_awesome(bytes: &[u8]) -> Option<Place> {
    let json: Value = serde_json::from_slice(bytes).ok()?;
    Some(Place {
        name: name_of(text(&json, "district"), text(&json, "city"))?,
        detail: text(&json, "state").unwrap_or_default(),
        lat: number(json.get("lat"))?,
        lon: number(json.get("lng"))?,
    })
}

pub fn parse_brasilapi(bytes: &[u8]) -> Option<Place> {
    let json: Value = serde_json::from_slice(bytes).ok()?;
    let at = json.pointer("/location/coordinates")?;
    Some(Place {
        name: name_of(text(&json, "neighborhood"), text(&json, "city"))?,
        detail: text(&json, "state").unwrap_or_default(),
        lat: number(at.get("latitude"))?,
        lon: number(at.get("longitude"))?,
    })
}

pub fn parse_geocoding(bytes: &[u8]) -> Vec<Place> {
    let Ok(json) = serde_json::from_slice::<Value>(bytes) else { return Vec::new() };
    let Some(results) = json.get("results").and_then(Value::as_array) else { return Vec::new() };
    results
        .iter()
        .filter_map(|r| {
            let detail = [text(r, "admin1"), text(r, "country")].into_iter().flatten().collect::<Vec<_>>();
            Some(Place {
                name: text(r, "name")?,
                detail: detail.join(", "),
                lat: number(r.get("latitude"))?,
                lon: number(r.get("longitude"))?,
            })
        })
        .collect()
}

/// The places a CEP or a name may be; empty when none was found.
pub async fn search(query: String) -> Vec<Place> {
    if let Some(cep) = cep(&query) {
        if let Some(place) = get(&format!("https://cep.awesomeapi.com.br/json/{cep}")).await.and_then(|b| parse_awesome(&b)) {
            return vec![place];
        }
        return get(&format!("https://brasilapi.com.br/api/cep/v2/{cep}"))
            .await
            .and_then(|b| parse_brasilapi(&b))
            .into_iter()
            .collect();
    }
    let name: String = query.trim().chars().filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-').collect();
    if name.chars().count() < 2 {
        return Vec::new();
    }
    let url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=5&language=pt&format=json",
        name.trim().replace(' ', "+")
    );
    get(&url).await.map(|b| parse_geocoding(&b)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cep_with_or_without_the_dash() {
        assert_eq!(cep("30330-330"), Some("30330330".into()));
        assert_eq!(cep(" 30330330 "), Some("30330330".into()));
        assert_eq!(cep("3033-033"), None);
        assert_eq!(cep("Curitiba"), None);
        assert_eq!(cep("Rua 12345678"), None);
    }

    #[test]
    fn awesomeapi_gives_the_neighborhood_city_and_where() {
        let place = parse_awesome(include_bytes!("../../../tests/fixtures/weather/cep-awesome.json")).unwrap();

        assert_eq!(place.name, "São Pedro, Belo Horizonte");
        assert_eq!(place.detail, "MG");
        assert!((place.lat + 19.948).abs() < 0.01 && (place.lon + 43.937).abs() < 0.01);
    }

    #[test]
    fn brasilapi_as_well() {
        let place = parse_brasilapi(include_bytes!("../../../tests/fixtures/weather/cep-brasilapi.json")).unwrap();

        assert_eq!(place.name, "São Pedro, Belo Horizonte");
        assert!((place.lat + 19.92).abs() < 0.01);
    }

    #[test]
    fn geocoding_lists_cities_with_state_and_country() {
        let places = parse_geocoding(include_bytes!("../../../tests/fixtures/weather/geocoding.json"));

        assert!(!places.is_empty());
        assert_eq!(places[0].name, "Curitiba");
        assert_eq!(places[0].detail, "Paraná, Brasil");
    }

    #[test]
    fn nothing_found_is_an_empty_list() {
        assert!(parse_geocoding(br#"{"generationtime_ms":0.1}"#).is_empty());
        assert!(parse_geocoding(b"not json").is_empty());
        assert!(parse_awesome(br#"{"code":"not_found"}"#).is_none());
    }
}
