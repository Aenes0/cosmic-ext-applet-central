// SPDX-License-Identifier: GPL-3.0-only

//! The weather: from three services (Open-Meteo, MET Norway and wttr.in),
//! each number the median of those that answered, so one off doesn't pull
//! it; a mark when they disagree. Read every half hour by one applet of the
//! three, written down for the others.

pub mod place;
pub mod scene;
pub mod sources;
pub mod view;

use std::{collections::BTreeMap, path::PathBuf};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

pub use place::Place;

/// How old the weather may get before it's read again.
pub const STALE_AFTER_SECS: i64 = 30 * 60;
/// Degrees apart the services may be before the mark shows.
pub const SPREAD_LIMIT: f32 = 3.0;
/// How many hours ahead, and days.
pub const HOURS: usize = 24;
pub const DAYS: usize = 7;

/// The sky, from the calmest to the roughest: a median of these is the
/// middle way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Sky {
    Clear,
    PartlyCloudy,
    Cloudy,
    Fog,
    Drizzle,
    Showers,
    Rain,
    Thunder,
    Snow,
}

impl Sky {
    /// Open-Meteo's: the WMO's codes.
    pub fn from_wmo(code: u32) -> Option<Sky> {
        Some(match code {
            0 => Sky::Clear,
            1 | 2 => Sky::PartlyCloudy,
            3 => Sky::Cloudy,
            45 | 48 => Sky::Fog,
            51..=57 => Sky::Drizzle,
            61..=67 => Sky::Rain,
            71..=77 | 85 | 86 => Sky::Snow,
            80..=82 => Sky::Showers,
            95..=99 => Sky::Thunder,
            _ => return None,
        })
    }

    /// MET Norway's symbols, as `partlycloudy_day`.
    pub fn from_met(symbol: &str) -> Option<Sky> {
        let name = symbol.split('_').next().unwrap_or(symbol);
        Some(match name {
            "clearsky" => Sky::Clear,
            "fair" | "partlycloudy" => Sky::PartlyCloudy,
            "cloudy" => Sky::Cloudy,
            "fog" => Sky::Fog,
            _ if name.contains("thunder") => Sky::Thunder,
            _ if name.contains("snow") || name.contains("sleet") => Sky::Snow,
            _ if name.contains("showers") => Sky::Showers,
            "lightrain" => Sky::Drizzle,
            "rain" | "heavyrain" => Sky::Rain,
            _ => return None,
        })
    }

    /// wttr.in's: WorldWeatherOnline's codes.
    pub fn from_wwo(code: u32) -> Option<Sky> {
        Some(match code {
            113 => Sky::Clear,
            116 => Sky::PartlyCloudy,
            119 | 122 => Sky::Cloudy,
            143 | 248 | 260 => Sky::Fog,
            263 | 266 | 281 | 284 | 293 | 296 => Sky::Drizzle,
            176 | 353 | 356 | 359 => Sky::Showers,
            299 | 302 | 305 | 308 => Sky::Rain,
            200 | 386 | 389 | 392 | 395 => Sky::Thunder,
            179 | 182 | 185 | 227 | 230 | 311..=350 | 362..=377 => Sky::Snow,
            _ => return None,
        })
    }
}

/// Which service.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Source {
    OpenMeteo,
    MetNorway,
    Wttr,
}

/// What one service says now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NowObs {
    pub temp: Option<f32>,
    pub feels: Option<f32>,
    pub humidity: Option<f32>,
    pub wind_kmh: Option<f32>,
    pub sky: Option<Sky>,
}

/// One hour, from its start, in seconds since the epoch.
#[derive(Clone, Debug, PartialEq)]
pub struct HourObs {
    pub at: i64,
    pub temp: f32,
    pub rain_chance: Option<f32>,
    pub sky: Option<Sky>,
}

/// One day where the weather is.
#[derive(Clone, Debug, PartialEq)]
pub struct DayObs {
    pub date: NaiveDate,
    pub max: f32,
    pub min: f32,
    pub rain_chance: Option<f32>,
    pub sky: Option<Sky>,
    pub uv: Option<f32>,
    /// Seconds since the epoch.
    pub sunrise: Option<i64>,
    pub sunset: Option<i64>,
}

/// All one service says.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub source: Source,
    pub now: NowObs,
    pub hours: Vec<HourObs>,
    pub days: Vec<DayObs>,
    /// Seconds ahead of UTC where the weather is, when it says.
    pub utc_offset: Option<i32>,
}

/// The weather, the services' together.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Weather {
    pub place: Place,
    /// Seconds since the epoch.
    pub fetched_at: i64,
    pub sources: Vec<Source>,
    pub utc_offset: i32,
    pub now: Now,
    pub hours: Vec<Hour>,
    pub days: Vec<Day>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Now {
    pub temp: f32,
    /// The lowest and the highest the services said, when far apart.
    pub spread: Option<(f32, f32)>,
    pub feels: Option<f32>,
    pub humidity: Option<f32>,
    pub wind_kmh: Option<f32>,
    pub sky: Sky,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hour {
    pub at: i64,
    pub temp: f32,
    pub rain_chance: Option<f32>,
    pub sky: Sky,
    pub day: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Day {
    pub date: NaiveDate,
    pub max: f32,
    pub min: f32,
    pub rain_chance: Option<f32>,
    pub sky: Sky,
    pub uv: Option<f32>,
    pub sunrise: Option<i64>,
    pub sunset: Option<i64>,
}

impl Weather {
    /// The light there at this moment: dawn and dusk within 40 minutes of
    /// sunrise and sunset; not knowing them, 6 and 18.
    pub fn light(&self, now: i64) -> scene::Light {
        const TWILIGHT: i64 = 40 * 60;
        let date = local_date(now, self.utc_offset);
        let midnight = now - (now + i64::from(self.utc_offset)).rem_euclid(86_400);
        let (rise, set) = self
            .days
            .iter()
            .find(|d| d.date == date)
            .and_then(|d| d.sunrise.zip(d.sunset))
            .unwrap_or((midnight + 6 * 3600, midnight + 18 * 3600));
        let time = if (now - rise).abs() <= TWILIGHT {
            scene::Time::Dawn
        } else if (now - set).abs() <= TWILIGHT {
            scene::Time::Dusk
        } else if now > rise && now < set {
            scene::Time::Day
        } else {
            scene::Time::Night
        };
        scene::Light {
            time,
            sun: ((now - rise) as f32 / (set - rise).max(1) as f32).clamp(0.0, 1.0),
            moon: scene::moon_phase(now),
            south: self.place.lat < 0.0,
        }
    }

    pub fn is_stale(&self, now: i64) -> bool {
        now - self.fetched_at >= STALE_AFTER_SECS
    }

    /// The hours from the one under way.
    pub fn hours_from(&self, now: i64) -> impl Iterator<Item = &Hour> {
        let start = now - now.rem_euclid(3600);
        self.hours.iter().filter(move |h| h.at >= start)
    }

    /// The days from today, where the weather is.
    pub fn days_from(&self, now: i64) -> impl Iterator<Item = &Day> {
        let today = local_date(now, self.utc_offset);
        self.days.iter().filter(move |d| d.date >= today)
    }

    /// Today's, where the weather is.
    pub fn today(&self, now: i64) -> Option<&Day> {
        self.days_from(now).next()
    }
}

fn is_day(days: &[Day], at: i64, utc_offset: i32) -> bool {
    let date = local_date(at, utc_offset);
    match days.iter().find(|d| d.date == date).and_then(|d| d.sunrise.zip(d.sunset)) {
        Some((rise, set)) => at >= rise && at < set,
        None => {
            let hour = (at + i64::from(utc_offset)).rem_euclid(86_400) / 3600;
            (6..18).contains(&hour)
        }
    }
}

/// The date there, seconds since the epoch.
pub fn local_date(at: i64, utc_offset: i32) -> NaiveDate {
    chrono::DateTime::from_timestamp(at + i64::from(utc_offset), 0)
        .map(|t| t.date_naive())
        .unwrap_or_default()
}

/// The middle value; between the two in the middle, their mean.
pub fn median(values: &mut [f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f32::total_cmp);
    let mid = values.len() / 2;
    Some(if values.len() % 2 == 0 { (values[mid - 1] + values[mid]) / 2.0 } else { values[mid] })
}

/// The middle sky; between two, the rougher, which is the one to know of.
pub fn median_sky(skies: &mut [Sky]) -> Option<Sky> {
    if skies.is_empty() {
        return None;
    }
    skies.sort();
    Some(skies[skies.len() / 2])
}

fn median_of<T>(items: &[T], value: impl Fn(&T) -> Option<f32>) -> Option<f32> {
    median(&mut items.iter().filter_map(value).collect::<Vec<_>>())
}

fn sky_of<T>(items: &[T], sky: impl Fn(&T) -> Option<Sky>) -> Option<Sky> {
    median_sky(&mut items.iter().filter_map(sky).collect::<Vec<_>>())
}

/// The services' reports together; none when none said the temperature.
pub fn merge(place: Place, reports: &[Report], fetched_at: i64) -> Option<Weather> {
    let nows: Vec<&NowObs> = reports.iter().map(|r| &r.now).collect();
    let mut temps: Vec<f32> = nows.iter().filter_map(|n| n.temp).collect();
    let low = temps.iter().copied().fold(f32::INFINITY, f32::min);
    let high = temps.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let temp = median(&mut temps)?;
    let utc_offset = reports
        .iter()
        .find_map(|r| r.utc_offset)
        .unwrap_or_else(|| chrono::Local::now().offset().local_minus_utc());

    let mut days_by: BTreeMap<NaiveDate, Vec<&DayObs>> = BTreeMap::new();
    for day in reports.iter().flat_map(|r| &r.days) {
        days_by.entry(day.date).or_default().push(day);
    }
    let days: Vec<Day> = days_by
        .into_iter()
        .filter_map(|(date, days)| {
            Some(Day {
                date,
                max: median_of(&days, |d| Some(d.max))?,
                min: median_of(&days, |d| Some(d.min))?,
                rain_chance: median_of(&days, |d| d.rain_chance),
                sky: sky_of(&days, |d| d.sky).unwrap_or(Sky::PartlyCloudy),
                uv: median_of(&days, |d| d.uv),
                sunrise: days.iter().find_map(|d| d.sunrise),
                sunset: days.iter().find_map(|d| d.sunset),
            })
        })
        .take(DAYS + 1)
        .collect();

    let mut hours_by: BTreeMap<i64, Vec<&HourObs>> = BTreeMap::new();
    for hour in reports.iter().flat_map(|r| &r.hours) {
        hours_by.entry(hour.at).or_default().push(hour);
    }
    let start = fetched_at - fetched_at.rem_euclid(3600);
    let hours: Vec<Hour> = hours_by
        .into_iter()
        .filter(|(at, _)| *at >= start)
        .filter_map(|(at, hours)| {
            Some(Hour {
                at,
                temp: median_of(&hours, |h| Some(h.temp))?,
                rain_chance: median_of(&hours, |h| h.rain_chance),
                sky: sky_of(&hours, |h| h.sky)?,
                day: is_day(&days, at + 1800, utc_offset),
            })
        })
        .take(HOURS + 1)
        .collect();

    let now = Now {
        temp,
        spread: (high - low > SPREAD_LIMIT).then_some((low, high)),
        feels: median_of(&nows, |n| n.feels),
        humidity: median_of(&nows, |n| n.humidity),
        wind_kmh: median_of(&nows, |n| n.wind_kmh),
        sky: sky_of(&nows, |n| n.sky)
            .or_else(|| hours.first().map(|h| h.sky))
            .unwrap_or(Sky::PartlyCloudy),
    };
    Some(Weather {
        place,
        fetched_at,
        sources: reports.iter().map(|r| r.source).collect(),
        utc_offset,
        now,
        hours,
        days,
    })
}

/// Where it's written down, for the three applets.
fn cache_path() -> PathBuf {
    super::energy::dir().join("weather.json")
}

/// When the written-down weather last changed.
pub fn modified() -> Option<std::time::SystemTime> {
    std::fs::metadata(cache_path()).ok()?.modified().ok()
}

pub fn load() -> Option<Weather> {
    serde_json::from_slice(&std::fs::read(cache_path()).ok()?).ok()
}

pub fn save(weather: &Weather) {
    let path = cache_path();
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let Ok(json) = serde_json::to_vec(weather) else { return };
    let partial = path.with_extension("json.part");
    if std::fs::write(&partial, json).is_ok() {
        _ = std::fs::rename(partial, path);
    }
}

/// Held while this applet reads the weather, so the three don't all ask.
pub struct FetchLock(#[allow(dead_code)] std::fs::File);

impl FetchLock {
    pub fn take() -> Option<FetchLock> {
        use std::os::fd::AsRawFd;
        let dir = super::energy::dir();
        std::fs::create_dir_all(&dir).ok()?;
        let file = std::fs::File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("weather.lock"))
            .ok()?;
        (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0)
            .then_some(FetchLock(file))
    }
}

/// The three services, at once, together; none when none answered.
pub async fn fetch(place: Place) -> Option<Weather> {
    let (a, b, c) = futures::join!(
        sources::open_meteo(&place),
        sources::met_norway(&place),
        sources::wttr(&place),
    );
    let reports: Vec<Report> = [a, b, c].into_iter().flatten().collect();
    let weather = merge(place, &reports, chrono::Utc::now().timestamp())?;
    save(&weather);
    Some(weather)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place() -> Place {
        Place { name: "Belo Horizonte".into(), detail: "MG".into(), lat: -19.95, lon: -43.94 }
    }

    fn report(source: Source, temp: f32, sky: Sky) -> Report {
        Report {
            source,
            now: NowObs { temp: Some(temp), sky: Some(sky), ..NowObs::default() },
            hours: Vec::new(),
            days: Vec::new(),
            utc_offset: Some(-10_800),
        }
    }

    #[test]
    fn the_median_is_not_pulled_by_one_far_off() {
        let reports = [
            report(Source::OpenMeteo, 25.0, Sky::Clear),
            report(Source::MetNorway, 26.0, Sky::Clear),
            report(Source::Wttr, 35.0, Sky::Rain),
        ];

        let weather = merge(place(), &reports, 0).unwrap();

        assert_eq!(weather.now.temp, 26.0);
        assert_eq!(weather.now.sky, Sky::Clear);
        assert_eq!(weather.now.spread, Some((25.0, 35.0)));
    }

    #[test]
    fn close_services_show_no_spread() {
        let reports = [report(Source::OpenMeteo, 25.0, Sky::Clear), report(Source::Wttr, 27.0, Sky::Cloudy)];

        let weather = merge(place(), &reports, 0).unwrap();

        assert_eq!(weather.now.temp, 26.0);
        assert_eq!(weather.now.spread, None);
        // Between two, the rougher sky.
        assert_eq!(weather.now.sky, Sky::Cloudy);
    }

    #[test]
    fn with_no_temperature_from_anyone_there_is_no_weather() {
        let mut empty = report(Source::OpenMeteo, 0.0, Sky::Clear);
        empty.now.temp = None;

        assert!(merge(place(), &[empty], 0).is_none());
    }

    #[test]
    fn hours_are_the_median_of_those_that_have_them() {
        let mut a = report(Source::OpenMeteo, 20.0, Sky::Clear);
        let mut b = report(Source::MetNorway, 20.0, Sky::Clear);
        a.hours = vec![
            HourObs { at: 3600, temp: 20.0, rain_chance: Some(10.0), sky: Some(Sky::Clear) },
            HourObs { at: 7200, temp: 22.0, rain_chance: None, sky: Some(Sky::Rain) },
        ];
        b.hours = vec![HourObs { at: 3600, temp: 24.0, rain_chance: None, sky: Some(Sky::Cloudy) }];

        let weather = merge(place(), &[a, b], 3600).unwrap();

        assert_eq!(weather.hours.len(), 2);
        assert_eq!(weather.hours[0].temp, 22.0);
        assert_eq!(weather.hours[0].rain_chance, Some(10.0));
        assert_eq!(weather.hours[1].sky, Sky::Rain);
    }

    #[test]
    fn hours_gone_by_are_left_out() {
        let mut a = report(Source::OpenMeteo, 20.0, Sky::Clear);
        a.hours = vec![
            HourObs { at: 0, temp: 18.0, rain_chance: None, sky: Some(Sky::Clear) },
            HourObs { at: 3600, temp: 19.0, rain_chance: None, sky: Some(Sky::Clear) },
        ];

        // Half past the second hour.
        let weather = merge(place(), &[a], 5400).unwrap();

        assert_eq!(weather.hours.iter().map(|h| h.at).collect::<Vec<_>>(), vec![3600]);
    }

    #[test]
    fn day_or_night_by_the_sun() {
        let date = local_date(50_000, 0);
        let days = vec![Day {
            date,
            max: 0.0,
            min: 0.0,
            rain_chance: None,
            sky: Sky::Clear,
            uv: None,
            sunrise: Some(20_000),
            sunset: Some(60_000),
        }];

        assert!(is_day(&days, 50_000, 0));
        assert!(!is_day(&days, 70_000, 0));
        assert!(!is_day(&days, 10_000, 0));
    }

    #[test]
    fn dawn_day_dusk_and_night_by_the_sun() {
        let date = local_date(50_000, 0);
        let mut weather = merge(place(), &[report(Source::OpenMeteo, 20.0, Sky::Clear)], 0).unwrap();
        weather.utc_offset = 0;
        weather.days = vec![Day {
            date,
            max: 0.0,
            min: 0.0,
            rain_chance: None,
            sky: Sky::Clear,
            uv: None,
            sunrise: Some(20_000),
            sunset: Some(60_000),
        }];

        assert_eq!(weather.light(20_000 + 600).time, scene::Time::Dawn);
        assert_eq!(weather.light(40_000).time, scene::Time::Day);
        assert_eq!(weather.light(40_000).sun, 0.5);
        assert_eq!(weather.light(60_000 - 600).time, scene::Time::Dusk);
        assert_eq!(weather.light(70_000).time, scene::Time::Night);
        assert!(weather.light(40_000).south);
    }

    #[test]
    fn codes_from_each_service() {
        assert_eq!(Sky::from_wmo(0), Some(Sky::Clear));
        assert_eq!(Sky::from_wmo(81), Some(Sky::Showers));
        assert_eq!(Sky::from_wmo(95), Some(Sky::Thunder));
        assert_eq!(Sky::from_met("partlycloudy_day"), Some(Sky::PartlyCloudy));
        assert_eq!(Sky::from_met("heavyrainshowersandthunder_night"), Some(Sky::Thunder));
        assert_eq!(Sky::from_met("lightrainshowers_day"), Some(Sky::Showers));
        assert_eq!(Sky::from_met("lightrain"), Some(Sky::Drizzle));
        assert_eq!(Sky::from_wwo(113), Some(Sky::Clear));
        assert_eq!(Sky::from_wwo(176), Some(Sky::Showers));
        assert_eq!(Sky::from_wwo(389), Some(Sky::Thunder));
        assert_eq!(Sky::from_wwo(1), None);
    }
}
