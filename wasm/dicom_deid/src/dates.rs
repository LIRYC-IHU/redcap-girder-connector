//! Date arithmetic shared by every deidentifier.
//!
//! Every date of a patient's files moves by the same number of days, derived
//! from the patient's pseudonym (see [`crate::pseudonym`]): intervals between
//! examinations are exact, while the calendar — era, season, weekday — is gone.
//! Birth dates are not shifted but blanked, since a shifted birth date would
//! give the exact age back by subtraction; the age is written in years instead.

/// Plausible acquisition years; anything outside is not treated as a date.
const MIN_YEAR: i64 = 1850;
const MAX_YEAR: i64 = 2200;

/// A calendar date, as days since 1970-01-01.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Days(pub i64);

/// How a date was written, so the shifted value can be written back the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateFormat {
    /// `YYYYMMDD`, optionally followed by a time (`HHMMSS`, fractions, offset).
    Compact,
    /// `YYYY-MM-DD`, optionally followed by a time.
    Hyphenated,
}

/// A date recognized inside a value, plus whatever followed it.
#[derive(Debug, Clone)]
pub struct ParsedDate<'a> {
    pub days: Days,
    pub format: DateFormat,
    /// Time, timezone, anything else that trailed the date; written back as-is.
    pub remainder: &'a str,
}

/// Recognize a leading date in `value`.
///
/// Accepts `YYYYMMDD…` (DICOM `DA`/`DT`, HL7 v3) and `YYYY-MM-DD…`. Returns
/// `None` when the text does not start with a valid calendar date in a
/// plausible year, which is what keeps plain numbers from being shifted.
pub fn parse(value: &str) -> Option<ParsedDate<'_>> {
    let bytes = value.as_bytes();

    let (year, month, day, rest, format) =
        if bytes.len() >= 8 && bytes[..8].iter().all(u8::is_ascii_digit) {
            (
                value[0..4].parse().ok()?,
                value[4..6].parse().ok()?,
                value[6..8].parse().ok()?,
                &value[8..],
                DateFormat::Compact,
            )
        } else if bytes.len() >= 10
            && bytes[..4].iter().all(u8::is_ascii_digit)
            && bytes[4] == b'-'
            && bytes[5..7].iter().all(u8::is_ascii_digit)
            && bytes[7] == b'-'
            && bytes[8..10].iter().all(u8::is_ascii_digit)
        {
            (
                value[0..4].parse().ok()?,
                value[5..7].parse().ok()?,
                value[8..10].parse().ok()?,
                &value[10..],
                DateFormat::Hyphenated,
            )
        } else {
            return None;
        };

    if !(MIN_YEAR..=MAX_YEAR).contains(&year) || !(1..=12).contains(&month) {
        return None;
    }
    if day < 1 || day > days_in_month(year, month) {
        return None;
    }

    Some(ParsedDate {
        days: Days(days_from_civil(year, month, day)),
        format,
        remainder: rest,
    })
}

/// Rewrite a date, keeping its original notation and anything that trailed it.
pub fn render(days: Days, format: DateFormat, remainder: &str) -> String {
    let (year, month, day) = civil_from_days(days.0);
    match format {
        DateFormat::Compact => format!("{year:04}{month:02}{day:02}{remainder}"),
        DateFormat::Hyphenated => format!("{year:04}-{month:02}-{day:02}{remainder}"),
    }
}

/// Shift one value by `offset`, or return `None` if it is not a date.
pub fn shift(value: &str, offset: i64) -> Option<String> {
    let parsed = parse(value)?;
    Some(render(
        Days(parsed.days.0 + offset),
        parsed.format,
        parsed.remainder,
    ))
}

/// Age between two dates as whole years, months and days, the way people count
/// it: a birthday not yet reached this year does not count. `None` when either
/// is not a date or `at` comes before `birth`.
pub fn age(birth: &str, at: &str) -> Option<Age> {
    let birth = parse(birth)?;
    let at = parse(at)?;
    if at.days.0 < birth.days.0 {
        return None;
    }
    let (by, bm, bd) = civil_from_days(birth.days.0);
    let (ay, am, ad) = civil_from_days(at.days.0);

    let mut months = (ay - by) * 12 + (am - bm);
    if ad < bd {
        months -= 1;
    }
    Some(Age {
        years: months.div_euclid(12),
        months,
        days: at.days.0 - birth.days.0,
    })
}

/// See [`age`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Age {
    pub years: i64,
    /// Whole months in total, not the remainder after the years.
    pub months: i64,
    /// Days in total.
    pub days: i64,
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Howard Hinnant's days-from-civil, with 1970-01-01 as day zero.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146097 + day_of_era - 719468
}

/// Inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let day_of_era = z - era * 146097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };

    (if month <= 2 { year + 1 } else { year }, month, day)
}
