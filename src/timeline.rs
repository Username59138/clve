//! ASCII timeline for the terminal.

use crate::check::{Report, Span};
use crate::time;

pub fn draw(report: &Report, width: usize) -> String {
    let total = report.duration;
    if report.layers.is_empty() || total <= 0.0 {
        return "timeline is empty\n".into();
    }
    let cols = width.max(20);
    let scale = cols as f64 / total;
    let col = |t: f64| ((t * scale).round() as usize).min(cols);

    // tracks: visual from top to bottom, then audio
    let mut tracks: Vec<(bool, i32)> = report
        .layers
        .iter()
        .map(|s| (s.kind.is_audio_only(), s.z))
        .collect();
    tracks.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    tracks.dedup();

    let label_w = tracks
        .iter()
        .map(|(a, z)| label(*a, *z).chars().count())
        .max()
        .unwrap_or(0);

    let mut out = String::new();
    for (audio, z) in &tracks {
        let mut row = vec![' '; cols];
        let mut spans: Vec<&Span> = report
            .layers
            .iter()
            .filter(|s| s.kind.is_audio_only() == *audio && s.z == *z)
            .collect();
        spans.sort_by(|a, b| a.start.total_cmp(&b.start));
        for s in spans {
            paint(&mut row, col(s.start), col(s.end).max(col(s.start) + 1), &s.name);
        }
        let lbl = label(*audio, *z);
        out += &format!(
            "{lbl:>label_w$} |{}|\n",
            row.into_iter().collect::<String>()
        );
    }

    // time ruler
    let mut ruler = vec![' '; cols + 1];
    let step = nice_step(total, cols);
    let mut t = 0.0;
    while t <= total + 1e-9 {
        let txt = time::format(t);
        let c = col(t);
        if c + txt.len() <= cols + 1 && ruler[c..c + txt.len()].iter().all(|&ch| ch == ' ') {
            for (i, ch) in txt.chars().enumerate() {
                ruler[c + i] = ch;
            }
        }
        t += step;
    }
    // pin the end time to the right edge if there is room
    let end = time::format(total);
    let from = (cols + 1).saturating_sub(end.len());
    if ruler[from.saturating_sub(1)..].iter().all(|&ch| ch == ' ') {
        for (i, ch) in end.chars().enumerate() {
            ruler[from + i] = ch;
        }
    }
    out += &format!(
        "{:>label_w$}  {}\n",
        "",
        ruler.into_iter().collect::<String>().trim_end()
    );
    out += &format!("{:>label_w$}  total {}\n", "", time::format(total));
    out
}

fn label(audio: bool, z: i32) -> String {
    if audio {
        format!("audio z={z}")
    } else {
        format!("z={z}")
    }
}

/// Paints [name.....] into cells from..to, truncating the name if short on space.
fn paint(row: &mut [char], from: usize, to: usize, name: &str) {
    let to = to.min(row.len());
    if from >= to {
        return;
    }
    let w = to - from;
    let body: Vec<char> = if w >= 2 {
        let inner = w - 2;
        let mut v = vec!['['];
        let chars: Vec<char> = name.chars().collect();
        for i in 0..inner {
            v.push(*chars.get(i).unwrap_or(&' '));
        }
        v.push(']');
        v
    } else {
        vec!['#']
    };
    row[from..to].copy_from_slice(&body);
}

/// Tick step: 1, 2, 5, 10, 15, 30 s, 1, 2, 5 min... so labels don't collide.
fn nice_step(total: f64, cols: usize) -> f64 {
    let max_marks = (cols / 10).max(1) as f64;
    let raw = total / max_marks;
    const STEPS: [f64; 12] = [
        0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0,
    ];
    STEPS
        .iter()
        .copied()
        .find(|s| *s >= raw)
        .unwrap_or(3600.0)
}
