//! Incrementally tails Claude Code transcripts and aggregates token usage.

use crate::config::Config;
use chrono::{DateTime, Duration, Local, TimeZone, Timelike, Utc};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const BLOCK_HOURS: i64 = 5;
const ACTIVE_WINDOW_SECS: u64 = 5 * 60;

struct Rec {
    t: DateTime<Utc>,
    model: String,
    proj: String,
    sess: String,
    inp: u64,
    out: u64,
    cw: u64,
    cr: u64,
}

impl Rec {
    fn all(&self) -> u64 {
        self.inp + self.out + self.cw + self.cr
    }
    fn cost(&self, cfg: &Config) -> f64 {
        let p = cfg.price_for(&self.model);
        (self.inp as f64 * p[0] + self.out as f64 * p[1] + self.cw as f64 * p[2] + self.cr as f64 * p[3])
            / 1e6
    }
}

#[derive(Default, Clone, Copy)]
pub struct Sum {
    pub inp: u64,
    pub out: u64,
    pub cw: u64,
    pub cr: u64,
    pub all: u64,
    pub cost: f64,
    pub n: u32,
}

impl Sum {
    fn add(&mut self, r: &Rec, cfg: &Config) {
        self.inp += r.inp;
        self.out += r.out;
        self.cw += r.cw;
        self.cr += r.cr;
        self.all += r.all();
        self.cost += r.cost(cfg);
        self.n += 1;
    }
}

pub struct Day {
    pub label: String,
    pub cost: f64,
}

pub struct Model {
    pub name: String,
    pub cost: f64,
}

pub struct Session {
    pub proj: String,
    pub last: DateTime<Utc>,
    pub all: u64,
    pub cost: f64,
    pub active: bool,
}

pub struct Stats {
    pub now: DateTime<Utc>,
    pub block_start: Option<DateTime<Utc>>,
    pub block_left: f64,
    pub block_frac: f64,
    pub block: Sum,
    pub wbins: Vec<u64>,
    pub cur_bin: i64,
    pub spark: Vec<u64>,
    pub today: Sum,
    pub hour: Sum,
    pub burn: f64,
    pub days: Vec<Day>,
    pub models: Vec<Model>,
    pub sessions: Vec<Session>,
    pub active: usize,
    pub last: Option<DateTime<Utc>>,
}

impl Stats {
    pub fn live(&self) -> bool {
        self.last.map_or(false, |l| (self.now - l).num_seconds() < 120)
    }
}

#[derive(Default)]
pub struct Store {
    root: PathBuf,
    offsets: HashMap<PathBuf, u64>,
    partial: HashMap<PathBuf, Vec<u8>>,
    records: HashMap<String, Rec>,
    session_mtime: HashMap<String, SystemTime>,
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(ft) if ft.is_dir() => walk(&p, out),
            Ok(_) if p.extension().map_or(false, |x| x == "jsonl") => out.push(p),
            _ => {}
        }
    }
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self { root, ..Default::default() }
    }

    fn ids(&self, path: &Path) -> (String, String) {
        let rel: Vec<String> = path
            .strip_prefix(&self.root)
            .map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        if rel.len() > 1 {
            let sess = Path::new(&rel[1]).file_stem().map(|s| s.to_string_lossy().into_owned());
            (rel[0].clone(), sess.unwrap_or_default())
        } else {
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            ("?".into(), stem)
        }
    }

    pub fn scan(&mut self) {
        let mut files = Vec::new();
        walk(&self.root, &mut files);
        for path in files {
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            let (proj, sess) = self.ids(&path);
            if let Ok(m) = meta.modified() {
                let e = self.session_mtime.entry(sess.clone()).or_insert(m);
                if m > *e {
                    *e = m;
                }
            }
            let size = meta.len();
            let mut off = *self.offsets.get(&path).unwrap_or(&0);
            if size < off {
                off = 0; // truncated / rewritten
                self.partial.remove(&path);
            }
            if size == off {
                continue;
            }
            let Ok(mut f) = std::fs::File::open(&path) else { continue };
            if f.seek(SeekFrom::Start(off)).is_err() {
                continue;
            }
            let mut chunk = Vec::with_capacity((size - off) as usize);
            if f.read_to_end(&mut chunk).is_err() {
                continue;
            }
            self.offsets.insert(path.clone(), off + chunk.len() as u64);
            let mut buf = self.partial.remove(&path).unwrap_or_default();
            buf.extend_from_slice(&chunk);
            let mut start = 0;
            for i in 0..buf.len() {
                if buf[i] == b'\n' {
                    self.ingest(&buf[start..i], &path, &proj, &sess);
                    start = i + 1;
                }
            }
            self.partial.insert(path, buf[start..].to_vec());
        }
    }

    fn ingest(&mut self, line: &[u8], path: &Path, proj: &str, sess: &str) {
        if !line.windows(7).any(|w| w == b"\"usage\"") {
            return;
        }
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else { return };
        let msg = &v["message"];
        let usage = &msg["usage"];
        let Some(ts) = v["timestamp"].as_str() else { return };
        if !usage.is_object() {
            return;
        }
        let Ok(t) = DateTime::parse_from_rfc3339(ts) else { return };
        let n = |k: &str| usage[k].as_u64().unwrap_or(0);
        let key = match msg["id"].as_str() {
            Some(id) => format!("{}:{}", id, v["requestId"].as_str().unwrap_or("")),
            None => format!("{}:{}", path.display(), ts),
        };
        self.records.insert(
            key,
            Rec {
                t: t.with_timezone(&Utc),
                model: msg["model"].as_str().unwrap_or("").to_string(),
                proj: proj.to_string(),
                sess: sess.to_string(),
                inp: n("input_tokens"),
                out: n("output_tokens"),
                cw: n("cache_creation_input_tokens"),
                cr: n("cache_read_input_tokens"),
            },
        );
    }

    pub fn stats(&self, cfg: &Config) -> Stats {
        let now = Utc::now();
        let mut recs: Vec<&Rec> = self.records.values().collect();
        recs.sort_by_key(|r| r.t);

        // 5-hour block: starts at the floored hour of the first message after
        // the previous block expired.
        let block = Duration::hours(BLOCK_HOURS);
        let mut start: Option<DateTime<Utc>> = None;
        for r in &recs {
            if start.map_or(true, |s| r.t >= s + block) {
                start = r.t.with_minute(0).and_then(|t| t.with_second(0)).and_then(|t| t.with_nanosecond(0));
            }
        }
        let active_block = start.map_or(false, |s| now < s + block);
        let block_start = if active_block { start } else { None };
        let block_secs = (BLOCK_HOURS * 3600) as f64;
        let block_left = block_start.map_or(0.0, |s| (s + block - now).num_milliseconds() as f64 / 1000.0);

        let mut s_block = Sum::default();
        let nbins = (BLOCK_HOURS * 6) as usize;
        let mut wbins = vec![0u64; nbins];
        let mut spark = vec![0u64; 20];
        let mut s_today = Sum::default();
        let mut s_hour = Sum::default();
        let mut s_10m = Sum::default();

        let today_local = Local::now().date_naive();
        let midnight = |d: chrono::NaiveDate| {
            Local
                .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
                .earliest()
                .map(|t| t.with_timezone(&Utc))
                .unwrap_or(now)
        };
        let today0 = midnight(today_local);
        let mut days: Vec<Day> = (0..7)
            .rev()
            .map(|back| {
                let d = today_local - Duration::days(back);
                Day {
                    label: if back == 0 { "Today".into() } else { d.format("%a").to_string() },
                    cost: 0.0,
                }
            })
            .collect();
        let day_starts: Vec<DateTime<Utc>> =
            (0..7).rev().map(|back| midnight(today_local - Duration::days(back))).collect();

        let mut models: HashMap<String, f64> = HashMap::new();
        let mut sessions: HashMap<&str, Session> = HashMap::new();
        let week_ago = now - Duration::days(7);

        for r in &recs {
            let cost = r.cost(cfg);
            if let Some(bs) = block_start {
                if r.t >= bs {
                    s_block.add(r, cfg);
                    let i = ((r.t - bs).num_seconds() / 600).clamp(0, nbins as i64 - 1) as usize;
                    wbins[i] += r.out;
                }
            }
            let age = (now - r.t).num_seconds();
            if (0..3600).contains(&age) {
                spark[19 - (age / 180) as usize] += r.out;
                s_hour.add(r, cfg);
            }
            if (0..600).contains(&age) {
                s_10m.add(r, cfg);
            }
            if r.t >= today0 {
                s_today.add(r, cfg);
                let name = r.model.trim_start_matches("claude-");
                *models.entry(if name.is_empty() { "unknown".into() } else { name.into() }).or_default() +=
                    cost;
            }
            for (i, d0) in day_starts.iter().enumerate() {
                let d1 = if i + 1 < 7 { day_starts[i + 1] } else { today0 + Duration::days(1) };
                if r.t >= *d0 && r.t < d1 {
                    days[i].cost += cost;
                }
            }
            if r.t >= week_ago {
                let s = sessions.entry(&r.sess).or_insert_with(|| Session {
                    proj: r.proj.clone(),
                    last: r.t,
                    all: 0,
                    cost: 0.0,
                    active: false,
                });
                s.last = s.last.max(r.t);
                s.all += r.all();
                s.cost += cost;
            }
        }

        let sysnow = SystemTime::now();
        for (sid, s) in sessions.iter_mut() {
            s.active = self.session_mtime.get(*sid).map_or(false, |m| {
                sysnow.duration_since(*m).map_or(true, |d| d.as_secs() < ACTIVE_WINDOW_SECS)
            });
        }
        let active = sessions.values().filter(|s| s.active).count();
        let mut sessions: Vec<Session> = sessions.into_values().collect();
        sessions.sort_by(|a, b| b.last.cmp(&a.last));
        let mut models: Vec<Model> = models.into_iter().map(|(name, cost)| Model { name, cost }).collect();
        models.sort_by(|a, b| b.cost.partial_cmp(&a.cost).unwrap_or(std::cmp::Ordering::Equal));

        Stats {
            now,
            block_start,
            block_left,
            block_frac: if block_start.is_some() { 1.0 - block_left / block_secs } else { 0.0 },
            block: s_block,
            wbins,
            cur_bin: block_start.map_or(-1, |s| ((now - s).num_seconds() / 600).min(nbins as i64 - 1)),
            spark,
            today: s_today,
            hour: s_hour,
            burn: s_10m.out as f64 / 10.0,
            days,
            models,
            sessions,
            active,
            last: recs.last().map(|r| r.t),
        }
    }
}

/// Made-up data for README screenshots (`--screenshots`).
pub fn sample_stats() -> Stats {
    let now = Utc::now();
    let start = now - Duration::minutes(2 * 60 + 18);
    let block_left = (BLOCK_HOURS * 3600 - (2 * 60 + 18) * 60) as f64;
    let mut wbins = vec![0u64; (BLOCK_HOURS * 6) as usize];
    for (i, v) in [3200, 8100, 12400, 6100, 0, 2400, 9800, 15200, 11800, 7400, 13900, 4600, 9100, 6800]
        .iter()
        .enumerate()
    {
        wbins[i] = *v;
    }
    let spark = vec![
        420, 0, 880, 1300, 610, 0, 0, 1900, 2600, 1200, 740, 0, 1650, 2300, 3100, 1800, 900, 2700, 3400, 2100,
    ];
    let sum = |inp, out, cw, cr, cost, n| Sum { inp, out, cw, cr, all: inp + out + cw + cr, cost, n };
    let today_local = Local::now().date_naive();
    let costs = [3.20, 0.0, 5.84, 9.12, 4.41, 7.63, 11.27];
    let days = costs
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let back = 6 - i as i64;
            Day {
                label: if back == 0 {
                    "Today".into()
                } else {
                    (today_local - Duration::days(back)).format("%a").to_string()
                },
                cost: *c,
            }
        })
        .collect();
    let session = |proj: &str, mins: i64, all, cost, active| Session {
        proj: proj.into(),
        last: now - Duration::minutes(mins),
        all,
        cost,
        active,
    };
    Stats {
        now,
        block_start: Some(start),
        block_left,
        block_frac: 1.0 - block_left / (BLOCK_HOURS * 3600) as f64,
        block: sum(386, 110_800, 214_500, 4_498_000, 6.94, 87),
        wbins,
        cur_bin: 13,
        spark,
        today: sum(612, 168_300, 341_200, 7_402_000, 11.27, 142),
        hour: sum(144, 31_900, 58_700, 1_892_000, 2.03, 26),
        burn: 2860.0,
        days,
        models: vec![
            Model { name: "opus-5-5".into(), cost: 9.41 },
            Model { name: "sonnet-5-5".into(), cost: 1.52 },
            Model { name: "haiku-5-5".into(), cost: 0.34 },
        ],
        sessions: vec![
            session("C--dev-webapp", 0, 3_910_000, 5.62, true),
            session("C--dev-backend", 26, 1_480_000, 2.17, false),
            session("C--dev-dotfiles", 190, 402_000, 0.61, false),
            session("C--dev-pipeline", 1500, 2_260_000, 3.05, false),
        ],
        active: 1,
        last: Some(now - Duration::seconds(20)),
    }
}

// ---------------------------------------------------------------- format ---
pub fn fmt_tokens(n: f64) -> String {
    if n >= 1e9 {
        format!("{:.2}B", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.2}M", n / 1e6)
    } else if n >= 1e3 {
        format!("{:.1}K", n / 1e3)
    } else {
        format!("{}", n as u64)
    }
}

pub fn fmt_money(v: f64) -> String {
    if v >= 100.0 {
        format!("${:.0}", v)
    } else {
        format!("${:.2}", v)
    }
}

pub fn fmt_left(sec: f64) -> String {
    let m = (sec.max(0.0) as i64) / 60;
    let (h, m) = (m / 60, m % 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else {
        format!("{m}m")
    }
}

pub fn fmt_ago(sec: i64) -> String {
    match sec.max(0) {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86400 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86400),
    }
}

/// "C--Users-me-my-app" -> "app" (best effort; dashes are ambiguous).
pub fn pretty_project(folder: &str) -> String {
    let tail = folder.rsplit('-').find(|s| !s.is_empty()).unwrap_or(folder);
    tail.to_string()
}
