//! Excel number formats: the text a cell shows, and date serials as ISO
//! dates.
//!
//! A format code has up to four sections (positive, negative, zero, text),
//! optionally chosen by conditions such as `[>=100]`. Each section is
//! either a number pattern (`#,##0.00`, `0%`, `0.00E+00`, `# ?/?`) or a
//! date pattern (`yyyy-mm-dd h:mm`, `[h]:mm`). Numbers are rounded the way
//! Excel shows them: from their 15-significant-digit decimal form, half
//! away from zero.
//!
//! What is not reproduced: column width (`####` for text that does not fit,
//! and `*` fill characters, which are dropped), padding at the edges (`_x`
//! and `?` spaces are trimmed from the ends), colors, and locale: `/` is
//! the date separator as written, and built-in formats whose code depends
//! on the locale (`builtin:<id>`) show dates in ISO form and numbers as
//! General.

// Short names (v for a value, d for a decimal, n for a count) read best here.
#![allow(clippy::many_single_char_names)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use crate::model::Value;
use crate::styles::Styles;

/// A stand-in for a built-in format whose code depends on the locale. The
/// short date (14), short date-time (22), and East Asian date and time
/// formats (27–36 and 50–58) show dates in ISO form, so they still read as
/// dates; Excel shows them in the author's locale (observed for id 31
/// under Korean Excel: `2024년 01월 01일`; US Excel shows 14 as m/d/yyyy;
/// the other ids are from ECMA-376, unverified). Currency and accounting ids
/// (5–8, 41–44) show as General.
fn locale_builtin(id: u32) -> &'static str {
    match id {
        // The short date and date-time, which follow the system's short
        // date (US Excel shows m/d/yyyy).
        22 => "yyyy-mm-dd h:mm",
        32 => "hh:mm",
        33 => "hh:mm:ss",
        14 | 27..=36 | 50..=58 => "yyyy-mm-dd",
        _ => "General",
    }
}

/// What a cell shows when its format cannot show the number.
const NO_TEXT: &str = "########";

/// The largest serial Excel shows as a date: 9999-12-31.
const MAX_DATE: i64 = 2_958_465;
const MAX_DATE_1904: i64 = 2_957_003;

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Lit(String),
    General,
    /// A digit placeholder: `0`, `#`, or `?`.
    Digit(u8),
    Point,
    Comma,
    Percent,
    /// `E+` (true) or `E-`.
    Exp(bool),
    Slash,
    /// A fixed fraction denominator, as in `?/8`.
    Denom(u32),
    At,
    Year(usize),
    Month(usize),
    Minute(usize),
    /// An `m` run not yet known to be a month or a minute.
    MonthOrMinute(usize),
    Day(usize),
    Hour(usize),
    Second(usize),
    /// Fraction-of-second digits after `ss`.
    SubSec(usize),
    /// `AM/PM` or `A/P`, as the two strings to show.
    AmPm(String, String),
    /// `[h]`, `[mm]`, `[ss]`: elapsed time in one unit.
    Elapsed(char, usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Cond {
    op: Op,
    rhs: f64,
}

impl Cond {
    /// Whether only numbers at or below zero satisfy this condition.
    fn for_negatives(self) -> bool {
        matches!(self.op, Op::Lt | Op::Le | Op::Eq) && self.rhs <= 0.0
    }

    fn negate(self) -> Cond {
        let op = match self.op {
            Op::Lt => Op::Ge,
            Op::Le => Op::Gt,
            Op::Gt => Op::Le,
            Op::Ge => Op::Lt,
            Op::Eq => Op::Ne,
            Op::Ne => Op::Eq,
        };
        Cond { op, rhs: self.rhs }
    }

    fn matches(self, v: f64) -> bool {
        match self.op {
            Op::Lt => v < self.rhs,
            Op::Le => v <= self.rhs,
            Op::Gt => v > self.rhs,
            Op::Ge => v >= self.rhs,
            Op::Eq => (v - self.rhs).abs() < f64::EPSILON,
            Op::Ne => (v - self.rhs).abs() >= f64::EPSILON,
        }
    }
}

// Independent facts about one section, set once when it is parsed.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default, PartialEq)]
struct Section {
    toks: Vec<Tok>,
    cond: Option<Cond>,
    /// A date or time pattern.
    date: bool,
    /// Shows a date part (year, month, or day).
    has_date: bool,
    /// Shows a time part.
    has_time: bool,
    elapsed: bool,
    text: bool,
    /// A number section's layout, worked out once when it is parsed.
    layout: Option<Layout>,
    /// A fraction section's slash.
    slash: Option<usize>,
}

/// A parsed number format code.
#[derive(Debug, Clone, PartialEq)]
pub struct Format {
    /// Sections numbers can use, in order.
    numeric: Vec<Section>,
    /// The section text values use, when the code has one.
    text: Option<Section>,
    general: bool,
}

impl Format {
    pub fn parse(code: &str) -> Format {
        if code.trim().is_empty() {
            return Format::parse("General");
        }
        if let Some(id) = code.strip_prefix("builtin:") {
            return Format::parse(locale_builtin(id.parse().unwrap_or(0)));
        }
        let mut sections: Vec<Section> = split_sections(code).iter().map(|s| parse_section(s)).collect();
        // The fourth section, or a section with `@`, formats text.
        let text = if sections.len() >= 4 {
            Some(sections.remove(3))
        } else {
            sections.iter().position(|s| s.text).map(|i| sections.remove(i))
        };
        sections.truncate(3);
        let general = sections.len() <= 1
            && text.is_none()
            && sections.first().is_none_or(|s| s.toks.iter().all(|t| *t == Tok::General) && !s.toks.is_empty());
        Format { numeric: sections, text, general }
    }

    /// Whether this is the General format, which shows a value as it is.
    pub fn is_general(&self) -> bool {
        self.general
    }

    /// Whether a number shows as a date or time (not an elapsed duration).
    pub fn is_date(&self) -> bool {
        self.numeric.first().is_some_and(|s| s.date && !s.elapsed)
    }

    /// Whether this format changes how `v` reads compared with General.
    pub fn shapes(&self, v: &Value) -> bool {
        match v {
            Value::Number(_) => !self.general && !self.numeric.is_empty(),
            Value::Text(_) => self.text.as_ref().is_some_and(|s| s.toks != [Tok::At]),
            _ => false,
        }
    }

    /// The text Excel shows for `v`, trimmed of edge padding.
    pub fn display(&self, v: &Value, date1904: bool) -> String {
        match v {
            Value::Empty => String::new(),
            Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
            Value::Error(e) => e.clone(),
            Value::Text(t) => match &self.text {
                Some(s) => render_text(s, t).trim_matches(' ').to_owned(),
                None => t.to_string(),
            },
            Value::Number(n) => self.number(*n, date1904).trim_matches(' ').to_owned(),
        }
    }

    /// A date-formatted number as ISO 8601: `2024-03-31`, with the time
    /// (`2024-03-31T18:30:00`) when the format shows one or the serial has
    /// one, or the time alone for a time format under one day. `None` for
    /// other formats, for serials outside Excel's dates, and for the 1900
    /// system's 1900-01-00 and 1900-02-29, which no real date matches.
    pub fn iso_date(&self, v: f64, date1904: bool) -> Option<String> {
        let (section, _, _) = self.pick(v)?;
        if !section.date || section.elapsed || v < 0.0 {
            return None;
        }
        let sub = section
            .toks
            .iter()
            .filter_map(|t| if let Tok::SubSec(n) = t { Some(*n) } else { None })
            .max()
            .unwrap_or(0)
            .min(3);
        let t = Instant::new(v, sub, date1904)?;
        let time = format!("{:02}:{:02}:{:02}{}", t.hour, t.minute, t.second, t.sub_text(sub));
        if !section.has_date && v < 1.0 {
            return Some(time);
        }
        // Excel's 1900-01-00 and 1900-02-29 are not dates ISO 8601 can name.
        if !date1904 && matches!(t.serial_day, 0 | 60) {
            return None;
        }
        let date = format!("{:04}-{:02}-{:02}", t.year, t.month, t.day);
        Some(if section.has_time || t.has_time() { format!("{date}T{time}") } else { date })
    }

    /// The section for `v`, the value it formats, and whether a minus sign
    /// goes in front.
    fn pick(&self, v: f64) -> Option<(&Section, f64, bool)> {
        let n = &self.numeric;
        let first = n.first()?;
        if n.iter().any(|s| s.cond.is_some()) {
            // A section shows a negative number's minus unless it can only
            // receive numbers at or below zero: those its own condition
            // admits (`[<0]`, `[=-5]`) or those an earlier section's
            // condition leaves it (`[>=0]0;0`). Observed in Excel.
            // A first section without a condition takes numbers at or
            // above zero (`0;[<0]0` shows 5 as 5); only written conditions
            // count toward the sign (`0.00;[<-10]0;0` shows -5 as -5).
            let c1 = n[0].cond;
            let c2 = n.get(1).and_then(|s| s.cond);
            let negative_only = |conds: &[Option<Cond>]| conds.iter().flatten().any(|c| c.for_negatives());
            if c1.unwrap_or(Cond { op: Op::Ge, rhs: 0.0 }).matches(v) {
                return Some((first, v.abs(), v < 0.0 && !c1.is_some_and(Cond::for_negatives)));
            }
            let not1 = c1.map(Cond::negate);
            if let Some(c) = c2
                && c.matches(v)
            {
                return Some((&n[1], v.abs(), v < 0.0 && !negative_only(&[Some(c), not1])));
            }
            let fallback = match n.len() {
                1 => first,
                2 if c2.is_none() => &n[1],
                2 => return None,
                _ => &n[2],
            };
            let left = [not1, if n.len() > 2 { c2.map(Cond::negate) } else { None }];
            return Some((fallback, v.abs(), v < 0.0 && !negative_only(&left)));
        }
        Some(match n.len() {
            1 => (first, v.abs(), v < 0.0),
            2 if v < 0.0 => (&n[1], -v, false),
            2 => (first, v, false),
            _ if v > 0.0 => (first, v, false),
            _ if v < 0.0 => (&n[1], -v, false),
            _ => (&n[2], 0.0, false),
        })
    }

    fn number(&self, v: f64, date1904: bool) -> String {
        if self.numeric.is_empty() {
            return general(v);
        }
        // No section takes the number (`[>0]0;[<0]0` and zero): Excel fills
        // the cell with `#`, as for a date it cannot show.
        let Some((section, x, minus)) = self.pick(v) else { return NO_TEXT.to_owned() };
        let body = if section.date {
            match render_date(section, if minus { -x } else { x }, date1904) {
                Some(s) => return s,
                None => return NO_TEXT.to_owned(),
            }
        } else {
            render_number(section, x)
        };
        // A negative number that rounds to zero loses its sign; a section
        // that shows no number at all keeps it (`-fixed`), as Excel does.
        let (body, nonzero) = body;
        if minus && nonzero != Some(false) { format!("-{body}") } else { body }
    }
}

/// A workbook's number formats by style index, parsed once each.
pub struct Formats<'a> {
    styles: &'a Styles,
    date1904: bool,
    cache: RefCell<HashMap<u32, Arc<Format>>>,
}

impl<'a> Formats<'a> {
    pub fn new(styles: &'a Styles, date1904: bool) -> Self {
        Formats { styles, date1904, cache: RefCell::new(HashMap::new()) }
    }

    pub fn get(&self, style: u32) -> Arc<Format> {
        let mut cache = self.cache.borrow_mut();
        Arc::clone(
            cache.entry(style).or_insert_with(|| Arc::new(Format::parse(&self.styles.display_number_format(style)))),
        )
    }

    /// The text a cell with `style` shows for `v`, when its format changes
    /// how `v` reads.
    pub fn display(&self, style: u32, v: &Value) -> Option<String> {
        let f = self.get(style);
        if !f.shapes(v) {
            return None;
        }
        let text = f.display(v, self.date1904);
        match v {
            Value::Number(n) if text == general(*n) => None,
            _ => Some(text),
        }
    }

    /// The text a cell shows, always.
    pub fn shown(&self, style: u32, v: &Value) -> String {
        let f = self.get(style);
        f.display(v, self.date1904)
    }

    /// A date-formatted number as ISO text.
    pub fn iso_date(&self, style: u32, v: &Value) -> Option<String> {
        let Value::Number(n) = v else { return None };
        let f = self.get(style);
        f.iso_date(*n, self.date1904)
    }
}

/// Splits a code at `;` outside quotes, brackets, and escapes.
fn split_sections(code: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = code.chars();
    let (mut quoted, mut bracket) = (false, false);
    while let Some(c) = chars.next() {
        let cur = out.last_mut().expect("never empty");
        match c {
            '"' if !bracket => quoted = !quoted,
            '[' if !quoted => bracket = true,
            ']' if !quoted => bracket = false,
            '\\' | '_' | '*' if !quoted && !bracket => {
                cur.push(c);
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
                continue;
            }
            ';' if !quoted && !bracket => {
                out.push(String::new());
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out
}

fn starts_with_ci(chars: &[char], i: usize, word: &str) -> bool {
    let w: Vec<char> = word.chars().collect();
    chars.len() >= i + w.len() && chars[i..i + w.len()].iter().zip(&w).all(|(a, b)| a.eq_ignore_ascii_case(b))
}

fn push_lit(toks: &mut Vec<Tok>, s: &str) {
    if let Some(Tok::Lit(last)) = toks.last_mut() {
        last.push_str(s);
    } else {
        toks.push(Tok::Lit(s.to_owned()));
    }
}

fn run(chars: &[char], i: usize, lower: char) -> usize {
    chars[i..].iter().take_while(|c| c.to_ascii_lowercase() == lower).count()
}

fn parse_section(code: &str) -> Section {
    let chars: Vec<char> = code.chars().collect();
    let mut toks: Vec<Tok> = Vec::new();
    let mut cond = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '"' => {
                let end = chars[i + 1..].iter().position(|&c| c == '"').map_or(chars.len(), |p| i + 1 + p);
                push_lit(&mut toks, &chars[i + 1..end].iter().collect::<String>());
                i = end + 1;
                continue;
            }
            '\\' => {
                if let Some(n) = next {
                    push_lit(&mut toks, &n.to_string());
                }
                i += 2;
                continue;
            }
            '_' => {
                push_lit(&mut toks, " ");
                i += 2;
                continue;
            }
            '*' => {
                i += 2;
                continue;
            }
            '[' => {
                let end = chars[i + 1..].iter().position(|&c| c == ']').map_or(chars.len(), |p| i + 1 + p);
                let inner: String = chars[i + 1..end].iter().collect();
                bracket(&inner, &mut toks, &mut cond);
                i = end + 1;
                continue;
            }
            '0' | '#' | '?' => toks.push(Tok::Digit(c as u8)),
            '.' => {
                let after_seconds = toks
                    .iter()
                    .rev()
                    .find(|t| !matches!(t, Tok::Lit(_)))
                    .is_some_and(|t| matches!(t, Tok::Second(_) | Tok::Elapsed('s', _)));
                let zeros = chars[i + 1..].iter().take_while(|&&c| c == '0').count();
                if after_seconds && zeros > 0 {
                    toks.push(Tok::SubSec(zeros));
                    i += 1 + zeros;
                    continue;
                }
                toks.push(Tok::Point);
            }
            ',' => toks.push(Tok::Comma),
            '%' => toks.push(Tok::Percent),
            'E' | 'e' if matches!(next, Some('+' | '-')) => {
                toks.push(Tok::Exp(next == Some('+')));
                i += 2;
                continue;
            }
            '/' => {
                toks.push(Tok::Slash);
                let digits = chars[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
                if digits > 0 && chars[i + 1] != '0' {
                    let n: String = chars[i + 1..i + 1 + digits].iter().collect();
                    toks.push(Tok::Denom(n.parse().unwrap_or(1)));
                    i += 1 + digits;
                    continue;
                }
            }
            '@' => toks.push(Tok::At),
            'G' | 'g' if starts_with_ci(&chars, i, "general") => {
                toks.push(Tok::General);
                i += 7;
                continue;
            }
            'A' | 'a' if starts_with_ci(&chars, i, "am/pm") => {
                // Shown in the case it is written in.
                let am: String = chars[i..i + 2].iter().collect();
                let pm: String = chars[i + 3..i + 5].iter().collect();
                toks.push(Tok::AmPm(am, pm));
                i += 5;
                continue;
            }
            'A' | 'a' if starts_with_ci(&chars, i, "a/p") => {
                toks.push(Tok::AmPm(c.to_string(), chars[i + 2].to_string()));
                i += 3;
                continue;
            }
            'y' | 'Y' | 'm' | 'M' | 'd' | 'D' | 'h' | 'H' | 's' | 'S' => {
                let lower = c.to_ascii_lowercase();
                let n = run(&chars, i, lower);
                toks.push(match lower {
                    'y' => Tok::Year(n),
                    'm' => Tok::MonthOrMinute(n),
                    'd' => Tok::Day(n),
                    'h' => Tok::Hour(n),
                    _ => Tok::Second(n),
                });
                i += n;
                continue;
            }
            'e' => toks.push(Tok::Year(4)),
            _ => push_lit(&mut toks, &c.to_string()),
        }
        i += 1;
    }
    finish_section(toks, cond)
}

/// A `[...]` part: a currency or locale, elapsed time, a condition, or a
/// color (ignored).
fn bracket(inner: &str, toks: &mut Vec<Tok>, cond: &mut Option<Cond>) {
    if let Some(rest) = inner.strip_prefix('$') {
        let symbol = rest.split('-').next().unwrap_or_default();
        if !symbol.is_empty() {
            push_lit(toks, symbol);
        }
        return;
    }
    let lower = inner.to_ascii_lowercase();
    if let Some(first) = lower.chars().next()
        && matches!(first, 'h' | 'm' | 's')
        && lower.chars().all(|c| c == first)
    {
        toks.push(Tok::Elapsed(first, lower.len()));
        return;
    }
    let ops = [("<=", Op::Le), (">=", Op::Ge), ("<>", Op::Ne), ("<", Op::Lt), (">", Op::Gt), ("=", Op::Eq)];
    for (text, op) in ops {
        if let Some(rest) = inner.strip_prefix(text) {
            if let Ok(rhs) = rest.trim().parse() {
                *cond = Some(Cond { op, rhs });
            }
            return;
        }
    }
}

fn finish_section(mut toks: Vec<Tok>, cond: Option<Cond>) -> Section {
    let is_time = |t: &Tok| matches!(t, Tok::Hour(_) | Tok::Second(_) | Tok::Elapsed(..) | Tok::AmPm(..));
    // `m` is a minute right after an hour or right before a second.
    for k in 0..toks.len() {
        if let Tok::MonthOrMinute(n) = toks[k] {
            let before = toks[..k].iter().rev().find(|t| is_date_tok(t));
            let after = toks[k + 1..].iter().find(|t| is_date_tok(t));
            let minute = before.is_some_and(|t| matches!(t, Tok::Hour(_) | Tok::Elapsed('h', _)))
                || after.is_some_and(|t| matches!(t, Tok::Second(_) | Tok::Elapsed('s', _)));
            toks[k] = if minute { Tok::Minute(n) } else { Tok::Month(n) };
        }
    }
    let date = toks.iter().any(is_date_tok);
    let has_date = toks.iter().any(|t| matches!(t, Tok::Year(_) | Tok::Month(_) | Tok::Day(_)));
    let has_time = toks.iter().any(|t| is_time(t) || matches!(t, Tok::Minute(_) | Tok::SubSec(_)));
    let elapsed = toks.iter().any(|t| matches!(t, Tok::Elapsed(..)));
    let text = toks.contains(&Tok::At);
    if date {
        // Number punctuation in a date pattern is literal.
        toks = toks
            .into_iter()
            .map(|t| match t {
                Tok::Digit(c) => Tok::Lit((c as char).to_string()),
                Tok::Point => Tok::Lit(".".to_owned()),
                Tok::Comma => Tok::Lit(",".to_owned()),
                Tok::Percent => Tok::Lit("%".to_owned()),
                Tok::Slash => Tok::Lit("/".to_owned()),
                Tok::Denom(n) => Tok::Lit(n.to_string()),
                Tok::Exp(plus) => Tok::Lit(if plus { "E+" } else { "E-" }.to_owned()),
                other => other,
            })
            .collect();
    }
    let (layout, slash) = if date { (None, None) } else { (Some(layout(&toks)), fraction_slash(&toks)) };
    Section { toks, cond, date, has_date, has_time, elapsed, text, layout, slash }
}

fn is_date_tok(t: &Tok) -> bool {
    matches!(
        t,
        Tok::Year(_)
            | Tok::Month(_)
            | Tok::Minute(_)
            | Tok::MonthOrMinute(_)
            | Tok::Day(_)
            | Tok::Hour(_)
            | Tok::Second(_)
            | Tok::SubSec(_)
            | Tok::AmPm(..)
            | Tok::Elapsed(..)
    )
}

fn render_text(section: &Section, text: &str) -> String {
    let mut out = String::new();
    for t in &section.toks {
        match t {
            Tok::At => out.push_str(text),
            Tok::Lit(s) => out.push_str(s),
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------
// Decimal digits

/// A non-negative number as Excel holds it for display: at most 15
/// significant digits, `0.d1d2d3… × 10^exp`. No digits is zero.
#[derive(Debug, Clone, PartialEq)]
struct Decimal {
    digits: Vec<u8>,
    exp: i32,
}

impl Decimal {
    fn new(v: f64) -> Decimal {
        let v = v.abs();
        if v == 0.0 || !v.is_finite() {
            return Decimal { digits: Vec::new(), exp: 0 };
        }
        let s = format!("{v:.14e}");
        let (mantissa, e) = s.split_once('e').expect("exponent form");
        let digits: Vec<u8> = mantissa.bytes().filter(u8::is_ascii_digit).map(|b| b - b'0').collect();
        let exp = e.parse::<i32>().expect("exponent") + 1;
        Decimal { digits, exp }.trimmed()
    }

    fn trimmed(mut self) -> Decimal {
        while self.digits.last() == Some(&0) {
            self.digits.pop();
        }
        if self.digits.is_empty() {
            self.exp = 0;
        }
        self
    }

    fn is_zero(&self) -> bool {
        self.digits.is_empty()
    }

    /// Rounded half up to `places` digits after the point.
    fn round(&self, places: i32) -> Decimal {
        let keep = self.exp + places;
        if keep < 0 || self.is_zero() {
            return Decimal { digits: Vec::new(), exp: 0 };
        }
        #[allow(clippy::cast_sign_loss)]
        let keep = keep as usize;
        if keep >= self.digits.len() {
            return self.clone();
        }
        let mut digits = self.digits[..keep].to_vec();
        let mut exp = self.exp;
        if self.digits[keep] >= 5 {
            let mut k = digits.len();
            loop {
                if k == 0 {
                    digits.insert(0, 1);
                    exp += 1;
                    break;
                }
                k -= 1;
                if digits[k] == 9 {
                    digits[k] = 0;
                } else {
                    digits[k] += 1;
                    break;
                }
            }
        }
        Decimal { digits, exp }.trimmed()
    }

    /// Rounded half up to `n` significant digits.
    fn round_sig(&self, n: i32) -> Decimal {
        self.round(n - self.exp)
    }

    fn digit(&self, k: i32) -> u8 {
        usize::try_from(k).ok().and_then(|k| self.digits.get(k)).copied().unwrap_or(0)
    }

    /// The integer part's digits, empty below one.
    fn int_digits(&self) -> String {
        (0..self.exp.max(0)).map(|k| char::from(b'0' + self.digit(k))).collect()
    }

    /// The first `n` digits after the point.
    fn frac_digits(&self, n: usize) -> String {
        (0..n).map(|k| char::from(b'0' + self.digit(self.exp + i32::try_from(k).unwrap_or(i32::MAX)))).collect()
    }

    /// Plain notation, no trailing zeros after the point.
    fn plain(&self) -> String {
        if self.is_zero() {
            return "0".to_owned();
        }
        let int = self.int_digits();
        #[allow(clippy::cast_sign_loss)]
        let frac_len = (i32::try_from(self.digits.len()).unwrap_or(i32::MAX) - self.exp).max(0) as usize;
        let frac = self.frac_digits(frac_len);
        let int = if int.is_empty() { "0".to_owned() } else { int };
        if frac.is_empty() { int } else { format!("{int}.{frac}") }
    }

    /// `1.23457E+11`: mantissa without trailing zeros, two-digit exponent.
    fn scientific(&self, sig: i32) -> String {
        let d = self.round_sig(sig);
        let e = d.exp - 1;
        let m = Decimal { digits: d.digits.clone(), exp: 1 };
        format!("{}E{}{:02}", m.plain(), if e < 0 { '-' } else { '+' }, e.abs())
    }

    fn shifted(&self, by: i32) -> Decimal {
        if self.is_zero() { self.clone() } else { Decimal { digits: self.digits.clone(), exp: self.exp + by } }
    }
}

/// A number in the General format: up to 11 characters, switching to
/// scientific notation for large and very small magnitudes.
fn general(v: f64) -> String {
    if v == 0.0 {
        return "0".to_owned();
    }
    let d = Decimal::new(v);
    let e = d.exp - 1;
    let body = if !(-9..11).contains(&e) {
        d.scientific(6)
    } else if e >= 0 {
        // Rounding can carry into a twelfth digit (99999999999.5).
        let fixed = d.round((10 - (e + 1)).max(0)).plain();
        if fixed.len() > 11 { d.scientific(6) } else { fixed }
    } else if e >= -4 {
        d.round(9).plain()
    } else {
        let exact = d.plain();
        if exact.len() <= 11 { exact } else { d.scientific(6) }
    };
    if v < 0.0 { format!("-{body}") } else { body }
}

// ---------------------------------------------------------------------
// Number sections

/// Digits placed into placeholders right to left; digits left over go in
/// front of the first placeholder.
fn place_int(digits: &str, ph: &[u8]) -> Vec<String> {
    let digits: Vec<char> = digits.chars().collect();
    let mut out = vec![String::new(); ph.len()];
    let mut left = digits.len();
    for p in (0..ph.len()).rev() {
        if left > 0 {
            left -= 1;
            out[p] = digits[left].to_string();
        } else {
            out[p] = match ph[p] {
                b'0' => "0".to_owned(),
                b'?' => " ".to_owned(),
                _ => String::new(),
            };
        }
    }
    if left > 0 && !out.is_empty() {
        let lead: String = digits[..left].iter().collect();
        out[0].insert_str(0, &lead);
    }
    out
}

/// Fraction digits placed left to right; trailing zeros shown only for `0`.
fn place_frac(digits: &str, ph: &[u8]) -> Vec<String> {
    let digits: Vec<char> = digits.chars().collect();
    let mut out: Vec<String> = digits.iter().map(char::to_string).collect();
    for k in (0..ph.len()).rev() {
        if digits[k] != '0' || ph[k] == b'0' {
            break;
        }
        out[k] = if ph[k] == b'?' { " ".to_owned() } else { String::new() };
    }
    out
}

fn bytecount(s: &[u8], b: u8) -> usize {
    s.iter().fold(0, |n, &c| n + usize::from(c == b))
}

fn group_thousands(s: &str) -> String {
    let lead = s.len() - s.trim_start_matches(' ').len();
    let (pad, digits) = s.split_at(lead);
    let mut out = String::new();
    for (k, c) in digits.chars().enumerate() {
        if k > 0 && (digits.len() - k) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{pad}{out}")
}

/// The layout of a fixed or scientific number part.
#[derive(Debug, Clone, Default, PartialEq)]
struct Layout {
    int_ph: Vec<u8>,
    frac_ph: Vec<u8>,
    exp_ph: Vec<u8>,
    grouping: bool,
    scale: i32,
    percent: i32,
}

fn layout(toks: &[Tok]) -> Layout {
    let exp_at = toks.iter().position(|t| matches!(t, Tok::Exp(_))).unwrap_or(toks.len());
    let point = toks[..exp_at].iter().position(|t| *t == Tok::Point).unwrap_or(exp_at);
    let digits = |r: &[Tok]| r.iter().filter_map(|t| if let Tok::Digit(c) = t { Some(*c) } else { None }).collect();
    let int_ph: Vec<u8> = digits(&toks[..point]);
    let frac_ph: Vec<u8> = digits(&toks[point.min(exp_at)..exp_at]);
    let exp_ph: Vec<u8> = digits(&toks[exp_at..]);
    // `ahead[k]`: a digit placeholder at or after `k`, before the next
    // point (or the exponent). One pass each way keeps this linear.
    let mut ahead = vec![false; exp_at + 1];
    for k in (0..exp_at).rev() {
        ahead[k] = match toks[k] {
            Tok::Digit(_) => true,
            Tok::Point => false,
            _ => ahead[k + 1],
        };
    }
    let (mut grouping, mut scale, mut seen_digit) = (false, 0, false);
    for (k, t) in toks[..exp_at].iter().enumerate() {
        match t {
            Tok::Digit(_) => seen_digit = true,
            // Between digits of the integer part: a thousands separator;
            // after the last digit of a part: a scale by 1000.
            Tok::Comma if seen_digit && ahead[k + 1] && k < point => grouping = true,
            Tok::Comma if seen_digit && !ahead[k + 1] => scale += 1,
            _ => {}
        }
    }
    let percent = i32::try_from(toks.iter().filter(|t| **t == Tok::Percent).count()).unwrap_or(0);
    Layout { int_ph, frac_ph, exp_ph, grouping, scale, percent }
}

/// The section's text for `v`, and whether it shows a number other than
/// zero (`None` when it shows no number at all, only text).
fn render_number(section: &Section, v: f64) -> (String, Option<bool>) {
    let toks = &section.toks;
    if toks.contains(&Tok::General) {
        let text = toks
            .iter()
            .map(|t| match t {
                Tok::General => general(v),
                Tok::Lit(s) => s.clone(),
                Tok::Percent => "%".to_owned(),
                _ => String::new(),
            })
            .collect();
        return (text, Some(v != 0.0));
    }
    if let Some(slash) = section.slash {
        return render_fraction(toks, slash, v);
    }
    let default = Layout::default();
    let l = section.layout.as_ref().unwrap_or(&default);
    let shows_number = !l.int_ph.is_empty() || !l.frac_ph.is_empty();
    let mut d = Decimal::new(v).shifted(2 * l.percent - 3 * l.scale);
    let exp_value = if toks.iter().any(|t| matches!(t, Tok::Exp(_))) {
        let ni = i32::try_from(l.int_ph.len()).unwrap_or(1);
        let frac = i32::try_from(l.frac_ph.len()).unwrap_or(0);
        let engineering = ni > 1 && l.int_ph.contains(&b'#');
        let exponent = |d: &Decimal| {
            if d.is_zero() {
                0
            } else if engineering {
                (d.exp - 1).div_euclid(ni) * ni
            } else {
                d.exp - ni.max(1)
            }
        };
        let mut e = exponent(&d);
        let mut m = d.shifted(-e).round(frac);
        if !m.is_zero() && m.exp > ni.max(1) {
            // Rounding carried into a new digit (9.99 to 10.0).
            d = m.shifted(e);
            e = exponent(&d);
            m = d.shifted(-e).round(frac);
        }
        d = m;
        Some(e)
    } else {
        d = d.round(i32::try_from(l.frac_ph.len()).unwrap_or(0));
        None
    };
    let int_digits = d.int_digits();
    let mut ints = place_int(&int_digits, &l.int_ph);
    if l.grouping && !ints.is_empty() {
        let joined: String = ints.concat();
        ints = vec![String::new(); ints.len()];
        ints[0] = group_thousands(&joined);
    }
    let fracs = place_frac(&d.frac_digits(l.frac_ph.len()), &l.frac_ph);
    let mut out = String::new();
    let (mut ki, mut kf, mut seen_point, mut in_exp) = (0, 0, false, false);
    let mut exp_text: Option<String> = None;
    for t in toks {
        match t {
            Tok::Digit(_) if in_exp => {
                if let Some(s) = exp_text.take() {
                    out.push_str(&s);
                }
            }
            Tok::Digit(_) if !seen_point => {
                out.push_str(&ints[ki]);
                ki += 1;
            }
            Tok::Digit(_) => {
                out.push_str(&fracs[kf]);
                kf += 1;
            }
            Tok::Point if !in_exp => {
                if l.int_ph.is_empty() {
                    out.push_str(&int_digits);
                }
                seen_point = true;
                out.push('.');
            }
            Tok::Exp(plus) => {
                in_exp = true;
                let e = exp_value.unwrap_or(0);
                let width = bytecount(&l.exp_ph, b'0').max(1);
                let sign = if e < 0 {
                    "-"
                } else if *plus {
                    "+"
                } else {
                    ""
                };
                out.push('E');
                exp_text = Some(format!("{sign}{:0width$}", e.abs()));
            }
            Tok::Percent => out.push('%'),
            Tok::Lit(s) => out.push_str(s),
            _ => {}
        }
    }
    if let Some(s) = exp_text {
        out.push_str(&s);
    }
    (out, shows_number.then(|| !d.is_zero()))
}

/// The slash of a fraction pattern: digit placeholders before it and
/// placeholders or a fixed denominator after it.
fn fraction_slash(toks: &[Tok]) -> Option<usize> {
    let k = toks.iter().position(|t| *t == Tok::Slash)?;
    let before = k > 0 && matches!(toks[k - 1], Tok::Digit(_));
    let after = matches!(toks.get(k + 1), Some(Tok::Digit(_) | Tok::Denom(_)));
    (before && after).then_some(k)
}

fn render_fraction(toks: &[Tok], slash: usize, v: f64) -> (String, Option<bool>) {
    let num_start = toks[..slash].iter().rposition(|t| !matches!(t, Tok::Digit(_))).map_or(0, |p| p + 1);
    let num_ph: Vec<u8> =
        toks[num_start..slash].iter().filter_map(|t| if let Tok::Digit(c) = t { Some(*c) } else { None }).collect();
    let den_end = toks[slash + 1..].iter().position(|t| !matches!(t, Tok::Digit(_))).map_or(toks.len(), |p| {
        if p == 0 && matches!(toks[slash + 1], Tok::Denom(_)) { slash + 2 } else { slash + 1 + p }
    });
    let fixed = if let Some(Tok::Denom(n)) = toks.get(slash + 1) { Some(u64::from(*n).max(1)) } else { None };
    let den_ph: Vec<u8> =
        toks[slash + 1..den_end].iter().filter_map(|t| if let Tok::Digit(c) = t { Some(*c) } else { None }).collect();
    let int_ph: Vec<u8> =
        toks[..num_start].iter().filter_map(|t| if let Tok::Digit(c) = t { Some(*c) } else { None }).collect();
    let has_int = !int_ph.is_empty();
    let mut whole = if has_int { v.trunc() } else { 0.0 };
    let f = v - whole;
    let (mut num, den) = match fixed {
        Some(den) => {
            #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let num = (f * den as f64).round() as u64;
            (num, den)
        }
        None => best_fraction(f, 10u64.pow(u32::try_from(den_ph.len().clamp(1, 4)).unwrap_or(1)) - 1),
    };
    if has_int && num == den {
        whole += 1.0;
        num = 0;
    }
    let blank = has_int && num == 0;
    // A zero value still shows its whole part (`0`), even under `#`.
    let whole_digits = match Decimal::new(whole).round(0).int_digits() {
        d if d.is_empty() && blank => "0".to_owned(),
        d => d,
    };
    let ints = place_int(&whole_digits, &int_ph);
    let nums = place_int(&num.to_string(), &num_ph);
    let den_text = den.to_string();
    let mut out = String::new();
    let mut ki = 0;
    for (k, t) in toks.iter().enumerate() {
        let width_of = |s: &str| " ".repeat(s.chars().count());
        match t {
            Tok::Digit(_) if k < num_start => {
                out.push_str(&ints[ki]);
                ki += 1;
            }
            Tok::Digit(_) if k < slash => {
                let s = &nums[k - num_start];
                out.push_str(&if blank { width_of(s) } else { s.clone() });
            }
            Tok::Slash => out.push(if blank { ' ' } else { '/' }),
            Tok::Denom(_) | Tok::Digit(_) if k == slash + 1 => {
                let mut s = den_text.clone();
                if fixed.is_none() {
                    while s.len() < den_ph.len() {
                        s.push(if den_ph[s.len()] == b'?' { ' ' } else { '0' });
                    }
                }
                out.push_str(&if blank { width_of(&s) } else { s });
            }
            Tok::Digit(_) if k < den_end => {}
            Tok::Lit(s) if blank && k > num_start.saturating_sub(1) && k < den_end => out.push_str(&width_of(s)),
            Tok::Lit(s) => out.push_str(s),
            Tok::Percent => out.push('%'),
            _ => {}
        }
    }
    (out, Some(whole != 0.0 || num != 0))
}

/// The last continued-fraction convergent of `f` whose denominator is at
/// most `max_den`. Excel picks this, not the closest fraction: it shows
/// 0.14159 under `??/??` as 1/7, not 14/99.
fn best_fraction(f: f64, max_den: u64) -> (u64, u64) {
    let (mut p0, mut q0, mut p1, mut q1) = (0u64, 1u64, 1u64, 0u64);
    let mut x = f;
    for _ in 0..64 {
        // The first term is the whole part, which may exceed `max_den`
        // (`?/?` shows 12.5 as 25/2); later terms that large overflow the
        // denominator, which the check below catches.
        let a = x.floor();
        if !a.is_finite() || a >= 1.8e19 {
            break;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let a = a as u64;
        let (p2, q2) = (a.saturating_mul(p1).saturating_add(p0), a.saturating_mul(q1).saturating_add(q0));
        if q2 > max_den {
            break;
        }
        (p0, q0, p1, q1) = (p1, q1, p2, q2);
        #[allow(clippy::cast_precision_loss)]
        let rest = x - a as f64;
        if rest < 1e-9 {
            break;
        }
        x = 1.0 / rest;
    }
    if q1 == 0 { (0, 1) } else { (p1, q1) }
}

// ---------------------------------------------------------------------
// Dates

/// A serial broken into calendar and clock parts, rounded to the shown
/// fraction of a second.
struct Instant {
    serial_day: i64,
    year: i64,
    month: u32,
    day: u32,
    weekday: usize,
    hour: i64,
    minute: i64,
    second: i64,
    /// Fraction of a second in units of `10^-sub`.
    sub: i64,
    /// Whole seconds since serial zero.
    total_seconds: i64,
}

impl Instant {
    fn new(v: f64, sub: usize, date1904: bool) -> Option<Instant> {
        let scale = 10i64.pow(u32::try_from(sub).unwrap_or(0));
        #[allow(clippy::cast_precision_loss)]
        let ticks = v * 86_400.0 * scale as f64;
        if !(0.0..=3e17).contains(&ticks) {
            return None;
        }
        #[allow(clippy::cast_possible_truncation)]
        let ticks = ticks.round() as i64;
        let per_day = 86_400 * scale;
        let serial_day = ticks / per_day;
        if serial_day > if date1904 { MAX_DATE_1904 } else { MAX_DATE } {
            return None;
        }
        let rem = ticks % per_day;
        let secs = rem / scale;
        let (year, month, day) = excel_date(serial_day, date1904);
        let weekday = if date1904 {
            usize::try_from((serial_day + 5).rem_euclid(7)).unwrap_or(0)
        } else {
            usize::try_from((serial_day + 6).rem_euclid(7)).unwrap_or(0)
        };
        Some(Instant {
            serial_day,
            year,
            month,
            day,
            weekday,
            hour: secs / 3600,
            minute: secs / 60 % 60,
            second: secs % 60,
            sub: rem % scale,
            total_seconds: ticks / scale,
        })
    }

    fn has_time(&self) -> bool {
        self.hour != 0 || self.minute != 0 || self.second != 0 || self.sub != 0
    }

    fn sub_text(&self, sub: usize) -> String {
        if sub == 0 { String::new() } else { format!(".{:0sub$}", self.sub) }
    }
}

/// Days since 1970-01-01 to a civil date (proleptic Gregorian).
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// Excel's calendar for a serial day. The 1900 system counts a 1900-02-29
/// that never was (serial 60) and shows serial 0 as 1900-01-00.
fn excel_date(serial: i64, date1904: bool) -> (i64, u32, u32) {
    if date1904 {
        return civil(serial - 24_107);
    }
    match serial {
        0 => (1900, 1, 0),
        60 => (1900, 2, 29),
        s if s < 60 => civil(s - 25_568),
        s => civil(s - 25_569),
    }
}

fn render_date(section: &Section, v: f64, date1904: bool) -> Option<String> {
    if v < 0.0 {
        return None;
    }
    let sub = section.toks.iter().filter_map(|t| if let Tok::SubSec(n) = t { Some(*n) } else { None }).max();
    let t = Instant::new(v, sub.unwrap_or(0).min(3), date1904)?;
    let twelve = section.toks.iter().any(|t| matches!(t, Tok::AmPm(..)));
    let pad = |n: i64, width: usize| if width >= 2 { format!("{n:02}") } else { n.to_string() };
    let mut out = String::new();
    for tok in &section.toks {
        match tok {
            Tok::Lit(s) => out.push_str(s),
            Tok::Year(n) if *n <= 2 => {
                let _ = write!(out, "{:02}", t.year.rem_euclid(100));
            }
            Tok::Year(_) => {
                let _ = write!(out, "{:04}", t.year);
            }
            Tok::Month(n) => {
                let name = MONTHS[(t.month as usize).clamp(1, 12) - 1];
                match n {
                    1 | 2 => out.push_str(&pad(i64::from(t.month), *n)),
                    3 => out.push_str(&name[..3]),
                    5 => out.push_str(&name[..1]),
                    _ => out.push_str(name),
                }
            }
            Tok::Day(n) => match n {
                1 | 2 => out.push_str(&pad(i64::from(t.day), *n)),
                3 => out.push_str(&DAYS[t.weekday][..3]),
                _ => out.push_str(DAYS[t.weekday]),
            },
            Tok::Hour(n) => {
                let h = if twelve {
                    match t.hour % 12 {
                        0 => 12,
                        h => h,
                    }
                } else {
                    t.hour
                };
                out.push_str(&pad(h, *n));
            }
            Tok::Minute(n) => out.push_str(&pad(t.minute, *n)),
            Tok::Second(n) => out.push_str(&pad(t.second, *n)),
            Tok::SubSec(n) => {
                let s = t.sub_text(sub.unwrap_or(0).min(3));
                out.push_str(&s[..(n + 1).min(s.len())]);
            }
            Tok::AmPm(am, pm) => out.push_str(if t.hour < 12 { am } else { pm }),
            Tok::Elapsed(unit, n) => {
                let value = match unit {
                    'h' => t.total_seconds / 3600,
                    'm' => t.total_seconds / 60,
                    _ => t.total_seconds,
                };
                let _ = write!(out, "{value:0n$}");
            }
            _ => {}
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(code: &str, v: f64) -> String {
        Format::parse(code).display(&Value::Number(v), false)
    }

    #[test]
    fn general_numbers() {
        for (v, want) in [
            (0.0, "0"),
            (1234.5, "1234.5"),
            (-3.0, "-3"),
            (0.1 + 0.2, "0.3"),
            (1.0 / 3.0, "0.333333333"),
            (123_456_789_012.0, "1.23457E+11"),
            (12_345_678_901.0, "12345678901"),
            (0.000_012_345_67, "1.23457E-05"),
            (0.000_012_3, "0.0000123"),
            (1e-10, "1E-10"),
            (99_999_999_999.5, "1E+11"),
        ] {
            assert_eq!(show("General", v), want, "{v}");
        }
    }

    #[test]
    fn fixed_patterns() {
        for (code, v, want) in [
            ("0", 2.5, "3"),
            ("0", -0.4, "0"),
            ("0.00", -0.004, "0.00"),
            ("\"fixed\"", -5.0, "-fixed"),
            ("\"0\"", -5.0, "-0"),
            ("#", -0.4, ""),
            ("0\" in 2024\"", -0.4, "0 in 2024"),
            ("", -5.0, "-5"),
            ("0.00", 2.675, "2.68"),
            ("#,##0", 1_234_567.0, "1,234,567"),
            ("#,##0.00", -1234.5, "-1,234.50"),
            ("#,###", 0.0, ""),
            ("0.0%", 0.1234, "12.3%"),
            ("#,##0,", 1_234_567.0, "1,235"),
            ("0.0,,", 12_345_678.0, "12.3"),
            ("000-00-0000", 123_456_789.0, "123-45-6789"),
            ("#.##", 1.0, "1."),
            (".00", 1.5, "1.50"),
            ("00000", 42.0, "00042"),
            ("#,##0;(#,##0)", -1234.0, "(1,234)"),
            ("0;-0;\"zero\"", 0.0, "zero"),
            ("\"Total \"0", 7.0, "Total 7"),
            ("_-* #,##0_-;-* #,##0_-;_-* \"-\"_-;_-@_-", -1234.0, "-1,234"),
            ("_-* #,##0_-;-* #,##0_-;_-* \"-\"_-;_-@_-", 0.0, "-"),
            ("[Red]0.00", 1.234_56, "1.23"),
            ("[$€-407] #,##0.00", 5.0, "€ 5.00"),
            ("0.00E+00", 12345.0, "1.23E+04"),
            ("##0.0E+0", 12345.0, "12.3E+3"),
            ("0.00E+00", 0.000_123, "1.23E-04"),
            ("0.0E+00", 9.99, "1.0E+01"),
        ] {
            assert_eq!(show(code, v), want, "{code} {v}");
        }
    }

    #[test]
    fn fractions() {
        assert_eq!(show("# ?/?", 1.5), "1 1/2");
        assert_eq!(show("# ??/??", 2.141_59), "2  1/7");
        assert_eq!(show("# ?/?", 0.0), "0");
        assert_eq!(show("# ?/?", 1.0), "1");
        assert_eq!(show("?/8", 0.375), "3/8");
        assert_eq!(show("?/?", 12.5), "25/2");
        assert_eq!(show("??/??", 100.5), "201/2");
        assert_eq!(show("?/8", -0.01), "0/8");
    }

    #[test]
    fn dates_and_times() {
        for (code, v, want) in [
            ("yyyy-mm-dd", 45_292.0, "2024-01-01"),
            ("m/d/yy", 45_292.0, "1/1/24"),
            ("d-mmm-yy", 45_351.0, "29-Feb-24"),
            ("dddd, mmmm d, yyyy", 45_292.0, "Monday, January 1, 2024"),
            ("h:mm AM/PM", 0.75, "6:00 PM"),
            ("h:mm am/pm", 0.75, "6:00 pm"),
            ("hh:mm:ss", 0.5, "12:00:00"),
            ("[h]:mm", 1.5, "36:00"),
            ("mm:ss.0", 0.000_011_574, "00:01.0"),
            ("yyyy-mm-dd", 60.0, "1900-02-29"),
            ("yyyy-mm-dd", 61.0, "1900-03-01"),
            ("ddd", 1.0, "Sun"),
            ("yyyy\"년\" m\"월\" d\"일\"", 45_292.0, "2024년 1월 1일"),
        ] {
            assert_eq!(show(code, v), want, "{code} {v}");
        }
        assert_eq!(Format::parse("yyyy-mm-dd").display(&Value::Number(0.0), true), "1904-01-01");
        assert_eq!(show("yyyy-mm-dd", -1.0), "########");
        assert_eq!(show("builtin:31", 45_292.0), "2024-01-01");
        assert_eq!(Format::parse("builtin:31").iso_date(45_292.0, false).as_deref(), Some("2024-01-01"));
        assert_eq!(show("builtin:42", 1234.0), "1234");
    }

    #[test]
    fn iso_dates() {
        let iso = |code: &str, v: f64| Format::parse(code).iso_date(v, false);
        assert_eq!(iso("yyyy-mm-dd", 45_292.0).as_deref(), Some("2024-01-01"));
        assert_eq!(iso("yyyy-mm-dd", 45_292.75).as_deref(), Some("2024-01-01T18:00:00"));
        assert_eq!(iso("m/d/yy h:mm", 45_292.0).as_deref(), Some("2024-01-01T00:00:00"));
        assert_eq!(iso("h:mm", 0.25).as_deref(), Some("06:00:00"));
        assert_eq!(iso("[h]:mm", 1.5), None);
        assert_eq!(iso("hh:mm:ss.0000", 0.5 + 0.123 / 86_400.0).as_deref(), Some("12:00:00.123"));
        assert_eq!(iso("h:mm", 0.999_999_99).as_deref(), Some("00:00:00"));
        assert_eq!(iso("yyyy-mm-dd", 0.0), None);
        assert_eq!(iso("yyyy-mm-dd", 60.0), None);
        assert_eq!(Format::parse("yyyy-mm-dd").iso_date(2_957_003.0, true).as_deref(), Some("9999-12-31"));
        assert_eq!(Format::parse("yyyy-mm-dd").iso_date(2_957_004.0, true), None);
        assert_eq!(iso("builtin:14", 45_292.0).as_deref(), Some("2024-01-01"));
        assert_eq!(iso("#,##0", 45_292.0), None);
    }

    #[test]
    fn text_sections() {
        let f = Format::parse("0.00;-0.00;0;\"Note: \"@");
        assert_eq!(f.display(&Value::Text("x".into()), false), "Note: x");
        assert!(f.shapes(&Value::Text("x".into())));
        let at = Format::parse("@");
        assert_eq!(at.display(&Value::Number(1.5), false), "1.5");
        assert!(!at.shapes(&Value::Text("x".into())));
        assert!(Format::parse("General").is_general());
        assert!(!Format::parse("General").shapes(&Value::Number(1.0)));
    }

    #[test]
    fn conditions() {
        assert_eq!(show("[>=1000]#,##0,\"K\";0", 12_345.0), "12K");
        assert_eq!(show("[>=1000]#,##0,\"K\";0", 12.0), "12");
        assert_eq!(show("[>=1000]#,##0,\"K\";0", -5.0), "-5");
        assert_eq!(show("[<=100]0;0", -5.0), "-5");
        assert_eq!(show("[<0]0;0", -5.0), "5");
        assert_eq!(show("[=-5]0;0", -5.0), "5");
        assert_eq!(show("[<=-1]0;[>=1]0;0", -0.5), "-1");
        assert_eq!(show("[>=0]0;0", -5.0), "5");
        assert_eq!(show("[>0]0;[<0]0", 0.0), NO_TEXT);
        assert_eq!(show("0;[<0]0", 5.0), "5");
        assert_eq!(show("0.00;[<-10]0;0", 5.0), "5.00");
        assert_eq!(show("0.00;[<-10]0;0", -5.0), "-5");
        assert_eq!(show("[>100]0;0", -5.0), "-5");
    }
}
