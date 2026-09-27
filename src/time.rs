//! Parsing and formatting time: "5s", "1.5s", "500ms", "00:12", "01:02:03.5".

use anyhow::{bail, Result};

pub fn parse(s: &str) -> Result<f64> {
    let s = s.trim();
    if s.is_empty() {
        bail!("empty time value");
    }
    if let Some(ms) = s.strip_suffix("ms") {
        return num(ms, s).map(|v| v / 1000.0);
    }
    if let Some(sec) = s.strip_suffix('s') {
        return num(sec, s);
    }
    if s.contains(':') {
        let mut total = 0.0;
        for part in s.split(':') {
            total = total * 60.0 + num(part, s)?;
        }
        return Ok(total);
    }
    num(s, s)
}

fn num(part: &str, whole: &str) -> Result<f64> {
    match part.trim().parse::<f64>() {
        Ok(v) if v >= 0.0 && v.is_finite() => Ok(v),
        _ => bail!("can't parse time \"{whole}\" (examples: 5s, 1.5s, 500ms, 00:12, 01:02:03)"),
    }
}

/// 72.5 -> "01:12.5", 3725 -> "1:02:05"
pub fn format(t: f64) -> String {
    let total = (t * 10.0).round() / 10.0;
    let h = (total / 3600.0).floor() as u64;
    let m = ((total % 3600.0) / 60.0).floor() as u64;
    let s = total % 60.0;
    let sec = if s.fract().abs() < 1e-6 {
        format!("{:02}", s as u64)
    } else {
        format!("{:04.1}", s)
    };
    if h > 0 {
        format!("{h}:{m:02}:{sec}")
    } else {
        format!("{m:02}:{sec}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        assert_eq!(parse("5s").unwrap(), 5.0);
        assert_eq!(parse("500ms").unwrap(), 0.5);
        assert_eq!(parse("00:12").unwrap(), 12.0);
        assert_eq!(parse("01:02:03").unwrap(), 3723.0);
        assert_eq!(parse("2.5").unwrap(), 2.5);
        assert!(parse("abc").is_err());
        assert!(parse("-1s").is_err());
    }

    #[test]
    fn formats() {
        assert_eq!(format(72.0), "01:12");
        assert_eq!(format(10.5), "00:10.5");
        assert_eq!(format(3725.0), "1:02:05");
    }
}
