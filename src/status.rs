//! Claude's service status, from the public status page (status.claude.com):
//! the overall state, the main services (claude.ai, API, Claude Code, ...)
//! and any open incidents.
//!
//! A background thread asks `GET https://status.claude.com/api/v2/summary.json`
//! every couple of minutes. The request carries no login or other data.

use chrono::{DateTime, Utc};
use serde_json::Value;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const PAGE: &str = "https://status.claude.com";
const URL: &str = "https://status.claude.com/api/v2/summary.json";
const EVERY: Duration = Duration::from_secs(2 * 60);
/// Never ask more often than this, even when the drawer is opened repeatedly.
const MIN_GAP: Duration = Duration::from_secs(30);

/// How bad things are, worst last (so `max()` gives the worst).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Operational,
    Maintenance,
    Degraded,
    Partial,
    Major,
}

impl Level {
    fn from_component(s: &str) -> Level {
        match s {
            "operational" => Level::Operational,
            "under_maintenance" => Level::Maintenance,
            "degraded_performance" => Level::Degraded,
            "partial_outage" => Level::Partial,
            "major_outage" => Level::Major,
            _ => Level::Degraded,
        }
    }

    /// Page indicator or incident impact: none / minor / major / critical / maintenance.
    fn from_impact(s: &str) -> Level {
        match s {
            "none" => Level::Operational,
            "maintenance" => Level::Maintenance,
            "minor" => Level::Degraded,
            "major" => Level::Partial,
            "critical" => Level::Major,
            _ => Level::Degraded,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Level::Operational => "Operational",
            Level::Maintenance => "Maintenance",
            Level::Degraded => "Degraded",
            Level::Partial => "Partial outage",
            Level::Major => "Major outage",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    /// Short name, e.g. "API" for "Claude API (api.anthropic.com)".
    pub name: String,
    pub level: Level,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Incident {
    pub name: String,
    pub impact: Level,
    /// "Investigating", "Identified", "Monitoring", ...
    pub stage: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Service {
    pub overall: Level,
    /// "All Systems Operational", "Partial System Outage", ...
    pub description: String,
    pub components: Vec<Component>,
    pub incidents: Vec<Incident>,
    pub fetched_at: DateTime<Utc>,
}

impl Service {
    /// The worst of the page indicator, the services and the incidents.
    pub fn worst(&self) -> Level {
        self.components
            .iter()
            .map(|c| c.level)
            .chain(self.incidents.iter().map(|i| i.impact))
            .chain([self.overall])
            .max()
            .unwrap_or(Level::Operational)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Snapshot {
    pub enabled: bool,
    /// Why the last fetch failed, if it did.
    pub error: Option<String>,
    /// The last good reading, kept while later fetches fail.
    pub service: Option<Service>,
}

/// Services always shown, in this order; any other service is shown only
/// while it has a problem.
const MAIN: [&str; 5] = ["claude.ai", "Code", "API", "Cowork", "Console"];

/// "Claude API (api.anthropic.com)" -> "API", "Claude for Government" -> "Government".
fn short(name: &str) -> String {
    let n = name.split(" (").next().unwrap_or(name).trim();
    let n = n.strip_prefix("Claude ").unwrap_or(n);
    let n = n
        .strip_prefix("for ")
        .or_else(|| n.strip_prefix("on "))
        .or_else(|| n.strip_prefix("in "))
        .unwrap_or(n);
    n.to_string()
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

pub fn parse(body: &str, now: DateTime<Utc>) -> Option<Service> {
    let v: Value = serde_json::from_str(body).ok()?;
    let st = v.get("status")?;
    let overall = Level::from_impact(st["indicator"].as_str()?);
    let description = st["description"].as_str().unwrap_or("").to_string();
    // Top-level services only (a group stands for its members).
    let all: Vec<Component> = v["components"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["group_id"].is_null())
        .filter_map(|c| {
            Some(Component {
                name: short(c["name"].as_str()?),
                level: Level::from_component(c["status"].as_str()?),
            })
        })
        .collect();
    let mut components: Vec<Component> = MAIN
        .iter()
        .filter_map(|m| all.iter().find(|c| c.name == *m).cloned())
        .collect();
    components.extend(
        all.into_iter()
            .filter(|c| !MAIN.contains(&c.name.as_str()) && c.level != Level::Operational),
    );
    let incidents = v["incidents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|i| !matches!(i["status"].as_str(), Some("resolved" | "postmortem")))
        .filter_map(|i| {
            Some(Incident {
                name: i["name"].as_str()?.to_string(),
                impact: Level::from_impact(i["impact"].as_str().unwrap_or("minor")),
                stage: title_case(i["status"].as_str().unwrap_or("")),
            })
        })
        .collect();
    Some(Service {
        overall,
        description,
        components,
        incidents,
        fetched_at: now,
    })
}

// -------------------------------------------------------------------- cache --
/// The last good reply is kept on disk, so the drawer shows the last known
/// status (with its age) when the live check fails, even right after start.
fn cache_path() -> std::path::PathBuf {
    crate::config::data_dir().join("status-cache.json")
}

fn save_cache(body: &str, at: DateTime<Utc>) {
    let v = serde_json::json!({ "fetched_at": at.to_rfc3339(), "body": body });
    let _ = std::fs::write(cache_path(), v.to_string());
}

fn load_cache() -> Option<Service> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(cache_path()).ok()?).ok()?;
    let at = DateTime::parse_from_rfc3339(v["fetched_at"].as_str()?).ok()?;
    parse(v["body"].as_str()?, at.with_timezone(&Utc))
}

fn fetch() -> Result<String, String> {
    let res = crate::limits::agent()?.get(URL).call();
    match res {
        Ok(r) => r.into_string().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, _)) => Err(format!("HTTP {code}")),
        Err(e) => Err(match e.kind() {
            ureq::ErrorKind::Dns | ureq::ErrorKind::ConnectionFailed => "offline".into(),
            _ => "network error".into(),
        }),
    }
}

/// Runs the background fetcher. Cheap to query from the UI thread.
pub struct Fetcher {
    shared: Arc<Mutex<Snapshot>>,
    wake: Sender<()>,
}

impl Fetcher {
    pub fn start(enabled: bool) -> Fetcher {
        let shared = Arc::new(Mutex::new(Snapshot {
            enabled,
            error: None,
            service: if enabled { load_cache() } else { None },
        }));
        let (wake, rx) = mpsc::channel::<()>();
        let s = shared.clone();
        std::thread::spawn(move || {
            let mut last: Option<Instant> = None;
            loop {
                let enabled = s.lock().map(|g| g.enabled).unwrap_or(false);
                if enabled && last.is_none_or(|t| t.elapsed() >= MIN_GAP) {
                    last = Some(Instant::now());
                    let now = Utc::now();
                    let got = fetch().and_then(|b| {
                        let sv = parse(&b, now).ok_or_else(|| "unexpected reply".to_string())?;
                        save_cache(&b, now);
                        Ok(sv)
                    });
                    if let Ok(mut g) = s.lock() {
                        if g.enabled {
                            match got {
                                Ok(sv) => {
                                    g.service = Some(sv);
                                    g.error = None;
                                }
                                Err(e) => g.error = Some(e),
                            }
                        }
                    }
                }
                match rx.recv_timeout(EVERY) {
                    Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        });
        Fetcher { shared, wake }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Ask again soon (e.g. the drawer was opened). Rate-limited.
    pub fn poke(&self) {
        let _ = self.wake.send(());
    }

    pub fn set_enabled(&self, on: bool) {
        if let Ok(mut g) = self.shared.lock() {
            *g = Snapshot {
                enabled: on,
                error: None,
                service: if on { load_cache() } else { None },
            };
        }
        self.poke();
    }
}

/// Made-up reading for `--demo` and the README screenshots.
pub fn sample(now: DateTime<Utc>) -> Snapshot {
    let c = |name: &str| Component {
        name: name.into(),
        level: Level::Operational,
    };
    Snapshot {
        enabled: true,
        error: None,
        service: Some(Service {
            overall: Level::Operational,
            description: "All Systems Operational".into(),
            components: MAIN.iter().map(|n| c(n)).collect(),
            incidents: Vec::new(),
            fetched_at: now - chrono::Duration::minutes(1),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = r#"{"status":{"indicator":"minor","description":"Minor Service Outage"},
      "components":[
        {"name":"claude.ai","status":"operational","group_id":null,"group":false},
        {"name":"Claude API (api.anthropic.com)","status":"degraded_performance","group_id":null,"group":false},
        {"name":"Claude Code","status":"operational","group_id":null,"group":false},
        {"name":"Claude Platform on AWS","status":"major_outage","group_id":"g1","group":false},
        {"name":"Claude for Government","status":"operational","group_id":null,"group":false},
        {"name":"Claude on Cloud Platforms","status":"partial_outage","group_id":null,"group":true}],
      "incidents":[{"name":"Elevated errors on the API","status":"investigating","impact":"minor"},
        {"name":"Old one","status":"resolved","impact":"major"}]}"#;

    #[test]
    fn parses_summary() {
        let s = parse(REPLY, Utc::now()).expect("status");
        assert_eq!(s.overall, Level::Degraded);
        assert_eq!(s.description, "Minor Service Outage");
        let names: Vec<&str> = s.components.iter().map(|c| c.name.as_str()).collect();
        // main services first, then other top-level ones only when not operational
        assert_eq!(names, ["claude.ai", "Code", "API", "Cloud Platforms"]);
        assert_eq!(s.components[2].level, Level::Degraded);
        assert_eq!(s.incidents.len(), 1);
        assert_eq!(s.incidents[0].stage, "Investigating");
        assert_eq!(s.worst(), Level::Partial);
    }

    #[test]
    fn rejects_bad_replies() {
        assert!(parse("{}", Utc::now()).is_none());
        assert!(parse("nope", Utc::now()).is_none());
    }
}
