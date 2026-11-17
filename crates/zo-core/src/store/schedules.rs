//! Scheduled tasks: things Zohara Agent should do later, once or on a repeat. Made on the Scheduled page or by asking in a chat.
//! See docs/SCHEDULED.md for what people are told, and docs/MEMORY-AND-SCHEDULE-PLAN.md / docs/VM-MIGRATION-PLAN.md for the plan.
//!
//! This file is the list itself (what a task is, how its times are read and moved on, what is kept of each run). Running a task is
//! `crate::scheduler`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

/// What a task is.
pub const KINDS: [&str; 3] = ["task", "reminder", "browser"];

/// How many runs a task remembers.
pub const HISTORY_MAX: usize = 20;

/// One run of a task, as kept in its history.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Run {
    /// When it ended, in milliseconds since 1970.
    pub time: u64,
    /// ok | failed | skipped (it could not start: no model, the machine is missing, a limit)
    pub status: String,
    /// The final answer (or the reason it failed), cut short.
    #[serde(default)]
    pub summary: String,
    /// The chat the run left behind (a reminder leaves none).
    #[serde(default)]
    pub chat: Option<String>,
    /// How long it took.
    #[serde(default)]
    pub ms: u64,
    /// The time it should have run at, when the computer was off then (the run is a catch-up).
    #[serde(default)]
    pub missed: Option<u64>,
    /// "scheduled" or "manual" (Run now).
    #[serde(default)]
    pub how: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Task {
    pub id: String,
    /// A short name for lists.
    pub title: String,
    /// What Zohara Agent is asked to do when it is time, written as an instruction (for a reminder: the text of the reminder).
    pub prompt: String,
    /// When it runs next, in milliseconds since 1970.
    pub next_run: u64,
    /// once, hourly, daily, weekdays, weekly, monthly, or "every N minutes|hours|days|weeks|months".
    pub repeat: String,
    pub enabled: bool,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub last_run: Option<u64>,
    /// The chat the last run left behind.
    #[serde(default)]
    pub last_chat: Option<String>,
    /// task (the agent does it with its tools), reminder (a notification only, no model) or browser (the agent may use the browser and
    /// web tools; it always asks before it changes anything). Tasks made before kinds existed are tasks.
    #[serde(default)]
    pub kind: String,
    /// The permission mode of the task: "auto" or "manual". None: the mode the person has set. Never bypass.
    #[serde(default)]
    pub mode: Option<String>,
    /// Where it runs: "" or "this" (this computer, which must be on), "cloud" (the Zohara Agent server) or "machine:<name>" (an added
    /// machine, worked on through the SSH tools from the host that owns the task).
    #[serde(default)]
    pub run_on: String,
    /// The person's time zone (for example "Asia/Karachi"), so that "9am" means their 9am on a server that keeps UTC.
    #[serde(default)]
    pub tz: Option<String>,
    /// The day of the month a monthly task keeps to ("on the 31st" is the last day in a shorter month and the 31st again after it).
    #[serde(default)]
    pub day: Option<u32>,
    /// ok | failed | skipped; None before the first run.
    #[serde(default)]
    pub last_status: Option<String>,
    /// A short result of the last run (the end of the answer, or why it failed).
    #[serde(default)]
    pub last_output: Option<String>,
    /// The last runs, newest first, at most `HISTORY_MAX`.
    #[serde(default)]
    pub history: Vec<Run>,
    /// Set when somebody asked for "Run now" on a host that will do it on its next look (the cloud runner); cleared when it ran.
    #[serde(default)]
    pub run_requested: bool,
}

impl Task {
    /// The kind, with the old default.
    pub fn kind(&self) -> &str {
        if KINDS.contains(&self.kind.as_str()) { &self.kind } else { "task" }
    }
    /// "this", "cloud" or "machine:<name>".
    pub fn run_on(&self) -> &str {
        if self.run_on.is_empty() { "this" } else { &self.run_on }
    }
    pub fn machine(&self) -> Option<&str> {
        self.run_on.strip_prefix("machine:")
    }
}

pub const REPEATS: [&str; 6] = ["once", "hourly", "daily", "weekdays", "weekly", "monthly"];

/// The shortest repeat there is: a task that runs more often than this would swamp the computer.
pub const MIN_EVERY_MS: u64 = 5 * 60_000;

/// Reads a repeat: one of `REPEATS`, or "every N minutes|hours|days|weeks|months" ("every 3 days", "every 90 minutes"; "every other
/// week" is not understood), or "weekday(s)". Returns the form that is stored.
pub fn normalize_repeat(text: &str) -> Result<String, String> {
    let t = text.trim().to_lowercase();
    let t = match t.as_str() {
        "" => "once".to_string(),
        "every day" | "everyday" => "daily".to_string(),
        "every hour" => "hourly".to_string(),
        "every week" => "weekly".to_string(),
        "every month" | "each month" => "monthly".to_string(),
        "weekday" | "every weekday" | "on weekdays" | "every weekdays" => "weekdays".to_string(),
        _ => t,
    };
    if REPEATS.contains(&t.as_str()) {
        return Ok(t);
    }
    let words: Vec<&str> = t.split_whitespace().collect();
    if let ["every", n, unit] = words.as_slice() {
        let n: u64 = n.parse().map_err(|_| "After \"every\" give a number, for example: every 3 days.".to_string())?;
        let unit = unit.trim_end_matches('s');
        if n == 0 {
            return Err("Every 0 of anything is not a repeat.".into());
        }
        let ok = ["minute", "hour", "day", "week", "month"].contains(&unit);
        if !ok {
            return Err("The unit must be minutes, hours, days, weeks or months.".into());
        }
        if n > 10_000 {
            return Err("That is too long a repeat.".into());
        }
        let r = format!("every {n} {unit}{}", if n == 1 { "" } else { "s" });
        return match interval_ms(&r) {
            Some(ms) if ms >= MIN_EVERY_MS => Ok(r),
            Some(_) => Err("A task can repeat at most every 5 minutes.".into()),
            None => Ok(r), // calendar units
        };
    }
    Err(format!("repeat must be one of: {}, or something like \"every 3 days\" (every N minutes, hours, days, weeks or months).", REPEATS.join(", ")))
}

/// The length of a repeat in milliseconds, when it is a fixed length (days, weeks, weekdays and months follow the calendar).
fn interval_ms(repeat: &str) -> Option<u64> {
    match repeat {
        "hourly" => Some(3_600_000),
        _ => {
            let words: Vec<&str> = repeat.split_whitespace().collect();
            let ["every", n, unit] = words.as_slice() else { return None };
            let n: u64 = n.parse().ok()?;
            let unit = unit.trim_end_matches('s');
            Some(n * match unit {
                "minute" => 60_000,
                "hour" => 3_600_000,
                _ => return None,
            })
        }
    }
}

/// A repeat that follows the calendar: (days to step, months to step, weekdays only).
fn calendar_step(repeat: &str) -> Option<(i64, i64, bool)> {
    match repeat {
        "daily" => return Some((1, 0, false)),
        "weekdays" => return Some((1, 0, true)),
        "weekly" => return Some((7, 0, false)),
        "monthly" => return Some((0, 1, false)),
        _ => {}
    }
    let words: Vec<&str> = repeat.split_whitespace().collect();
    let ["every", n, unit] = words.as_slice() else { return None };
    let n: i64 = n.parse().ok()?;
    match unit.trim_end_matches('s') {
        "day" => Some((n, 0, false)),
        "week" => Some((7 * n, 0, false)),
        "month" => Some((0, n, false)),
        _ => None,
    }
}

// ── calendar, in the person's own time zone, by the system's `date` ──

/// A local date and time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Civil {
    pub y: i64,
    pub mo: i64,
    pub d: i64,
    pub h: i64,
    pub mi: i64,
    pub s: i64,
    /// 1 = Monday ... 7 = Sunday.
    pub wd: i64,
}

/// A time zone name that is safe to give to `date` (letters, digits and `_ / + - :`), or None.
pub fn clean_tz(tz: Option<&str>) -> Option<String> {
    let t = tz?.trim();
    (!t.is_empty() && t.len() <= 64 && !t.starts_with('/') && !t.contains("..") && t.chars().all(|c| c.is_ascii_alphanumeric() || "_/+-:".contains(c))).then(|| t.to_string())
}

fn date_cmd(tz: Option<&str>) -> Command {
    let mut c = Command::new("date");
    c.env("LC_ALL", "C");
    if let Some(tz) = clean_tz(tz) {
        c.env("TZ", tz);
    }
    c
}

pub fn local_parts(ms: u64, tz: Option<&str>) -> Option<Civil> {
    let out = date_cmd(tz).args(["-d", &format!("@{}", ms / 1000), "+%Y %m %d %H %M %S %u"]).output().ok()?;
    let v: Vec<i64> = String::from_utf8_lossy(&out.stdout).split_whitespace().filter_map(|x| x.parse().ok()).collect();
    (v.len() == 7).then(|| Civil { y: v[0], mo: v[1], d: v[2], h: v[3], mi: v[4], s: v[5], wd: v[6] })
}

/// The moment of a local date and time (a time that does not exist because the clock jumped forward is moved on by an hour).
pub fn from_local(c: &Civil, tz: Option<&str>) -> Option<u64> {
    for extra in [0, 1] {
        let text = format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", c.y, c.mo, c.d, c.h + extra, c.mi, c.s);
        let out = date_cmd(tz).args(["-d", &text, "+%s"]).output().ok()?;
        if out.status.success() {
            if let Ok(secs) = String::from_utf8_lossy(&out.stdout).trim().parse::<u64>() {
                return Some(secs * 1000);
            }
        }
    }
    None
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 29 } else { 28 },
    }
}

fn add_days(c: &Civil, n: i64) -> Civil {
    let days = days_from_civil(c.y, c.mo, c.d) + n;
    let (y, mo, d) = civil_from_days(days);
    Civil { y, mo, d, wd: (days + 3).rem_euclid(7) + 1, ..*c } // 1970-01-01 was a Thursday
}

/// `c` moved on by `n` months; the day is `day` (or the same day), or the last day of a shorter month.
fn add_months(c: &Civil, n: i64, day: Option<i64>) -> Civil {
    let idx = c.y * 12 + (c.mo - 1) + n;
    let (y, mo) = (idx.div_euclid(12), idx.rem_euclid(12) + 1);
    let want = day.unwrap_or(c.d);
    Civil { y, mo, d: want.min(days_in_month(y, mo)), ..*c }
}

/// The next time after `after` that a repeating task runs, counted from `next_run`. None for "once" (and for a repeat that is not understood).
pub fn next_after(next_run: u64, repeat: &str, tz: Option<&str>, day: Option<u32>, after: u64) -> Option<u64> {
    if let Some(step) = interval_ms(repeat).filter(|s| *s > 0) {
        let mut t = next_run;
        if t <= after {
            t += ((after - t) / step + 1) * step;
        }
        return Some(t);
    }
    let (days, months, weekdays) = calendar_step(repeat)?;
    if next_run > after {
        return Some(next_run);
    }
    let mut c = local_parts(next_run, tz)?;
    // skip most of a long absence at once, then walk the rest
    let gap_days = (after.saturating_sub(next_run) / 86_400_000) as i64;
    if days > 1 && !weekdays {
        let skip = (gap_days / days - 1).max(0) * days;
        c = add_days(&c, skip);
    } else if days == 1 && !weekdays {
        c = add_days(&c, (gap_days - 2).max(0));
    } else if weekdays {
        c = add_days(&c, (gap_days - 7).max(0));
        while c.wd >= 6 {
            c = add_days(&c, 1);
        }
    } else if months > 0 {
        let gap_months = gap_days / 31;
        let skip = (gap_months / months - 1).max(0) * months;
        c = add_months(&c, skip, day.map(i64::from));
    }
    for _ in 0..4000 {
        if let Some(t) = from_local(&c, tz) {
            if t > after {
                return Some(t);
            }
        }
        c = if weekdays {
            let mut n = add_days(&c, 1);
            while n.wd >= 6 {
                n = add_days(&n, 1);
            }
            n
        } else if months > 0 {
            add_months(&c, months, day.map(i64::from))
        } else {
            add_days(&c, days)
        };
    }
    None
}

/// The next `n` times a task would run, the first being `first` itself (a time in the past is first moved to the next one).
pub fn next_runs(first: u64, repeat: &str, tz: Option<&str>, day: Option<u32>, n: usize, now: u64) -> Vec<u64> {
    let mut out = Vec::new();
    let mut t = first;
    if t < now {
        match next_after(t, repeat, tz, day, now) {
            Some(x) => t = x,
            None => return out,
        }
    }
    out.push(t);
    while out.len() < n {
        match next_after(t, repeat, tz, day, t) {
            Some(x) if x > t => {
                out.push(x);
                t = x;
            }
            _ => break,
        }
    }
    out
}

fn path() -> PathBuf {
    crate::audit::data_dir().join("schedules.json")
}

/// One writer at a time inside this process (the scheduler thread, the service and the app all change the list).
static LOCK: Mutex<()> = Mutex::new(());

pub fn now_millis() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn read_all() -> Vec<Task> {
    std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Soonest first.
pub fn load() -> Vec<Task> {
    let mut v = read_all();
    v.sort_by_key(|t| t.next_run);
    v
}

pub fn get(id: &str) -> Option<Task> {
    read_all().into_iter().find(|t| t.id == id)
}

fn write(all: &[Task]) {
    let p = path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = p.with_extension("json.tmp");
    if let Ok(s) = serde_json::to_string_pretty(all) {
        if std::fs::write(&tmp, s).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

/// Reads the list, lets `f` change it, and writes it back, as one step.
fn change<T>(f: impl FnOnce(&mut Vec<Task>) -> T) -> T {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = read_all();
    let r = f(&mut all);
    write(&all);
    r
}

/// Replaces the whole list with what `f` makes of it, as one step. For the VM runner, whose list is a copy of the owner's (see
/// `runner::remote`).
pub fn replace_all(f: impl FnOnce(Vec<Task>) -> Vec<Task>) {
    change(|all| {
        let old = std::mem::take(all);
        *all = f(old);
    })
}

// ── reading times written in words ──

/// What a time written in words means.
#[derive(Clone, Debug, PartialEq)]
pub struct When {
    /// The first run.
    pub at: u64,
    /// A repeat that the words themselves said ("every weekday at 9"); None when they did not say one.
    pub repeat: Option<String>,
    /// The day of the month, for "on the 1st of each month".
    pub day: Option<u32>,
}

const WEEKDAYS: [(&str, i64); 7] = [("monday", 1), ("tuesday", 2), ("wednesday", 3), ("thursday", 4), ("friday", 5), ("saturday", 6), ("sunday", 7)];

fn weekday_of(w: &str) -> Option<i64> {
    let w = w.trim_end_matches('s'); // "mondays"
    WEEKDAYS.iter().find(|(n, _)| *n == w || (w.len() >= 3 && n.starts_with(w) && w.len() <= 4)).map(|(_, i)| *i)
}

/// "9", "9am", "9:30pm", "07:30", "noon", "midnight" -> (hour, minute).
fn parse_clock(t: &str) -> Option<(i64, i64)> {
    let t = t.trim().trim_start_matches("at ").trim();
    match t {
        "noon" => return Some((12, 0)),
        "midnight" => return Some((0, 0)),
        _ => {}
    }
    let (num, pm) = if let Some(n) = t.strip_suffix("pm") { (n.trim(), Some(true)) } else if let Some(n) = t.strip_suffix("am") { (n.trim(), Some(false)) } else { (t, None) };
    let (h, m) = match num.split_once(':') {
        Some((h, m)) => (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?),
        None => (num.parse::<i64>().ok()?, 0),
    };
    if !(0..60).contains(&m) {
        return None;
    }
    let h = match pm {
        Some(true) if (1..=12).contains(&h) => h % 12 + 12,
        Some(false) if (1..=12).contains(&h) => h % 12,
        Some(_) => return None,
        None if (0..24).contains(&h) => h,
        None => return None,
    };
    Some((h, m))
}

/// The first moment at or after `now` that is `hh:mm` on a day the test accepts, looking at most 400 days ahead.
fn first_day(now: u64, tz: Option<&str>, (h, m): (i64, i64), ok: impl Fn(&Civil) -> bool) -> Option<u64> {
    let today = local_parts(now, tz)?;
    for n in 0..400 {
        let c = add_days(&Civil { h, mi: m, s: 0, ..today }, n);
        if !ok(&c) {
            continue;
        }
        if let Some(t) = from_local(&c, tz) {
            if t > now {
                return Some(t);
            }
        }
    }
    None
}

const DEFAULT_CLOCK: (i64, i64) = (9, 0);

fn ordinal(t: &str) -> Option<u32> {
    let t = t.trim_end_matches("st").trim_end_matches("nd").trim_end_matches("rd").trim_end_matches("th");
    t.parse::<u32>().ok().filter(|d| (1..=31).contains(d))
}

fn split_at_clock(t: &str) -> (&str, Option<&str>) {
    match t.rsplit_once(" at ") {
        Some((a, b)) => (a.trim(), Some(b.trim())),
        None => (t, None),
    }
}

/// Reads a time written in words, in the time zone `tz` (the system's when None), relative to `now`.
/// "in 2 hours", "in 30 minutes", "tomorrow 9am", "next monday", "2026-11-05 18:30" (the system's `date` reads these),
/// and ones that also say a repeat: "every weekday at 9", "every day at 07:30", "every monday at 9am", "daily at 8",
/// "on the 1st of each month", "on the 15th of every month at 6pm".
pub fn parse_schedule(text: &str, tz: Option<&str>, now: u64) -> Result<When, String> {
    let t = text.trim().to_lowercase();
    if t.is_empty() {
        return Err("Say when, for example \"tomorrow 9am\", \"in 2 hours\" or \"every weekday at 9\".".into());
    }
    if t.len() > 80 || t.starts_with('-') || t.chars().any(char::is_control) {
        return Err("That doesn't look like a time.".into());
    }
    let nope = || format!("I couldn't read \"{}\" as a time. Try \"tomorrow 9am\", \"in 2 hours\", \"every weekday at 9\" or \"2026-11-05 18:30\".", text.trim());
    let words: Vec<&str> = t.split_whitespace().collect();

    // in N minutes / hours / days / weeks / months
    let rel = match words.as_slice() {
        ["in", n, unit] | ["after", n, unit] => Some((*n, *unit)),
        [n, unit, "from", "now"] => Some((*n, *unit)),
        _ => None,
    };
    if let Some((n, unit)) = rel {
        let n: i64 = match n {
            "a" | "an" | "one" => 1,
            "half" => 0,
            n => n.parse().map_err(|_| nope())?,
        };
        if !(1..=100_000).contains(&n) {
            return Err(nope());
        }
        let u = unit.trim_end_matches('s');
        let u = match u {
            "min" | "minute" => "minute",
            "hr" | "hour" => "hour",
            "d" | "day" => "day",
            "w" | "week" => "week",
            "mo" | "month" => "month",
            _ => return Err(nope()),
        };
        let at = match u {
            "minute" => now + n as u64 * 60_000,
            "hour" => now + n as u64 * 3_600_000,
            _ => {
                let c = local_parts(now, tz).ok_or_else(nope)?;
                let moved = match u {
                    "day" => add_days(&c, n),
                    "week" => add_days(&c, 7 * n),
                    _ => add_months(&c, n, None),
                };
                from_local(&moved, tz).ok_or_else(nope)?
            }
        };
        return Ok(When { at, repeat: None, day: None });
    }

    // on the 1st of each month [at 9am] / monthly on the 15th
    let flat = t.replace(',', " ");
    let (body, clock) = split_at_clock(flat.trim());
    let clock = clock.filter(|c| !c.is_empty());
    let bw: Vec<&str> = body.split_whitespace().collect();
    let day_of_month = match bw.as_slice() {
        ["on", "the", d, "of", "each" | "every", "month"] | ["on", "the", d, "of", "the", "month"] | ["monthly", "on", "the", d] | ["monthly", "on", d] | ["every", "month", "on", "the", d] | ["on", d, "of", "each" | "every", "month"] => ordinal(d),
        [d, "of", "each" | "every", "month"] | ["the", d, "of", "each" | "every", "month"] => ordinal(d),
        _ => None,
    };
    if let Some(day) = day_of_month {
        let clk = match clock {
            Some(c) => parse_clock(c).ok_or_else(nope)?,
            None => DEFAULT_CLOCK,
        };
        let at = first_of_month_day(now, tz, clk, day).ok_or_else(nope)?;
        return Ok(When { at, repeat: Some("monthly".into()), day: Some(day) });
    }

    // every weekday|day|week|month|hour|<weekday>|N units [at 9]; daily|weekly|monthly|hourly [at 9]
    let rep = match bw.as_slice() {
        ["every", "weekday" | "weekdays"] | ["weekdays"] | ["on", "weekdays"] | ["every", "work", "day"] => Some(("weekdays".to_string(), None)),
        ["every", "day"] | ["daily"] | ["everyday"] | ["every", "morning"] => Some(("daily".to_string(), None)),
        ["every", "week"] | ["weekly"] => Some(("weekly".to_string(), None)),
        ["every", "month"] | ["monthly"] | ["each", "month"] => Some(("monthly".to_string(), None)),
        ["every", "hour"] | ["hourly"] => Some(("hourly".to_string(), None)),
        ["every", w] if weekday_of(w).is_some() => Some(("weekly".to_string(), weekday_of(w))),
        ["weekly", "on", w] | ["every", "week", "on", w] if weekday_of(w).is_some() => Some(("weekly".to_string(), weekday_of(w))),
        ["every", n, unit] if n.parse::<u64>().is_ok() => Some((normalize_repeat(&format!("every {n} {unit}"))?, None)),
        _ => None,
    };
    if let Some((repeat, wd)) = rep {
        // hourly-type repeats have no time of day: the first run is one step from now
        if let Some(step) = interval_ms(&repeat) {
            return Ok(When { at: now + step, repeat: Some(repeat), day: None });
        }
        let clk = match clock {
            Some(c) => parse_clock(c).ok_or_else(nope)?,
            None => DEFAULT_CLOCK,
        };
        // "every week" / "every month" start a week / a month from today, at the time of day
        if wd.is_none() && matches!(repeat.as_str(), "weekly" | "monthly") {
            let today = local_parts(now, tz).ok_or_else(nope)?;
            let base = Civil { h: clk.0, mi: clk.1, s: 0, ..today };
            let (c, day) = if repeat == "weekly" { (add_days(&base, 7), None) } else { (add_months(&base, 1, None), Some(today.d as u32)) };
            return Ok(When { at: from_local(&c, tz).ok_or_else(nope)?, repeat: Some(repeat), day });
        }
        let weekdays = repeat == "weekdays";
        let at = first_day(now, tz, clk, |c| match wd {
            Some(w) => c.wd == w,
            None if weekdays => c.wd < 6,
            None => true,
        })
        .ok_or_else(nope)?;
        return Ok(When { at, repeat: Some(repeat), day: None });
    }

    // anything else: the system's own reading ("tomorrow 9am", "next monday", "2026-11-05 18:30", "now + 2 hours")
    let plain = t.replace(" at ", " ");
    let out = date_cmd(tz).args(["-d", plain.as_str(), "+%s"]).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(nope());
    }
    let secs: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().map_err(|_| "That doesn't look like a time.".to_string())?;
    Ok(When { at: secs * 1000, repeat: None, day: None })
}

/// The first `day`th of a month at `clock` after `now`.
fn first_of_month_day(now: u64, tz: Option<&str>, (h, m): (i64, i64), day: u32) -> Option<u64> {
    let today = local_parts(now, tz)?;
    for k in 0..26 {
        let c = add_months(&Civil { h, mi: m, s: 0, d: 1, ..today }, k, None);
        let c = Civil { d: i64::from(day).min(days_in_month(c.y, c.mo)), ..c };
        let t = from_local(&c, tz)?;
        if t > now {
            return Some(t);
        }
    }
    None
}

/// Turns "tomorrow 9am", "in 2 hours", "2026-11-05 18:30" or "next monday" into a time, using the system's `date`.
pub fn parse_when(text: &str) -> Result<u64, String> {
    parse_schedule(text, None, now_millis()).map(|w| w.at)
}

/// "Wed 04 Nov, 09:00" (in the system's time zone).
pub fn format_when(ms: u64) -> String {
    format_when_in(ms, None)
}

pub fn format_when_in(ms: u64, tz: Option<&str>) -> String {
    date_cmd(tz).args(["-d", &format!("@{}", ms / 1000), "+%a %d %b, %H:%M"]).output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "?".into())
}

// ── the list ──

/// What is needed to make a task.
#[derive(Clone, Debug, Default)]
pub struct New {
    pub title: String,
    pub prompt: String,
    pub next_run: u64,
    pub repeat: String,
    pub project: Option<String>,
    pub kind: String,
    pub mode: Option<String>,
    pub run_on: String,
    pub tz: Option<String>,
    pub day: Option<u32>,
}

/// The longest prompt and title.
pub const MAX_PROMPT_CHARS: usize = 8000;
pub const MAX_TITLE_CHARS: usize = 120;

/// Checks and tidies the fields shared by a new task and an edit.
fn check_fields(kind: &str, mode: &Option<String>, run_on: &str, tz: &Option<String>, day: Option<u32>) -> Result<(String, Option<String>, String, Option<String>, Option<u32>), String> {
    let kind = if kind.trim().is_empty() { "task" } else { kind.trim() };
    if !KINDS.contains(&kind) {
        return Err("The kind must be task, reminder or browser.".into());
    }
    let mode = match mode.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        None => None,
        Some(m) => match crate::policy::Mode::parse_known(m) {
            Some(crate::policy::Mode::Auto) => Some("auto".to_string()),
            Some(crate::policy::Mode::Manual) => Some("manual".to_string()),
            _ => return Err("A task runs in Auto or Manual mode; Bypass is not available for scheduled tasks.".into()),
        },
    };
    let run_on = match run_on.trim() {
        "" | "this" | "computer" => String::new(),
        "cloud" => "cloud".to_string(),
        m if m.starts_with("machine:") && m.len() > 8 && m.len() <= 80 && !m.chars().any(char::is_control) => m.to_string(),
        _ => return Err("Run on must be this computer, the cloud server or machine:<name>.".into()),
    };
    if let Some(t) = tz.as_deref().filter(|t| !t.trim().is_empty()) {
        if clean_tz(Some(t)).is_none() {
            return Err("That time zone name is not valid.".into());
        }
    }
    if let Some(d) = day {
        if !(1..=31).contains(&d) {
            return Err("The day of the month must be from 1 to 31.".into());
        }
    }
    Ok((kind.to_string(), mode, run_on, clean_tz(tz.as_deref()), day))
}

pub fn add_new(n: New) -> Result<Task, String> {
    if n.prompt.trim().is_empty() {
        return Err(if n.kind == "reminder" { "Say what to remind you of.".into() } else { "Say what Zohara Agent should do.".into() });
    }
    if n.prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(format!("That is too long (at most {MAX_PROMPT_CHARS} characters)."));
    }
    let repeat = normalize_repeat(&n.repeat)?;
    let (kind, mode, run_on, tz, day) = check_fields(&n.kind, &n.mode, &n.run_on, &n.tz, n.day)?;
    let now = now_millis();
    let title: String = if n.title.trim().is_empty() { n.prompt.trim().chars().take(48).collect() } else { n.title.trim().chars().take(MAX_TITLE_CHARS).collect() };
    change(|all| {
        let task = Task {
            id: format!("t{now}{}", all.len()),
            title,
            prompt: n.prompt.trim().to_string(),
            next_run: n.next_run,
            repeat,
            enabled: true,
            project: n.project,
            created: now,
            kind,
            mode,
            run_on,
            tz,
            day,
            ..Default::default()
        };
        all.push(task.clone());
        Ok(task)
    })
}

pub fn add(title: &str, prompt: &str, next_run: u64, repeat: &str, project: Option<String>) -> Result<Task, String> {
    add_new(New { title: title.into(), prompt: prompt.into(), next_run, repeat: repeat.into(), project, ..Default::default() })
}

/// What an edit may change (None leaves a field as it is).
#[derive(Clone, Debug, Default)]
pub struct Edit {
    pub title: Option<String>,
    pub prompt: Option<String>,
    pub next_run: Option<u64>,
    pub repeat: Option<String>,
    pub kind: Option<String>,
    /// Some("") clears the task's own mode (follow the person's mode).
    pub mode: Option<String>,
    pub run_on: Option<String>,
    pub tz: Option<String>,
    pub day: Option<Option<u32>>,
}

pub fn update(id: &str, e: Edit) -> Result<Task, String> {
    let cur = get(id).ok_or("That task is gone.")?;
    let repeat = match &e.repeat {
        Some(r) => normalize_repeat(r)?,
        None => cur.repeat.clone(),
    };
    let mode_in = match &e.mode {
        Some(m) => Some(m.clone()),
        None => cur.mode.clone(),
    };
    let (kind, mode, run_on, tz, day) = check_fields(
        e.kind.as_deref().unwrap_or(cur.kind()),
        &mode_in,
        e.run_on.as_deref().unwrap_or(&cur.run_on),
        &e.tz.clone().or(cur.tz.clone()),
        e.day.unwrap_or(cur.day),
    )?;
    if let Some(p) = &e.prompt {
        if p.trim().is_empty() {
            return Err("Say what Zohara Agent should do.".into());
        }
        if p.chars().count() > MAX_PROMPT_CHARS {
            return Err(format!("That is too long (at most {MAX_PROMPT_CHARS} characters)."));
        }
    }
    change(|all| {
        let t = all.iter_mut().find(|t| t.id == id).ok_or("That task is gone.")?;
        if let Some(x) = &e.title {
            if !x.trim().is_empty() {
                t.title = x.trim().chars().take(MAX_TITLE_CHARS).collect();
            }
        }
        if let Some(x) = &e.prompt {
            t.prompt = x.trim().to_string();
        }
        if let Some(x) = e.next_run {
            t.next_run = x;
            // a one-off that is moved to a later time is on again
            if repeat == "once" && x > now_millis() {
                t.enabled = true;
            }
        }
        t.repeat = repeat;
        t.kind = kind;
        t.mode = mode;
        t.run_on = run_on;
        t.tz = tz;
        t.day = day;
        Ok(t.clone())
    })
}

pub fn remove(id: &str) -> bool {
    change(|all| {
        let before = all.len();
        all.retain(|t| t.id != id);
        all.len() != before
    })
}

/// Makes a task due right now (the "Run now" button on a host that runs it on its next look).
pub fn run_now(id: &str) {
    change(|all| {
        if let Some(t) = all.iter_mut().find(|t| t.id == id) {
            t.enabled = true;
            t.next_run = now_millis();
        }
    })
}

/// Asks the host that owns the task to run it at its next look, without changing its schedule (the cloud runner does that; see `run_requested`).
pub fn request_run(id: &str) -> bool {
    change(|all| match all.iter_mut().find(|t| t.id == id) {
        Some(t) => {
            t.run_requested = true;
            true
        }
        None => false,
    })
}

pub fn set_enabled(id: &str, on: bool) -> bool {
    change(|all| match all.iter_mut().find(|t| t.id == id) {
        Some(t) => {
            t.enabled = on;
            if on {
                // switched on again after its time went by: the next time that is still to come (a one-off runs soon)
                if t.next_run < now_millis() {
                    match next_after(t.next_run, &t.repeat, t.tz.as_deref(), t.day, now_millis()) {
                        Some(n) => t.next_run = n,
                        None => t.next_run = now_millis() + 60_000,
                    }
                }
            }
            true
        }
        None => false,
    })
}

/// Enabled tasks whose time has come (or whose "Run now" was asked for).
pub fn due(now: u64) -> Vec<Task> {
    load().into_iter().filter(|t| (t.enabled && t.next_run <= now) || (t.run_requested && t.enabled)).collect()
}

/// A task is late (the computer was off or asleep at its time) when its run starts this long after the planned time.
pub const MISSED_AFTER_MS: u64 = 5 * 60_000;

/// The planned time, when `t` is running late enough to count as missed.
pub fn missed_time(t: &Task, now: u64) -> Option<u64> {
    (!t.run_requested && now.saturating_sub(t.next_run) > MISSED_AFTER_MS).then_some(t.next_run)
}

/// What a finished run leaves in the task.
pub struct Finished {
    pub status: String,
    pub summary: String,
    pub chat: Option<String>,
    pub ms: u64,
    pub missed: Option<u64>,
    pub how: String,
}

fn short(s: &str, max: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > max {
        format!("{}…", one.chars().take(max - 1).collect::<String>().trim_end())
    } else {
        one
    }
}

/// Record that a task ran: the result and one line of history; a repeating one moves to its next time (never into the past), a
/// one-off switches itself off. A "Run now" of a task whose time had not come leaves its schedule alone.
pub fn finish(id: &str, now: u64, f: Finished) {
    change(|all| {
        let Some(t) = all.iter_mut().find(|t| t.id == id) else { return };
        let manual = t.run_requested || f.how == "manual";
        t.run_requested = false;
        t.last_run = Some(now);
        t.last_chat = f.chat.clone();
        t.last_status = Some(f.status.clone());
        let summary = short(&f.summary, 400);
        t.last_output = Some(summary.clone());
        t.history.insert(0, Run { time: now, status: f.status, summary, chat: f.chat, ms: f.ms, missed: f.missed, how: f.how });
        t.history.truncate(HISTORY_MAX);
        // a manual run before the planned time does not use up that time
        if manual && t.next_run > now {
            return;
        }
        match next_after(t.next_run, &t.repeat, t.tz.as_deref(), t.day, now) {
            Some(next) => t.next_run = next,
            None => t.enabled = false,
        }
    })
}

/// Record that a task ran (no history details): kept for callers that only know the chat.
pub fn mark_ran(id: &str, now: u64, chat: Option<String>) {
    finish(id, now, Finished { status: "ok".into(), summary: String::new(), chat, ms: 0, missed: None, how: "scheduled".into() })
}

/// The task as the clients get it.
pub fn to_json(t: &Task) -> serde_json::Value {
    serde_json::json!({
        "id": t.id, "title": t.title, "prompt": t.prompt, "next_run": t.next_run, "repeat": t.repeat, "enabled": t.enabled,
        "project": t.project, "created": t.created, "last_run": t.last_run, "last_chat": t.last_chat,
        "kind": t.kind(), "mode": t.mode, "run_on": t.run_on(), "tz": t.tz, "day": t.day,
        "last_status": t.last_status, "last_output": t.last_output,
        "history_count": t.history.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_temp_dir(name: &str, f: impl FnOnce()) {
        let _g = super::super::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("zo-{name}-{}", std::process::id()));
        std::env::set_var("XDG_DATA_HOME", &dir);
        f();
        let _ = std::fs::remove_dir_all(&dir);
        std::env::remove_var("XDG_DATA_HOME");
    }

    /// 2026-11-14 (a Saturday) 12:00 UTC.
    const NOW: u64 = 1_794_657_600_000;

    fn utc(ms: u64) -> String {
        let c = local_parts(ms, Some("UTC")).unwrap();
        format!("{:04}-{:02}-{:02} {:02}:{:02} wd{}", c.y, c.mo, c.d, c.h, c.mi, c.wd)
    }

    #[test]
    fn times_are_read_from_plain_words_and_refused_when_nonsense() {
        let now = now_millis();
        let in_2h = parse_when("now + 2 hours").unwrap();
        assert!(in_2h > now + 7_100_000 && in_2h < now + 7_300_000);
        assert_eq!(parse_when("@1700000000").unwrap(), 1_700_000_000_000);
        assert!(parse_when("whenever it feels right").is_err());
        assert!(parse_when("").is_err());
        assert!(parse_when("-f /etc/passwd").is_err());
    }

    #[test]
    fn in_n_hours_and_minutes_work() {
        assert_eq!(utc(NOW), "2026-11-14 12:00 wd6");
        let w = parse_schedule("in 2 hours", Some("UTC"), NOW).unwrap();
        assert_eq!((w.at, w.repeat), (NOW + 7_200_000, None));
        assert_eq!(parse_schedule("in 30 minutes", None, NOW).unwrap().at, NOW + 1_800_000);
        assert_eq!(parse_schedule("In 1 hour", None, NOW).unwrap().at, NOW + 3_600_000);
        assert_eq!(parse_schedule("in an hour", None, NOW).unwrap().at, NOW + 3_600_000);
        assert_eq!(parse_schedule("5 minutes from now", None, NOW).unwrap().at, NOW + 300_000);
        assert_eq!(utc(parse_schedule("in 3 days", Some("UTC"), NOW).unwrap().at), "2026-11-17 12:00 wd2");
        assert_eq!(utc(parse_schedule("in 2 weeks", Some("UTC"), NOW).unwrap().at), "2026-11-28 12:00 wd6");
        assert_eq!(utc(parse_schedule("in 1 month", Some("UTC"), NOW).unwrap().at), "2026-12-14 12:00 wd1");
        assert!(parse_schedule("in 0 hours", None, NOW).is_err());
        assert!(parse_schedule("in lots hours", None, NOW).is_err());
        assert!(parse_schedule("in 3 fortnights", None, NOW).is_err());
    }

    #[test]
    fn repeats_said_in_words_are_understood() {
        let w = parse_schedule("every weekday at 9", Some("UTC"), NOW).unwrap();
        assert_eq!(w.repeat.as_deref(), Some("weekdays"));
        assert_eq!(utc(w.at), "2026-11-16 09:00 wd1", "Saturday noon: the next weekday 9:00 is Monday");
        let w = parse_schedule("every day at 07:30", Some("UTC"), NOW).unwrap();
        assert_eq!((w.repeat.as_deref(), utc(w.at)), (Some("daily"), "2026-11-15 07:30 wd7".to_string()));
        let w = parse_schedule("every monday at 9am", Some("UTC"), NOW).unwrap();
        assert_eq!((w.repeat.as_deref(), utc(w.at)), (Some("weekly"), "2026-11-16 09:00 wd1".to_string()));
        let w = parse_schedule("every Friday at 5:30pm", Some("UTC"), NOW).unwrap();
        assert_eq!((w.repeat.as_deref(), utc(w.at)), (Some("weekly"), "2026-11-20 17:30 wd5".to_string()));
        let w = parse_schedule("daily at 8", Some("UTC"), NOW).unwrap();
        assert_eq!((w.repeat.as_deref(), utc(w.at)), (Some("daily"), "2026-11-15 08:00 wd7".to_string()));
        // no time of day: 9:00
        assert_eq!(utc(parse_schedule("every weekday", Some("UTC"), NOW).unwrap().at), "2026-11-16 09:00 wd1");
        // today still to come
        assert_eq!(utc(parse_schedule("every day at 6pm", Some("UTC"), NOW).unwrap().at), "2026-11-14 18:00 wd6");
        let w = parse_schedule("every 3 days", Some("UTC"), NOW).unwrap();
        assert_eq!((w.repeat.as_deref(), w.at), (Some("every 3 days"), NOW + 21 * 3_600_000));
        assert!(parse_schedule("every 2 minutes", None, NOW).is_err(), "too often");
        let w = parse_schedule("every 2 hours", None, NOW).unwrap();
        assert_eq!((w.repeat.as_deref(), w.at), (Some("every 2 hours"), NOW + 7_200_000));
        assert!(parse_schedule("every weekday at 25", Some("UTC"), NOW).is_err());
    }

    #[test]
    fn on_the_nth_of_each_month() {
        let w = parse_schedule("on the 1st of each month", Some("UTC"), NOW).unwrap();
        assert_eq!((w.repeat.as_deref(), w.day, utc(w.at)), (Some("monthly"), Some(1), "2026-12-01 09:00 wd2".to_string()));
        let w = parse_schedule("on the 15th of every month at 6pm", Some("UTC"), NOW).unwrap();
        assert_eq!((w.day, utc(w.at)), (Some(15), "2026-11-15 18:00 wd7".to_string()));
        let w = parse_schedule("monthly on the 31st", Some("UTC"), NOW).unwrap();
        assert_eq!(utc(w.at), "2026-11-30 09:00 wd1", "November has 30 days: the last day");
    }

    #[test]
    fn time_zones_move_the_clock_and_bad_names_are_ignored() {
        // 09:00 in Karachi (UTC+5) is 04:00 UTC
        let w = parse_schedule("every day at 9", Some("Asia/Karachi"), NOW).unwrap();
        assert_eq!(utc(w.at), "2026-11-15 04:00 wd7");
        assert!(clean_tz(Some("../etc/passwd")).is_none() && clean_tz(Some("/etc/x")).is_none() && clean_tz(Some("A B")).is_none());
        assert_eq!(clean_tz(Some("Europe/Berlin")).as_deref(), Some("Europe/Berlin"));
    }

    #[test]
    fn calendar_repeats_move_on_in_the_calendar() {
        let tz = Some("UTC");
        // weekdays: Friday 09:00 -> Monday 09:00
        let fri = parse_schedule("2026-11-13 09:00", tz, NOW).unwrap().at;
        assert_eq!(utc(next_after(fri, "weekdays", tz, None, fri).unwrap()), "2026-11-16 09:00 wd1");
        assert_eq!(utc(next_after(fri, "daily", tz, None, fri).unwrap()), "2026-11-14 09:00 wd6");
        assert_eq!(utc(next_after(fri, "weekly", tz, None, fri).unwrap()), "2026-11-20 09:00 wd5");
        assert_eq!(utc(next_after(fri, "every 2 weeks", tz, None, fri).unwrap()), "2026-11-27 09:00 wd5");
        // a long absence: no backlog, the next one still to come, on the same weekday and hour
        let after = NOW + 400 * 86_400_000;
        let n = next_after(fri, "weekly", tz, None, after).unwrap();
        assert!(n > after && n <= after + 7 * 86_400_000 && utc(n).ends_with("09:00 wd5"), "{}", utc(n));
        let n = next_after(fri, "weekdays", tz, None, after).unwrap();
        assert!(n > after && n <= after + 3 * 86_400_000 && utc(n).contains("09:00 wd"), "{}", utc(n));
        let n = next_after(fri, "daily", tz, None, after).unwrap();
        assert!(n > after && n <= after + 86_400_000 && utc(n).contains("09:00"), "{}", utc(n));
        // months: the 31st stays the 31st after a short month
        let jan31 = parse_schedule("2027-01-31 09:00", tz, NOW).unwrap().at;
        let feb = next_after(jan31, "monthly", tz, Some(31), jan31).unwrap();
        assert_eq!(utc(feb), "2027-02-28 09:00 wd7");
        let mar = next_after(feb, "monthly", tz, Some(31), feb).unwrap();
        assert_eq!(utc(mar), "2027-03-31 09:00 wd3");
        // hours and minutes are fixed lengths
        assert_eq!(next_after(1_000, "every 90 minutes", None, None, 1_000 + 3 * 90 * 60_000), Some(1_000 + 4 * 90 * 60_000));
        assert_eq!(next_after(5, "once", None, None, 5), None);
        // a clock change does not move the hour of a daily task: 09:00 Berlin stays 09:00 across the end of summer time (2026-10-25)
        let t = parse_schedule("2026-10-24 09:00", Some("Europe/Berlin"), NOW).unwrap().at;
        let nx = next_after(t, "daily", Some("Europe/Berlin"), None, t).unwrap();
        assert_eq!(local_parts(nx, Some("Europe/Berlin")).unwrap().h, 9);
        assert_eq!(nx - t, 25 * 3_600_000);
    }

    #[test]
    fn the_next_runs_preview_lists_the_coming_times() {
        let tz = Some("UTC");
        let w = parse_schedule("every weekday at 9", tz, NOW).unwrap();
        let v: Vec<String> = next_runs(w.at, "weekdays", tz, None, 4, NOW).into_iter().map(utc).collect();
        assert_eq!(v, ["2026-11-16 09:00 wd1", "2026-11-17 09:00 wd2", "2026-11-18 09:00 wd3", "2026-11-19 09:00 wd4"]);
        assert_eq!(next_runs(NOW + 1000, "once", tz, None, 3, NOW).len(), 1);
        // a start in the past is first moved on
        let past = NOW - 3 * 86_400_000;
        assert!(next_runs(past, "daily", tz, None, 3, NOW).iter().all(|t| *t > NOW));
    }

    #[test]
    fn a_task_is_added_listed_soonest_first_and_removed() {
        with_temp_dir("sched-a", || {
            add("later", "do b", 2_000, "daily", None).unwrap();
            add("", "do a thing that has a long description", 1_000, "once", Some("p1".into())).unwrap();
            let all = load();
            assert_eq!(all[0].prompt, "do a thing that has a long description");
            assert_eq!(all[0].title, "do a thing that has a long description");
            assert_eq!(all[0].project.as_deref(), Some("p1"));
            assert_eq!((all[0].kind(), all[0].run_on()), ("task", "this"));
            assert!(add("x", "  ", 1, "once", None).is_err());
            assert!(add("x", "y", 1, "yearly", None).is_err());
            assert!(normalize_repeat("every 2 minutes").is_err(), "too often");
            assert_eq!(normalize_repeat("Every 3 days").unwrap(), "every 3 days");
            assert_eq!(normalize_repeat("every 1 hours").unwrap(), "every 1 hour");
            assert_eq!(normalize_repeat("monthly").unwrap(), "monthly");
            assert_eq!(normalize_repeat("every weekday").unwrap(), "weekdays");
            assert_eq!(normalize_repeat("").unwrap(), "once");
            assert!(normalize_repeat("every x days").is_err() && normalize_repeat("every 3 years").is_err());
            // a month later is the same moment next month (checked against the system calendar)
            let jan = 1_800_000_000_000u64; // 2027-01-15
            let feb = next_after(jan, "monthly", None, None, jan).unwrap();
            assert!(feb > jan + 27 * 86_400_000 && feb < jan + 32 * 86_400_000);
            assert!(remove(&all[0].id));
            assert_eq!(load().len(), 1);
        });
    }

    #[test]
    fn old_tasks_without_the_new_fields_still_load() {
        with_temp_dir("sched-old", || {
            let dir = path();
            std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
            std::fs::write(&dir, r#"[{"id":"t1","title":"old","prompt":"p","next_run":5,"repeat":"daily","enabled":true}]"#).unwrap();
            let t = &load()[0];
            assert_eq!((t.kind(), t.run_on(), t.mode.clone(), t.history.len()), ("task", "this", None, 0));
        });
    }

    #[test]
    fn kinds_modes_and_places_are_checked() {
        with_temp_dir("sched-kinds", || {
            let mk = |kind: &str, mode: Option<&str>, run_on: &str| add_new(New { prompt: "x".into(), next_run: 1, repeat: "once".into(), kind: kind.into(), mode: mode.map(String::from), run_on: run_on.into(), ..Default::default() });
            assert_eq!(mk("reminder", None, "").unwrap().kind, "reminder");
            assert_eq!(mk("browser", Some("Manual"), "cloud").unwrap().run_on, "cloud");
            assert_eq!(mk("task", None, "machine:vm-1").unwrap().machine(), Some("vm-1"));
            assert!(mk("shell", None, "").is_err());
            assert!(mk("task", Some("bypass"), "").is_err(), "no task runs in Bypass");
            assert!(mk("task", Some("yolo"), "").is_err());
            assert!(mk("task", None, "elsewhere").is_err());
            assert!(mk("task", None, "machine:").is_err());
            let long = New { prompt: "x".repeat(MAX_PROMPT_CHARS + 1), next_run: 1, repeat: "once".into(), ..Default::default() };
            assert!(add_new(long).is_err());
        });
    }

    #[test]
    fn a_task_can_be_edited_switched_and_asked_to_run() {
        with_temp_dir("sched-edit", || {
            let t = add("a", "do a", now_millis() + 3_600_000, "daily", None).unwrap();
            let e = update(&t.id, Edit { title: Some("b".into()), repeat: Some("every 2 days".into()), kind: Some("reminder".into()), mode: Some("manual".into()), run_on: Some("cloud".into()), ..Default::default() }).unwrap();
            assert_eq!((e.title.as_str(), e.repeat.as_str(), e.kind(), e.mode.as_deref(), e.run_on()), ("b", "every 2 days", "reminder", Some("manual"), "cloud"));
            assert_eq!(e.prompt, "do a", "what was not changed stays");
            assert!(update(&t.id, Edit { mode: Some("bypass".into()), ..Default::default() }).is_err());
            assert!(update(&t.id, Edit { prompt: Some("  ".into()), ..Default::default() }).is_err());
            assert!(update("nope", Edit::default()).is_err());
            let e = update(&t.id, Edit { mode: Some(String::new()), ..Default::default() }).unwrap();
            assert_eq!(e.mode, None, "an empty mode follows the person's mode again");
            assert!(set_enabled(&t.id, false) && !get(&t.id).unwrap().enabled);
            assert!(!set_enabled("nope", true));
            assert!(request_run(&t.id) && get(&t.id).unwrap().run_requested);
            // a switched-off task is not due even when asked; switched on it is
            assert!(due(now_millis()).is_empty());
            set_enabled(&t.id, true);
            assert_eq!(due(now_millis()).len(), 1);
        });
    }

    #[test]
    fn due_tasks_run_once_or_move_to_their_next_time() {
        with_temp_dir("sched-b", || {
            let once = add("once", "a", 1_000, "once", None).unwrap();
            let daily = add("daily", "b", 1_000, "daily", None).unwrap();
            let future = add("future", "c", 10_000_000_000_000, "once", None).unwrap();
            let now = 5 * 86_400_000 + 500;
            assert_eq!(due(now).len(), 2);
            assert_eq!(missed_time(&daily, now), Some(1_000), "days late: a catch-up");
            assert_eq!(missed_time(&daily, 1_000 + 60_000), None, "a minute late is not missed");
            mark_ran(&once.id, now, Some("c1".into()));
            mark_ran(&daily.id, now, None);
            let all = load();
            let o = all.iter().find(|t| t.id == once.id).unwrap();
            assert!(!o.enabled && o.last_run == Some(now) && o.last_chat.as_deref() == Some("c1"));
            let d = all.iter().find(|t| t.id == daily.id).unwrap();
            assert!(d.enabled && d.next_run > now && d.next_run <= now + 86_400_000, "next daily run is in the future, not a backlog");
            assert!(due(now).is_empty());
            assert!(all.iter().any(|t| t.id == future.id && t.enabled));
            set_enabled(&once.id, true);
            assert!(load().iter().find(|t| t.id == once.id).unwrap().enabled);
        });
    }

    #[test]
    fn finishing_a_run_keeps_a_history_of_twenty_and_a_manual_run_keeps_the_schedule() {
        with_temp_dir("sched-hist", || {
            let t = add("h", "do", now_millis() + 10 * 3_600_000, "daily", None).unwrap();
            let planned = t.next_run;
            let fin = |n: u64, how: &str| Finished { status: if n % 2 == 0 { "ok" } else { "failed" }.into(), summary: format!("run {n}"), chat: Some(format!("c{n}")), ms: n, missed: None, how: how.into() };
            finish(&t.id, now_millis(), fin(0, "manual"));
            let g = get(&t.id).unwrap();
            assert_eq!(g.next_run, planned, "Run now before the planned time does not use it up");
            assert!(!g.run_requested && g.history.len() == 1 && g.last_status.as_deref() == Some("ok"));
            for n in 1..=25 {
                finish(&t.id, now_millis(), fin(n, "scheduled"));
            }
            let g = get(&t.id).unwrap();
            assert_eq!(g.history.len(), HISTORY_MAX);
            assert_eq!(g.history[0].summary, "run 25", "newest first");
            assert_eq!(g.last_status.as_deref(), Some("failed"));
            assert_eq!(g.last_output.as_deref(), Some("run 25"));
            assert_eq!(g.last_chat.as_deref(), Some("c25"));
            assert!(to_json(&g)["history_count"] == 20 && to_json(&g)["kind"] == "task");
        });
    }
}
