//! Sizes, dates and counts the way Finder writes them, in Russian and English.

/// Local wall-clock time broken down, for the date column.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Civil {
    pub year: i32,
    /// 1..=12
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

/// Days since 1970-01-01 -> (year, month, day) in the proleptic Gregorian calendar.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

/// FILETIME ticks between 1601-01-01 and 1970-01-01.
const EPOCH_DIFF: i64 = 116_444_736_000_000_000;

/// A FILETIME (UTC) shifted by `offset_secs` (local minus UTC).
pub fn civil(filetime: u64, offset_secs: i64) -> Civil {
    let secs = (filetime as i64 - EPOCH_DIFF).div_euclid(10_000_000) + offset_secs;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    Civil { year, month, day, hour: (rem / 3600) as u32, minute: (rem % 3600 / 60) as u32 }
}

fn days_of(c: Civil) -> i64 {
    // Inverse of civil_from_days, for "today" and "yesterday".
    let (y, m) = if c.month <= 2 { (c.year as i64 - 1, c.month as i64 + 9) } else { (c.year as i64, c.month as i64 - 3) };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * m + 2) / 5 + c.day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

const MONTHS_RU: [&str; 12] = ["янв.", "февр.", "мар.", "апр.", "мая", "июн.", "июл.", "авг.", "сент.", "окт.", "нояб.", "дек."];
const MONTHS_EN: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

fn time(c: Civil, ru: bool) -> String {
    if ru {
        format!("{}:{:02}", c.hour, c.minute)
    } else {
        let h = match c.hour % 12 {
            0 => 12,
            h => h,
        };
        format!("{h}:{:02}\u{202F}{}", c.minute, if c.hour < 12 { "AM" } else { "PM" })
    }
}

/// Finder's list-view date: «Сегодня, 13:24», «Вчера, 18:50»,
/// «9 сент. 2025 г., 18:50» / "Today at 1:24 PM", "Sep 9, 2025 at 6:50 PM".
pub fn date(filetime: u64, now: Civil, offset_secs: i64, ru: bool) -> String {
    if filetime == 0 {
        return "--".into();
    }
    let c = civil(filetime, offset_secs);
    let delta = days_of(now) - days_of(c);
    let t = time(c, ru);
    match (delta, ru) {
        (0, true) => format!("Сегодня, {t}"),
        (1, true) => format!("Вчера, {t}"),
        (0, false) => format!("Today at {t}"),
        (1, false) => format!("Yesterday at {t}"),
        (_, true) => format!("{} {} {} г., {t}", c.day, MONTHS_RU[(c.month - 1) as usize], c.year),
        (_, false) => format!("{} {}, {} at {t}", MONTHS_EN[(c.month - 1) as usize], c.day, c.year),
    }
}

/// Decimal units like Finder (1 KB = 1000 bytes): «883 КБ», «1,6 МБ», «2 МБ».
pub fn size(bytes: u64, ru: bool) -> String {
    let (units, byte_word): ([&str; 5], &str) =
        if ru { (["КБ", "МБ", "ГБ", "ТБ", "ПБ"], "байт") } else { (["KB", "MB", "GB", "TB", "PB"], "bytes") };
    if bytes == 0 {
        return if ru { "Ноль байт".into() } else { "Zero bytes".into() };
    }
    if bytes < 1000 {
        return if !ru && bytes == 1 { "1 byte".into() } else { format!("{bytes} {byte_word}") };
    }
    let mut v = bytes as f64 / 1000.0;
    let mut u = 0;
    while v >= 999.95 && u < units.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    // Kilobytes are whole numbers; larger units get three significant digits
    // without trailing zeros (Finder: "1.6 MB", "15.5 MB", "1.22 TB", "2 MB").
    let num = if u == 0 {
        format!("{}", v.round() as u64)
    } else {
        let decimals = if v >= 100.0 { 0 } else if v >= 10.0 { 1 } else { 2 };
        let s = format!("{v:.decimals$}");
        if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
    };
    let num = if ru { num.replace('.', ",") } else { num };
    format!("{num} {}", units[u])
}

/// Russian plural form: 1 объект, 2 объекта, 5 объектов.
pub fn plural_ru(n: usize, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    let (m10, m100) = (n % 10, n % 100);
    if m10 == 1 && m100 != 11 {
        one
    } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
        few
    } else {
        many
    }
}

/// 2054 -> "2,054" / «2 054» (narrow no-break space, as macOS writes Russian).
pub fn group(n: usize, ru: bool) -> String {
    let digits = n.to_string();
    let sep = if ru { "\u{202F}" } else { "," };
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push_str(sep);
        }
        out.push(c);
    }
    out
}

/// Status bar: «12 объектов, 120,5 ГБ доступно», «Выбрано 2 из 12, …».
pub fn status(total: usize, selected: usize, free: Option<u64>, ru: bool) -> String {
    let count = if ru {
        if selected > 0 {
            format!("Выбрано {} из {}", group(selected, ru), group(total, ru))
        } else {
            format!("{} {}", group(total, ru), plural_ru(total, "объект", "объекта", "объектов"))
        }
    } else if selected > 0 {
        format!("{} of {} selected", group(selected, ru), group(total, ru))
    } else if total == 1 {
        "1 item".into()
    } else {
        format!("{} items", group(total, ru))
    };
    match free {
        Some(f) => format!("{count}, {} {}", size(f, ru), if ru { "доступно" } else { "available" }),
        None => count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2025-09-09 18:50 UTC
    const T: u64 = (1_757_443_800i64 * 10_000_000 + EPOCH_DIFF) as u64;

    #[test]
    fn civil_dates() {
        let c = civil(T, 0);
        assert_eq!(c, Civil { year: 2025, month: 9, day: 9, hour: 18, minute: 50 });
        let c = civil(T, 3 * 3600);
        assert_eq!((c.day, c.hour), (9, 21));
        let c = civil(T, 6 * 3600);
        assert_eq!((c.day, c.hour), (10, 0));
        assert_eq!(days_of(Civil { year: 1970, month: 1, day: 1, hour: 0, minute: 0 }), 0);
        assert_eq!(days_of(Civil { year: 2000, month: 3, day: 1, hour: 0, minute: 0 }), 11_017);
    }

    #[test]
    fn finder_dates() {
        let now = Civil { year: 2025, month: 9, day: 9, hour: 20, minute: 0 };
        assert_eq!(date(T, now, 0, true), "Сегодня, 18:50");
        assert_eq!(date(T, now, 0, false), "Today at 6:50\u{202F}PM");
        let tomorrow = Civil { day: 10, ..now };
        assert_eq!(date(T, tomorrow, 0, true), "Вчера, 18:50");
        let later = Civil { year: 2026, ..now };
        assert_eq!(date(T, later, 0, true), "9 сент. 2025 г., 18:50");
        assert_eq!(date(T, later, 0, false), "Sep 9, 2025 at 6:50\u{202F}PM");
        assert_eq!(date(0, now, 0, true), "--");
    }

    #[test]
    fn finder_sizes() {
        assert_eq!(size(0, true), "Ноль байт");
        assert_eq!(size(17, true), "17 байт");
        assert_eq!(size(17_400, true), "17 КБ");
        assert_eq!(size(883_000, false), "883 KB");
        assert_eq!(size(1_600_000, true), "1,6 МБ");
        assert_eq!(size(2_000_000, false), "2 MB");
        assert_eq!(size(15_500_000, false), "15.5 MB");
        assert_eq!(size(999_999, false), "1 MB");
        assert_eq!(size(1_220_000_000_000, true), "1,22 ТБ");
        assert_eq!(size(1_604_000, true), "1,6 МБ");
    }

    #[test]
    fn statuses() {
        assert_eq!(status(1, 0, None, true), "1 объект");
        assert_eq!(status(3, 0, None, true), "3 объекта");
        assert_eq!(status(12, 0, Some(120_600_000_000), true), "12 объектов, 121 ГБ доступно");
        assert_eq!(status(111, 0, None, true), "111 объектов");
        assert_eq!(status(20_000, 0, None, true), "20\u{202F}000 объектов");
        assert_eq!(status(12, 2, None, true), "Выбрано 2 из 12");
        assert_eq!(status(2054, 1, Some(1_220_000_000_000), false), "1 of 2,054 selected, 1.22 TB available");
    }
}
