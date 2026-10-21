// SPDX-License-Identifier: GPL-3.0-only

//! The three services: what each answers, read into a [`Report`]. Through
//! the system's curl, as the covers are: nothing more to build in.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use serde_json::Value;

use super::{DayObs, HourObs, NowObs, Place, Report, Sky, Source, local_date};

/// MET Norway asks who's asking.
const USER_AGENT: &str = "cosmic-ext-applet-central/0.1 github.com/Aenes0/cosmic-ext-applet-central";

/// A page, or none if it didn't come in 15 s.
pub(super) async fn get(url: &str) -> Option<Vec<u8>> {
    let output = crate::host::command("curl", &[])
        .args(["-sf", "-m", "15", "--compressed", "-A", USER_AGENT, url])
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    if output.status.success() {
        Some(output.stdout)
    } else {
        tracing::warn!("weather: {url} answered nothing ({})", output.status);
        None
    }
}

/// Where the sun stands, for services that don't say the time zone: an
/// hour each 15 degrees.
pub fn guessed_offset(lon: f64) -> i32 {
    ((lon / 15.0).round() as i32) * 3600
}

pub async fn open_meteo(place: &Place) -> Option<Report> {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={:.4}&longitude={:.4}\
         &current=temperature_2m,relative_humidity_2m,apparent_temperature,weather_code,wind_speed_10m\
         &hourly=temperature_2m,precipitation_probability,weather_code\
         &daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max,uv_index_max,sunrise,sunset\
         &timezone=auto&timeformat=unixtime&forecast_days=8&forecast_hours=26",
        place.lat, place.lon
    );
    parse_open_meteo(&get(&url).await?)
}

pub async fn met_norway(place: &Place) -> Option<Report> {
    let url = format!(
        "https://api.met.no/weatherapi/locationforecast/2.0/complete?lat={:.4}&lon={:.4}",
        place.lat, place.lon
    );
    parse_met_norway(&get(&url).await?, guessed_offset(place.lon))
}

pub async fn wttr(place: &Place) -> Option<Report> {
    let url = format!("https://wttr.in/{:.4},{:.4}?format=j1", place.lat, place.lon);
    parse_wttr(&get(&url).await?, guessed_offset(place.lon))
}

fn f32_at(value: &Value) -> Option<f32> {
    match value {
        Value::Number(n) => n.as_f64().map(|v| v as f32),
        // wttr.in says its numbers as text.
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn i64_at(value: &Value) -> Option<i64> {
    value.as_i64()
}

/// The `at`-th of a column of Open-Meteo's.
fn column<'a>(block: &'a Value, name: &str, at: usize) -> Option<&'a Value> {
    block.get(name)?.get(at).filter(|v| !v.is_null())
}

pub fn parse_open_meteo(bytes: &[u8]) -> Option<Report> {
    let json: Value = serde_json::from_slice(bytes).ok()?;
    let utc_offset = json.get("utc_offset_seconds").and_then(Value::as_i64).map(|o| o as i32);
    let offset = utc_offset.unwrap_or(0);
    let current = json.get("current")?;
    let now = NowObs {
        temp: current.get("temperature_2m").and_then(f32_at),
        feels: current.get("apparent_temperature").and_then(f32_at),
        humidity: current.get("relative_humidity_2m").and_then(f32_at),
        wind_kmh: current.get("wind_speed_10m").and_then(f32_at),
        sky: current.get("weather_code").and_then(Value::as_u64).and_then(|c| Sky::from_wmo(c as u32)),
    };
    let hourly = json.get("hourly")?;
    let hours = (0..hourly.get("time").and_then(Value::as_array).map_or(0, Vec::len))
        .filter_map(|i| {
            Some(HourObs {
                at: column(hourly, "time", i).and_then(i64_at)?,
                temp: column(hourly, "temperature_2m", i).and_then(f32_at)?,
                rain_chance: column(hourly, "precipitation_probability", i).and_then(f32_at),
                sky: column(hourly, "weather_code", i)
                    .and_then(Value::as_u64)
                    .and_then(|c| Sky::from_wmo(c as u32)),
            })
        })
        .collect();
    let daily = json.get("daily")?;
    let days = (0..daily.get("time").and_then(Value::as_array).map_or(0, Vec::len))
        .filter_map(|i| {
            Some(DayObs {
                date: local_date(column(daily, "time", i).and_then(i64_at)?, offset),
                max: column(daily, "temperature_2m_max", i).and_then(f32_at)?,
                min: column(daily, "temperature_2m_min", i).and_then(f32_at)?,
                rain_chance: column(daily, "precipitation_probability_max", i).and_then(f32_at),
                sky: column(daily, "weather_code", i)
                    .and_then(Value::as_u64)
                    .and_then(|c| Sky::from_wmo(c as u32)),
                uv: column(daily, "uv_index_max", i).and_then(f32_at),
                sunrise: column(daily, "sunrise", i).and_then(i64_at),
                sunset: column(daily, "sunset", i).and_then(i64_at),
            })
        })
        .collect();
    Some(Report { source: Source::OpenMeteo, now, hours, days, utc_offset })
}

pub fn parse_met_norway(bytes: &[u8], offset: i32) -> Option<Report> {
    let json: Value = serde_json::from_slice(bytes).ok()?;
    let series = json.pointer("/properties/timeseries")?.as_array()?;
    struct Step<'a> {
        at: i64,
        data: &'a Value,
    }
    let steps: Vec<Step> = series
        .iter()
        .filter_map(|s| {
            let at = chrono::DateTime::parse_from_rfc3339(s.get("time")?.as_str()?).ok()?.timestamp();
            Some(Step { at, data: s.get("data")? })
        })
        .collect();
    let instant = |s: &Step, key: &str| s.data.pointer(&format!("/instant/details/{key}")).and_then(f32_at);
    let symbol = |s: &Step, period: &str| {
        s.data
            .pointer(&format!("/{period}/summary/symbol_code"))
            .and_then(Value::as_str)
            .and_then(Sky::from_met)
    };
    let first = steps.first()?;
    let now = NowObs {
        temp: instant(first, "air_temperature"),
        feels: instant(first, "apparent_air_temperature"),
        humidity: instant(first, "relative_humidity"),
        wind_kmh: instant(first, "wind_speed").map(|ms| ms * 3.6),
        sky: symbol(first, "next_1_hours"),
    };
    // Hour by hour while it says so; then every six.
    let hours = steps
        .iter()
        .filter(|s| s.data.get("next_1_hours").is_some())
        .filter_map(|s| {
            Some(HourObs {
                at: s.at,
                temp: instant(s, "air_temperature")?,
                rain_chance: s
                    .data
                    .pointer("/next_1_hours/details/probability_of_precipitation")
                    .and_then(f32_at),
                sky: symbol(s, "next_1_hours"),
            })
        })
        .collect();
    // Each day it covers from the morning: its highest and lowest, and the
    // sky around noon.
    let mut days: Vec<DayObs> = Vec::new();
    let mut by_date: std::collections::BTreeMap<NaiveDate, Vec<&Step>> = Default::default();
    for step in &steps {
        by_date.entry(local_date(step.at, offset)).or_default().push(step);
    }
    for (date, steps) in by_date {
        let local_hour = |s: &Step| (s.at + i64::from(offset)).rem_euclid(86_400) / 3600;
        if steps.first().is_none_or(|s| local_hour(s) > 6) {
            continue;
        }
        let highs = steps.iter().filter_map(|s| {
            s.data.pointer("/next_6_hours/details/air_temperature_max").and_then(f32_at).or(instant(s, "air_temperature"))
        });
        let lows = steps.iter().filter_map(|s| {
            s.data.pointer("/next_6_hours/details/air_temperature_min").and_then(f32_at).or(instant(s, "air_temperature"))
        });
        let (Some(max), Some(min)) = (highs.reduce(f32::max), lows.reduce(f32::min)) else { continue };
        let noon = steps.iter().min_by_key(|s| (local_hour(s) - 12).abs());
        days.push(DayObs {
            date,
            max,
            min,
            rain_chance: None,
            sky: noon.and_then(|s| symbol(s, "next_6_hours").or(symbol(s, "next_1_hours"))),
            uv: None,
            sunrise: None,
            sunset: None,
        });
    }
    Some(Report { source: Source::MetNorway, now, hours, days, utc_offset: None })
}

/// wttr.in's "05:21 AM", that day, there.
fn clock_at(date: NaiveDate, text: &str, offset: i32) -> Option<i64> {
    let time = NaiveTime::parse_from_str(text.trim(), "%I:%M %p").ok()?;
    Some(NaiveDateTime::new(date, time).and_utc().timestamp() - i64::from(offset))
}

pub fn parse_wttr(bytes: &[u8], offset: i32) -> Option<Report> {
    let json: Value = serde_json::from_slice(bytes).ok()?;
    let current = json.get("current_condition")?.get(0)?;
    let code = |v: &Value| v.get("weatherCode").and_then(f32_at).and_then(|c| Sky::from_wwo(c as u32));
    let now = NowObs {
        temp: current.get("temp_C").and_then(f32_at),
        feels: current.get("FeelsLikeC").and_then(f32_at),
        humidity: current.get("humidity").and_then(f32_at),
        wind_kmh: current.get("windspeedKmph").and_then(f32_at),
        sky: code(current),
    };
    let mut hours = Vec::new();
    let mut days = Vec::new();
    for day in json.get("weather")?.as_array()? {
        let Some(date) = day.get("date").and_then(Value::as_str).and_then(|d| d.parse::<NaiveDate>().ok())
        else {
            continue;
        };
        let hourly = day.get("hourly").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut noon = None;
        let mut rain_chance: Option<f32> = None;
        let mut uv: Option<f32> = None;
        for hour in &hourly {
            // "0", "300" … "2100".
            let Some(hhmm) = hour.get("time").and_then(f32_at) else { continue };
            let Some(time) = NaiveTime::from_hms_opt((hhmm as u32) / 100, 0, 0) else { continue };
            let at = NaiveDateTime::new(date, time).and_utc().timestamp() - i64::from(offset);
            let chance = hour.get("chanceofrain").and_then(f32_at);
            rain_chance = match (rain_chance, chance) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
            if let Some(index) = hour.get("uvIndex").and_then(f32_at) {
                uv = Some(uv.map_or(index, |u: f32| u.max(index)));
            }
            if time.hour() == 12 {
                noon = code(hour);
            }
            if let Some(temp) = hour.get("tempC").and_then(f32_at) {
                hours.push(HourObs { at, temp, rain_chance: chance, sky: code(hour) });
            }
        }
        let astronomy = day.pointer("/astronomy/0");
        let sun = |key: &str| {
            astronomy.and_then(|a| a.get(key)).and_then(Value::as_str).and_then(|t| clock_at(date, t, offset))
        };
        let (Some(max), Some(min)) =
            (day.get("maxtempC").and_then(f32_at), day.get("mintempC").and_then(f32_at))
        else {
            continue;
        };
        days.push(DayObs {
            date,
            max,
            min,
            rain_chance,
            sky: noon,
            uv,
            sunrise: sun("sunrise"),
            sunset: sun("sunset"),
        });
    }
    Some(Report { source: Source::Wttr, now, hours, days, utc_offset: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPEN_METEO: &[u8] = include_bytes!("../../../tests/fixtures/weather/open-meteo.json");
    const MET_NORWAY: &[u8] = include_bytes!("../../../tests/fixtures/weather/met-norway.json");
    const WTTR: &[u8] = include_bytes!("../../../tests/fixtures/weather/wttr.json");
    const BH: i32 = -10_800;

    #[test]
    fn open_meteo_reads_now_hours_and_days() {
        let report = parse_open_meteo(OPEN_METEO).unwrap();

        assert_eq!(report.utc_offset, Some(BH));
        assert_eq!(report.now.temp, Some(25.7));
        assert_eq!(report.now.feels, Some(27.2));
        assert_eq!(report.now.humidity, Some(48.0));
        assert_eq!(report.now.sky, Some(Sky::Cloudy));
        assert_eq!(report.hours.len(), 24);
        assert_eq!(report.days.len(), 7);
        let today = &report.days[0];
        assert_eq!(today.date, NaiveDate::from_ymd_opt(2026, 10, 21).unwrap());
        assert_eq!((today.max, today.min), (28.2, 17.3));
        assert_eq!(today.rain_chance, Some(67.0));
        assert_eq!(today.sky, Some(Sky::Drizzle));
        assert!(today.sunrise.unwrap() < today.sunset.unwrap());
    }

    #[test]
    fn met_norway_reads_now_hours_and_whole_days_only() {
        let report = parse_met_norway(MET_NORWAY, BH).unwrap();

        assert_eq!(report.now.temp, Some(24.9));
        assert_eq!(report.now.feels, Some(25.8));
        // 2.6 m/s.
        assert!((report.now.wind_kmh.unwrap() - 9.36).abs() < 0.01);
        assert_eq!(report.now.sky, Some(Sky::Cloudy));
        assert!(report.hours.len() >= 24);
        assert!(report.hours.windows(2).all(|w| w[1].at - w[0].at == 3600));
        // Today began before its first step (11h there): not whole, left out.
        let first = report.days.first().unwrap();
        assert_eq!(first.date, NaiveDate::from_ymd_opt(2026, 10, 22).unwrap());
        assert!(report.days.iter().all(|d| d.max >= d.min));
    }

    #[test]
    fn wttr_reads_text_numbers_and_local_hours() {
        let report = parse_wttr(WTTR, BH).unwrap();

        assert_eq!(report.now.temp, Some(25.0));
        assert_eq!(report.now.feels, Some(25.0));
        assert_eq!(report.now.humidity, Some(56.0));
        assert_eq!(report.now.sky, Some(Sky::Cloudy));
        assert_eq!(report.days.len(), 3);
        let today = &report.days[0];
        assert_eq!((today.max, today.min), (29.0, 18.0));
        // 05:21 there is 08:21 UTC.
        let sunrise = chrono::DateTime::from_timestamp(today.sunrise.unwrap(), 0).unwrap();
        assert_eq!((sunrise.hour(), sunrise.minute()), (8, 21));
        // Every three hours.
        assert_eq!(report.hours.len(), 24);
        assert_eq!(report.hours[1].at - report.hours[0].at, 3 * 3600);
    }

    #[test]
    fn the_offset_guessed_from_the_longitude() {
        assert_eq!(guessed_offset(-43.94), BH);
        assert_eq!(guessed_offset(139.7), 9 * 3600);
        assert_eq!(guessed_offset(0.1), 0);
    }

    #[test]
    fn the_three_together_make_one_weather() {
        let reports = [
            parse_open_meteo(OPEN_METEO).unwrap(),
            parse_met_norway(MET_NORWAY, BH).unwrap(),
            parse_wttr(WTTR, BH).unwrap(),
        ];
        let place = Place { name: "Belo Horizonte".into(), detail: "MG".into(), lat: -19.95, lon: -43.94 };
        let fetched_at = reports[0].hours[0].at + 60;

        let weather = super::super::merge(place, &reports, fetched_at).unwrap();

        assert_eq!(weather.sources.len(), 3);
        assert_eq!(weather.now.temp, 25.0);
        assert_eq!(weather.now.spread, None);
        assert!(weather.hours.len() >= 24);
        assert!(weather.days.len() >= 7);
        assert_eq!(weather.utc_offset, BH);
    }
}
