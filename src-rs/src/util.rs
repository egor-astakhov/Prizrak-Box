//! Presentation helpers — ports of util/format.ts, util/profileView.ts,
//! util/flag.ts, util/delay.ts, util/subscriptionAlerts.ts, util/version.ts.

use chrono::{Datelike, Local, NaiveDate, TimeZone, Timelike};
use serde_json::Value;

use crate::i18n::{tr, tr_args};

const UNITS: [&str; 9] = ["B", "KB", "MB", "GB", "TB", "PB", "EB", "ZB", "YB"];

/// `prettyBytes` (3 significant digits, binary steps, decimal display).
pub fn pretty_bytes(n: f64) -> String {
    if n < 0.0 {
        return "0 B".into();
    }
    if n < 1024.0 {
        return format!("{} B", n as i64);
    }
    let exp = ((n.log2() / 10.0).floor() as usize).min(UNITS.len() - 1);
    let v = n / 1024f64.powi(exp as i32);
    format!("{} {}", to_precision3(v), UNITS[exp])
}

/// JS `Number(x.toPrecision(3))` then String().
pub fn to_precision3(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let digits = v.abs().log10().floor() as i32 + 1;
    let decimals = (3 - digits).max(0) as usize;
    let s = format!("{:.*}", decimals, v);
    let r: f64 = s.parse().unwrap_or(v);
    let mut out = format!("{}", r);
    if out.contains('.') {
        out = out.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    out
}

pub fn has_value(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.is_empty(),
        _ => true,
    }
}

pub fn str_of(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

pub fn bool_of(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

pub fn num_of(v: &Value, key: &str) -> f64 {
    match v.get(key) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

pub fn format_traffic(v: Option<&Value>) -> String {
    if !has_value(v) {
        return String::new();
    }
    match v.unwrap() {
        Value::Number(n) => pretty_bytes(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => s.parse::<f64>().map(pretty_bytes).unwrap_or_else(|_| s.clone()),
        other => other.to_string(),
    }
}

pub fn parse_date(v: Option<&Value>) -> Option<chrono::DateTime<Local>> {
    match v? {
        Value::Number(n) => {
            let f = n.as_f64()?;
            let ms = if f > 1e12 { f } else { f * 1000.0 };
            Local.timestamp_millis_opt(ms as i64).single()
        }
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                return None;
            }
            let re1 = regex::Regex::new(r"^(\d{2})[-/.](\d{2})[-/.](\d{4})$").unwrap();
            if let Some(m) = re1.captures(t) {
                let d = NaiveDate::from_ymd_opt(m[3].parse().ok()?, m[2].parse().ok()?, m[1].parse().ok()?)?;
                return Local.from_local_datetime(&d.and_hms_opt(0, 0, 0)?).single();
            }
            let re2 = regex::Regex::new(r"^(\d{4})[-/.](\d{2})[-/.](\d{2})$").unwrap();
            if let Some(m) = re2.captures(t) {
                let d = NaiveDate::from_ymd_opt(m[1].parse().ok()?, m[2].parse().ok()?, m[3].parse().ok()?)?;
                return Local.from_local_datetime(&d.and_hms_opt(0, 0, 0)?).single();
            }
            if let Ok(d) = chrono::DateTime::parse_from_rfc3339(t) {
                return Some(d.with_timezone(&Local));
            }
            let t2 = t.replace(' ', "T");
            if let Ok(d) = chrono::DateTime::parse_from_rfc3339(&t2) {
                return Some(d.with_timezone(&Local));
            }
            for f in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M", "%Y-%m-%dT%H:%M:%S%.f"] {
                if let Ok(n) = chrono::NaiveDateTime::parse_from_str(&t2, f) {
                    return Local.from_local_datetime(&n).single();
                }
            }
            if let Ok(n) = t.parse::<f64>() {
                let ms = if n > 1e12 { n } else { n * 1000.0 };
                return Local.timestamp_millis_opt(ms as i64).single();
            }
            None
        }
        _ => None,
    }
}

pub fn format_date_value(v: Option<&Value>) -> String {
    if !has_value(v) {
        return String::new();
    }
    match parse_date(v) {
        Some(d) => format!("{:02}.{:02}.{}", d.day(), d.month(), d.year()),
        None => v.map(display).unwrap_or_default(),
    }
}

pub fn format_exact(v: Option<&Value>) -> String {
    match parse_date(v) {
        Some(d) => {
            let time = if d.hour() != 0 || d.minute() != 0 {
                format!(" {:02}:{:02}", d.hour(), d.minute())
            } else {
                String::new()
            };
            format!("{:02}.{:02}.{}{}", d.day(), d.month(), d.year(), time)
        }
        None => {
            if has_value(v) {
                v.map(display).unwrap_or_default()
            } else {
                String::new()
            }
        }
    }
}

fn display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub fn relative_date(v: Option<&Value>) -> String {
    let Some(d) = parse_date(v) else {
        return if has_value(v) { v.map(display).unwrap_or_default() } else { String::new() };
    };
    let today = Local::now().date_naive();
    let days = (today - d.date_naive()).num_days();
    if days < 0 {
        return format_date_value(v);
    }
    match days {
        0 => tr("time.today"),
        1 => tr("time.yesterday"),
        n if n < 7 => tr_args("time.days-ago", &[("n", n.to_string())]),
        n if n < 30 => tr_args("time.weeks-ago", &[("n", (n / 7).to_string())]),
        n => tr_args("time.months-ago", &[("n", (n / 30).to_string())]),
    }
}

pub fn relative_seconds(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 10.0 {
        return tr("time.just-now");
    }
    if seconds < 60.0 {
        return tr_args("time.sec-ago", &[("n", (seconds as i64).to_string())]);
    }
    if seconds < 3600.0 {
        return tr_args("time.min-ago", &[("n", ((seconds / 60.0) as i64).to_string())]);
    }
    if seconds < 86400.0 {
        return tr_args("time.hour-ago", &[("n", ((seconds / 3600.0) as i64).to_string())]);
    }
    tr_args("time.days-ago", &[("n", ((seconds / 86400.0) as i64).to_string())])
}

fn has_flag(s: &str) -> bool {
    let mut prev = false;
    for c in s.chars() {
        let is = ('\u{1F1E6}'..='\u{1F1FF}').contains(&c);
        if is && prev {
            return true;
        }
        prev = is;
    }
    false
}

fn contains_flag_emoji(s: &str) -> bool {
    has_flag(s) || s.contains('\u{1F3F3}') || s.contains('\u{1F3F4}') || s.contains('\u{1F6A9}')
}

/// `profileDisplayTitle`.
pub fn profile_title(p: &Value) -> String {
    let title = str_of(p, "title").trim().to_string();
    let header = str_of(p, "headerTitle").trim().to_string();
    if !title.is_empty() && (header.is_empty() || contains_flag_emoji(&title) || !contains_flag_emoji(&header)) {
        return title;
    }
    if !header.is_empty() {
        header
    } else {
        title
    }
}

const CODES: &str = "AD AE AF AG AI AL AM AO AQ AR AS AT AU AW AX AZ BA BB BD BE BF BG BH BI BJ BL BM BN BO BQ BR BS BT BV BW BY BZ \
CA CC CD CF CG CH CI CK CL CM CN CO CR CU CV CW CX CY CZ DE DJ DK DM DO DZ EC EE EG EH ER ES ET FI FJ FK FM FO FR \
GA GB GD GE GF GG GH GI GL GM GN GP GQ GR GS GT GU GW GY HK HM HN HR HT HU ID IE IL IM IN IO IQ IR IS IT JE JM JO \
JP KE KG KH KI KM KN KP KR KW KY KZ LA LB LC LI LK LR LS LT LU LV LY MA MC MD ME MF MG MH MK ML MM MN MO MP MQ MR \
MS MT MU MV MW MX MY MZ NA NC NE NF NG NI NL NO NP NR NU NZ OM PA PE PF PG PH PK PL PM PN PR PS PT PW PY QA RE RO \
RS RU RW SA SB SC SD SE SG SH SI SJ SK SL SM SN SO SR SS ST SV SX SY SZ TC TD TF TG TH TJ TK TL TM TN TO TR TT TV \
TW TZ UA UG UM US UY UZ VA VC VE VG VI VN VU WF WS YE YT ZA ZM ZW UK";

/// `withFlag`: prefixes names starting with an ISO country code with its flag.
pub fn with_flag(name: &str) -> String {
    if name.is_empty() || has_flag(name) {
        return name.to_string();
    }
    let b: Vec<char> = name.chars().collect();
    if b.len() < 2 || !b[0].is_ascii_uppercase() || !b[1].is_ascii_uppercase() {
        return name.to_string();
    }
    if b.len() > 2 {
        let c = b[2];
        if !(c.is_whitespace() || "·-_|:.".contains(c) || c.is_ascii_digit()) {
            return name.to_string();
        }
    }
    let code: String = b[..2].iter().collect();
    if !CODES.split(' ').any(|c| c == code) {
        return name.to_string();
    }
    let cc = if code == "UK" { "GB".to_string() } else { code };
    let flag: String = cc.chars().map(|ch| char::from_u32(0x1F1E6 + (ch as u32 - 'A' as u32)).unwrap_or(ch)).collect();
    format!("{flag} {name}")
}

/// delayTone → colour key.
pub fn delay_tone(delay: i64) -> &'static str {
    if delay == 99999 || delay <= 0 {
        "none"
    } else if delay <= 300 {
        "good"
    } else if delay <= 600 {
        "fair"
    } else {
        "slow"
    }
}

pub fn delay_label(delay: i64) -> String {
    if delay <= 0 || delay == 99999 {
        "—".into()
    } else {
        format!("{delay} ms")
    }
}

pub fn normalize_version(v: &str) -> String {
    let t = v.trim();
    let t = t.strip_prefix('v').or_else(|| t.strip_prefix('V')).unwrap_or(t);
    t.split(['+', '-']).next().unwrap_or("").trim().to_string()
}

pub fn compare_versions(a: &str, b: &str) -> i32 {
    let (l, r) = (normalize_version(a), normalize_version(b));
    if l.is_empty() && r.is_empty() {
        return 0;
    }
    if l.is_empty() {
        return -1;
    }
    if r.is_empty() {
        return 1;
    }
    let lp: Vec<i64> = l.split('.').map(|s| s.parse().unwrap_or(0)).collect();
    let rp: Vec<i64> = r.split('.').map(|s| s.parse().unwrap_or(0)).collect();
    for i in 0..lp.len().max(rp.len()) {
        let (x, y) = (*lp.get(i).unwrap_or(&0), *rp.get(i).unwrap_or(&0));
        if x != y {
            return if x > y { 1 } else { -1 };
        }
    }
    0
}

pub fn is_http_url(u: &str) -> bool {
    let t = u.trim();
    if t.is_empty() {
        return false;
    }
    url::Url::parse(t).map(|p| p.scheme() == "http" || p.scheme() == "https").unwrap_or(false)
}

// ---- subscription alerts ----

const DAY_MS: f64 = 86_400_000.0;

pub fn clock_skew_ms(p: &Value) -> f64 {
    let s = num_of(p, "clockSkewSeconds");
    let at = num_of(p, "clockSkewAtSeconds");
    if s == 0.0 || at == 0.0 {
        return 0.0;
    }
    let age = now_ms() / 1000.0 - at;
    if !(0.0..=30.0 * 86400.0).contains(&age) {
        return 0.0;
    }
    s * 1000.0
}

pub fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

pub fn expire_ms(p: &Value) -> f64 {
    let raw = str_of(p, "expire");
    if raw.is_empty() {
        return 0.0;
    }
    parse_date(Some(&Value::String(raw.replace(' ', "T"))))
        .map(|d| d.timestamp_millis() as f64)
        .unwrap_or(0.0)
}

fn percent_reached(used: f64, total: f64, threshold: f64) -> bool {
    if total <= 0.0 {
        return false;
    }
    let whole = (used / total).floor() * 100.0;
    let rest = ((used % total) * 100.0 / total).floor();
    whole + rest >= threshold
}

fn num_list(p: &Value, key: &str) -> Vec<f64> {
    p.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

pub fn should_show_renew(p: &Value) -> bool {
    if str_of(p, "renewUrl").is_empty() {
        return false;
    }
    let now = now_ms() + clock_skew_ms(p);
    let expire_at = expire_ms(p);
    let mut days = num_list(p, "notifyExpireDays");
    if days.is_empty() {
        days = vec![1.0, 3.0, 7.0];
    }
    let remaining = expire_at - now;
    let expire_soon = expire_at > 0.0 && (remaining <= 0.0 || days.iter().any(|d| remaining <= d * DAY_MS));
    let total = num_of(p, "total");
    let used = num_of(p, "used");
    let tp = num_list(p, "notifyTrafficPercent");
    let traffic_soon = total > 0.0 && !tp.is_empty() && tp.iter().any(|t| percent_reached(used, total, *t));
    expire_soon || traffic_soon
}

/// `toLocaleString(locale, {2-digit date + time})` for the interface language:
/// ru "29.10.2026, 05:55", en "10/29/2026, 05:55 AM", zh "2026/10/29 05:55".
pub fn format_expire_date(ms: f64) -> String {
    let Some(d) = Local.timestamp_millis_opt(ms as i64).single() else { return String::new() };
    format_locale_date_time(d, &crate::i18n::lang())
}

fn format_locale_date_time(d: chrono::DateTime<Local>, lang: &str) -> String {
    match lang {
        "en" => {
            let h12 = match d.hour() % 12 {
                0 => 12,
                h => h,
            };
            let ampm = if d.hour() < 12 { "AM" } else { "PM" };
            format!("{:02}/{:02}/{}, {:02}:{:02} {}", d.month(), d.day(), d.year(), h12, d.minute(), ampm)
        }
        "zh" => format!("{}/{:02}/{:02} {:02}:{:02}", d.year(), d.month(), d.day(), d.hour(), d.minute()),
        _ => format!("{:02}.{:02}.{}, {:02}:{:02}", d.day(), d.month(), d.year(), d.hour(), d.minute()),
    }
}

#[cfg(test)]
mod date_tests {
    use super::*;

    /// Same strings Chromium/WebKit produce for these locales.
    #[test]
    fn expire_date_matches_to_locale_string() {
        let am = Local.with_ymd_and_hms(2026, 10, 29, 5, 55, 0).unwrap();
        let pm = Local.with_ymd_and_hms(2026, 10, 29, 17, 5, 0).unwrap();
        assert_eq!(format_locale_date_time(am, "ru"), "29.10.2026, 05:55");
        assert_eq!(format_locale_date_time(am, "en"), "10/29/2026, 05:55 AM");
        assert_eq!(format_locale_date_time(am, "zh"), "2026/10/29 05:55");
        assert_eq!(format_locale_date_time(pm, "en"), "10/29/2026, 05:05 PM");
        assert_eq!(format_locale_date_time(pm, "zh"), "2026/10/29 17:05");
    }
}

fn format_remaining(ms: f64) -> String {
    let days = (ms / DAY_MS).floor() as i64;
    if days >= 3 {
        return tr_args("subscriptionAlert.remainingDays", &[("days", days.to_string())]);
    }
    let hour = 3_600_000.0;
    let minute = 60_000.0;
    if days >= 1 {
        let hours = ((ms % DAY_MS) / hour).floor() as i64;
        return tr_args("subscriptionAlert.remainingDaysHours", &[("days", days.to_string()), ("hours", hours.to_string())]);
    }
    let hours = (ms / hour).floor() as i64;
    if hours >= 1 {
        let minutes = ((ms % hour) / minute).floor() as i64;
        return tr_args("subscriptionAlert.remainingHoursMinutes", &[("hours", hours.to_string()), ("minutes", minutes.to_string())]);
    }
    tr_args("subscriptionAlert.remainingMinutes", &[("minutes", ((ms / minute).floor() as i64).max(1).to_string())])
}

fn traffic_percent(p: &Value) -> Option<i64> {
    let total = num_of(p, "total");
    if total <= 0.0 {
        return None;
    }
    Some(((num_of(p, "used").max(0.0) * 100.0) / total).floor() as i64)
}

pub fn alert_text(alert: &Value) -> String {
    match str_of(alert, "kind").as_str() {
        "expired" => tr("subscriptionAlert.expired"),
        "expires_in" => tr_args("subscriptionAlert.expiresIn", &[("days", str_of(alert, "days"))]),
        "traffic_used" => tr_args("subscriptionAlert.trafficUsed", &[("percent", str_of(alert, "percent"))]),
        _ => String::new(),
    }
}

/// (message, detail) — `describeAlert`.
pub fn describe_alert(alert: &Value, p: &Value) -> (String, String) {
    if str_of(alert, "kind") == "traffic_used" {
        let pct = traffic_percent(p).map(|x| x.to_string()).unwrap_or_else(|| str_of(alert, "percent"));
        return (tr_args("subscriptionAlert.trafficUsed", &[("percent", pct)]), String::new());
    }
    let expire_at = expire_ms(p);
    if expire_at == 0.0 {
        return (alert_text(alert), String::new());
    }
    let remaining = expire_at - (now_ms() + clock_skew_ms(p));
    if remaining <= 0.0 {
        return (tr("subscriptionAlert.expired"), String::new());
    }
    (
        format_remaining(remaining),
        tr_args("subscriptionAlert.expiresOn", &[("date", format_expire_date(expire_at))]),
    )
}

pub fn notification_title(alert: &Value, p: &Value) -> String {
    if str_of(alert, "kind") == "traffic_used" {
        let pct = traffic_percent(p).map(|x| x.to_string()).unwrap_or_else(|| str_of(alert, "percent"));
        return tr_args("subscriptionAlert.trafficUsed", &[("percent", pct)]);
    }
    let expire_at = expire_ms(p);
    if str_of(alert, "kind") == "expires_in" && expire_at > 0.0 {
        return tr_args("subscriptionAlert.expiresAt", &[("date", format_expire_date(expire_at))]);
    }
    alert_text(alert)
}

pub fn format_date_time(d: chrono::DateTime<Local>) -> String {
    format!(
        "{}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        d.year(),
        d.month(),
        d.day(),
        d.hour(),
        d.minute(),
        d.second(),
        d.timestamp_subsec_millis()
    )
}
