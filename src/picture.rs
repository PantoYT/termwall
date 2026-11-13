//! termwall logo-from PICTURE: a picture -> logo.txt ($1..$9) and its colors.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

type Px = [i64; 4];
type Rgb = [i64; 3];

/// A picture as (width, height, rows of RGBA). PNG, GIF (the first frame), JPEG, WebP, BMP.
pub fn read_picture(path: &std::path::Path) -> Result<(usize, usize, Vec<Vec<Px>>), String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let img =
        image::load_from_memory(&data).map_err(|e| format!("{e} (termwall reads PNG, GIF, JPEG, WebP and BMP)"))?.to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let rows = (0..h).map(|y| (0..w).map(|x| img.get_pixel(x as u32, y as u32).0.map(i64::from)).collect()).collect();
    Ok((w, h, rows))
}

/// Area-average down to cols x out_rows (alpha-weighted); pixel art picks the nearest pixel.
fn shrink(w: usize, h: usize, rows: &[Vec<Px>], cols: usize, out_rows: usize) -> Vec<Vec<Px>> {
    let sharp = w <= cols * 2;
    let mut out = Vec::with_capacity(out_rows);
    for oy in 0..out_rows {
        let y0 = oy * h / out_rows;
        let y1 = (y0 + 1).max((oy + 1) * h / out_rows);
        let mut line = Vec::with_capacity(cols);
        for ox in 0..cols {
            let x0 = ox * w / cols;
            let x1 = (x0 + 1).max((ox + 1) * w / cols);
            if sharp {
                line.push(rows[(h - 1).min((y0 + y1) / 2)][(w - 1).min((x0 + x1) / 2)]);
                continue;
            }
            let (mut r, mut g, mut b, mut a, mut n) = (0, 0, 0, 0, 0);
            for row in &rows[y0..y1.min(h)] {
                for p in &row[x0.min(w)..x1.min(w)] {
                    r += p[0] * p[3];
                    g += p[1] * p[3];
                    b += p[2] * p[3];
                    a += p[3];
                    n += 1;
                }
            }
            line.push(if a != 0 { [r / a, g / a, b / a, a / n] } else { [0, 0, 0, 0] });
        }
        out.push(line);
    }
    out
}

/// A picture with no transparency: the color around its border, flooded in from the edges,
/// becomes transparent (a logo on a white square loses the square, not its own white).
/// Returns the rows and that color (None: nothing dropped).
fn drop_background(rows: Vec<Vec<Px>>, tol: i64) -> (Vec<Vec<Px>>, Option<Rgb>) {
    let h = rows.len();
    let w = rows.first().map_or(0, Vec::len);
    if h == 0 || w == 0 || rows.iter().flatten().any(|p| p[3] < 255) {
        return (rows, None);
    }
    let mut border: Vec<Px> = Vec::new();
    border.extend(&rows[0]);
    border.extend(&rows[h - 1]);
    border.extend(rows.iter().map(|r| r[0]));
    border.extend(rows.iter().map(|r| r[w - 1]));
    let mut counts: HashMap<Px, usize> = HashMap::new();
    for p in &border {
        *counts.entry(*p).or_default() += 1;
    }
    let (bg, n) = counts.iter().max_by_key(|(_, n)| **n).map(|(p, n)| (*p, *n)).unwrap_or(([0; 4], 0));
    if (n as f64) < border.len() as f64 * 0.6 {
        return (rows, None); // no single background color: keep the whole picture
    }
    let near = |p: &Px| (0..3).map(|i| (p[i] - bg[i]).abs()).sum::<i64>() <= tol;
    let mut rows = rows;
    let mut todo: Vec<(i64, i64)> = Vec::new();
    for x in 0..w as i64 {
        todo.push((x, 0));
        todo.push((x, h as i64 - 1));
    }
    for y in 0..h as i64 {
        todo.push((0, y));
        todo.push((w as i64 - 1, y));
    }
    while let Some((x, y)) = todo.pop() {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            continue;
        }
        let p = rows[y as usize][x as usize];
        if p[3] != 0 && near(&p) {
            rows[y as usize][x as usize] = [0, 0, 0, 0];
            todo.extend([(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]);
        }
    }
    (rows, Some([bg[0], bg[1], bg[2]]))
}

/// Squared distance from color c to the line between colors a and b (a == b: to a).
fn seg_d2(c: &[i64], a: &[i64], b: &[i64]) -> f64 {
    let ab: Vec<f64> = (0..3).map(|i| (b[i] - a[i]) as f64).collect();
    let ac: Vec<f64> = (0..3).map(|i| (c[i] - a[i]) as f64).collect();
    let n: f64 = ab.iter().map(|x| x * x).sum();
    let t = if n != 0.0 { ((0..3).map(|i| ab[i] * ac[i]).sum::<f64>() / n).clamp(0.0, 1.0) } else { 0.0 };
    (0..3).map(|i| (ac[i] - t * ab[i]).powi(2)).sum()
}

fn d2(a: &[i64], b: &[i64]) -> i64 {
    (a[0] - b[0]).pow(2) + (a[1] - b[1]).pow(2) + (a[2] - b[2]).pow(2)
}

/// Up to k colors for these pixels (k-means from the most common color outwards), the most used
/// first; close ones are merged. bg: a dropped background, so its blend into the edges goes too.
fn quantize(pixels: &[Px], k: usize, bg: Option<Rgb>) -> Vec<Rgb> {
    let mut hist: IndexMap<Rgb, i64> = IndexMap::new();
    for p in pixels {
        let q = [p[0] >> 3 << 3 | 4, p[1] >> 3 << 3 | 4, p[2] >> 3 << 3 | 4];
        *hist.entry(q).or_insert(0) += 1;
    }
    if hist.is_empty() {
        return vec![];
    }
    // max() keeps the first of equals, like Python's max over a dict
    let first_max = |key: &dyn Fn(&Rgb, i64) -> f64| -> Rgb {
        let mut best: Option<(Rgb, f64)> = None;
        for (c, n) in &hist {
            let v = key(c, *n);
            if best.is_none_or(|(_, b)| v > b) {
                best = Some((*c, v));
            }
        }
        best.map(|b| b.0).unwrap_or([0; 3])
    };
    let mut cents: Vec<Vec<i64>> = vec![first_max(&|_, n| n as f64).to_vec()];
    while cents.len() < k.min(hist.len()) {
        let c = first_max(&|c, n| (n as f64).sqrt() * cents.iter().map(|x| d2(c, x)).min().unwrap_or(0) as f64);
        cents.push(c.to_vec());
    }
    let total: i64 = hist.values().sum();
    let mut counts: Vec<i64> = Vec::new();
    for step in 0..14 {
        let mut sums = vec![[0i64; 4]; cents.len()];
        for (c, n) in &hist {
            let mut best = 0;
            for i in 1..cents.len() {
                if d2(c, &cents[i]) < d2(c, &cents[best]) {
                    best = i;
                }
            }
            let s = &mut sums[best];
            s[0] += c[0] * n;
            s[1] += c[1] * n;
            s[2] += c[2] * n;
            s[3] += n;
        }
        let mut next: Vec<Vec<i64>> =
            sums.iter().filter(|s| s[3] != 0).map(|s| vec![s[0] / s[3], s[1] / s[3], s[2] / s[3], s[3]]).collect();
        if step == 7 {
            // a color (under a fifth of the picture) that mixes two bigger ones is an edge blend:
            // dropped, its pixels go to those (a small white eye stays)
            next.sort_by_key(|c| -c[3]);
            let mut keep: Vec<Vec<i64>> = Vec::new();
            for c in next {
                let mut ends: Vec<Vec<i64>> = keep.clone();
                if let Some(b) = bg {
                    ends.push(b.to_vec());
                }
                let lim = 28f64.powi(2);
                let inner_bg = bg.is_some_and(|b| seg_d2(&c, &b, &b) < lim);
                let blend = ends.iter().any(|a| ends.iter().any(|b| seg_d2(&c, a, b) < lim));
                if c[3] as f64 >= total as f64 * 0.2 || inner_bg || !blend {
                    keep.push(c);
                }
            }
            next = keep;
        }
        counts = next.iter().map(|c| c[3]).collect();
        cents = next.into_iter().map(|c| c[..3].to_vec()).collect();
    }
    let mut pairs: Vec<(Vec<i64>, i64)> = cents.into_iter().zip(counts).collect();
    pairs.sort_by_key(|p| -p.1);
    let mut merged: Vec<Rgb> = Vec::new();
    for (c, _) in pairs {
        if merged.iter().all(|m| d2(&c, m) > 24 * 24) {
            merged.push([c[0], c[1], c[2]]);
        }
    }
    merged
}

/// A picture -> (logo text with $1..$N, ["#rrggbb" for $1..$N]), at most 120 x 40 characters.
/// blocks: half blocks, two pixels a character, one color each (logo.txt has no background
/// color, so where the two pixels differ the rarer one wins: lines and highlights survive a
/// fill); ascii: characters from light to dense.
pub fn picture_logo(
    path: &std::path::Path,
    width: usize,
    chars: &str,
    colors: usize,
    keep_bg: bool,
) -> Result<(String, Vec<String>), String> {
    let (mut w, mut h, mut rows) = read_picture(path)?;
    if w == 0 || h == 0 {
        return Err("the picture is empty".into());
    }
    let mut width = width.clamp(12, 120);
    let blocks = chars == "blocks";
    let per = if blocks { 2 } else { 1 };
    let mut out_rows = ((width as f64 * h as f64 / w as f64 * 0.5).round_ties_even() as usize).max(1);
    if out_rows > 40 {
        width = ((width as f64 * 40.0 / out_rows as f64).round_ties_even() as usize).max(4);
        out_rows = 40;
    }
    let mut bg = None;
    if !keep_bg {
        // on a 4x bigger copy, so the background's blend into the edges goes with it
        let (mid_w, mid_h) = (w.min(width * 4), h.min(out_rows * per * 4));
        let (r, b) = drop_background(shrink(w, h, &rows, mid_w, mid_h), 40);
        rows = r;
        bg = b;
        w = mid_w;
        h = mid_h;
    }
    let mut px = shrink(w, h, &rows, width, out_rows * per);
    let ph = px.len();
    // specks of up to 3 pixels apart from the rest (sparkles, noise) go
    let on = |px: &Vec<Vec<Px>>, x: i64, y: i64| {
        x >= 0 && y >= 0 && (x as usize) < width && (y as usize) < ph && px[y as usize][x as usize][3] >= 128
    };
    let mut seen: HashSet<(i64, i64)> = HashSet::new();
    for y0 in 0..ph as i64 {
        for x0 in 0..width as i64 {
            if seen.contains(&(x0, y0)) || !on(&px, x0, y0) {
                continue;
            }
            let mut part = Vec::new();
            let mut todo = vec![(x0, y0)];
            seen.insert((x0, y0));
            while let Some((x, y)) = todo.pop() {
                part.push((x, y));
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        let n = (x + dx, y + dy);
                        if !seen.contains(&n) && on(&px, n.0, n.1) {
                            seen.insert(n);
                            todo.push(n);
                        }
                    }
                }
            }
            if part.len() <= 3 {
                for (x, y) in part {
                    px[y as usize][x as usize] = [0, 0, 0, 0];
                }
            }
        }
    }
    let solid: Vec<Px> = px.iter().flatten().filter(|p| p[3] >= 128).copied().collect();
    let pal = quantize(&solid, colors.clamp(1, 9), if keep_bg { None } else { bg });
    if pal.is_empty() {
        return Err("the picture is all transparent".into());
    }
    let mut nearest: HashMap<Rgb, usize> = HashMap::new();
    let mut idx = |p: &Px| -> Option<usize> {
        if p[3] < 128 {
            return None;
        }
        let key = [p[0], p[1], p[2]];
        Some(*nearest.entry(key).or_insert_with(|| {
            let mut best = 0;
            for i in 1..pal.len() {
                if d2(&key, &pal[i]) < d2(&key, &pal[best]) {
                    best = i;
                }
            }
            best
        }))
    };
    // each color: the average of its own pixels, not of the bins
    let mut sums = vec![[0i64; 4]; pal.len()];
    for p in &solid {
        if let Some(i) = idx(p) {
            let s = &mut sums[i];
            s[0] += p[0];
            s[1] += p[1];
            s[2] += p[2];
            s[3] += 1;
        }
    }
    let fin: Vec<Rgb> =
        sums.iter().zip(&pal).map(|(s, c)| if s[3] != 0 { [s[0] / s[3], s[1] / s[3], s[2] / s[3]] } else { *c }).collect();
    let mut used = vec![0usize; pal.len()];
    enum Cell {
        Pair(Option<usize>, Option<usize>),
        One(Option<usize>, Px),
    }
    let mut cells: Vec<Vec<Cell>> = Vec::new();
    for y in 0..out_rows {
        let mut line = Vec::new();
        for x in 0..width {
            if blocks {
                let (a, b) = (idx(&px[y * 2][x]), idx(&px[y * 2 + 1][x]));
                for i in [a, b].into_iter().flatten() {
                    used[i] += 1;
                }
                line.push(Cell::Pair(a, b));
            } else {
                let i = idx(&px[y][x]);
                if let Some(i) = i {
                    used[i] += 1;
                }
                line.push(Cell::One(i, px[y][x]));
            }
        }
        cells.push(line);
    }
    let ramp: Vec<char> = " .:-=+*#%@".chars().collect();
    let mut text_rows: Vec<String> = Vec::new();
    let mut cur: Option<usize> = None;
    for line in &cells {
        let mut s = String::new();
        for cell in line {
            let (i, ch) = match *cell {
                Cell::Pair(None, None) => {
                    s.push(' ');
                    continue;
                }
                Cell::Pair(mut a, mut b) => {
                    if let (Some(x), Some(y)) = (a, b)
                        && x != y
                    {
                        if used[x] <= used[y] { b = None } else { a = None }
                    }
                    match (a, b) {
                        (Some(x), Some(y)) if x == y => (x, '█'),
                        (Some(x), _) => (x, '▀'),
                        (None, Some(y)) => (y, '▄'),
                        (None, None) => unreachable!(),
                    }
                }
                Cell::One(None, _) => {
                    s.push(' ');
                    continue;
                }
                Cell::One(Some(i), p) => {
                    let lum = (0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64) / 255.0;
                    let ch = ramp[(ramp.len() - 1).min((lum * ramp.len() as f64) as usize)];
                    (i, if ch == ' ' { '.' } else { ch })
                }
            };
            if Some(i) != cur {
                s += &format!("${}", i + 1);
                cur = Some(i);
            }
            s.push(ch);
        }
        text_rows.push(s.trim_end().to_string());
    }
    while text_rows.last().is_some_and(|r| r.trim().is_empty()) {
        text_rows.pop();
    }
    while text_rows.first().is_some_and(|r| r.trim().is_empty()) {
        text_rows.remove(0);
    }
    let plain = |r: &str| -> String {
        let c: Vec<char> = r.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < c.len() {
            if c[i] == '$' && i + 1 < c.len() && c[i + 1].is_ascii_digit() {
                i += 2;
                continue;
            }
            out.push(c[i]);
            i += 1;
        }
        out
    };
    let margin = text_rows
        .iter()
        .map(|r| plain(r))
        .filter(|p| !p.trim().is_empty())
        .map(|p| p.chars().count() - p.trim_start().chars().count())
        .min()
        .unwrap_or(0);
    if margin > 0 {
        // a common left margin out; the color switches before it move along
        text_rows = text_rows
            .into_iter()
            .map(|r| {
                let c: Vec<char> = r.chars().collect();
                let (mut out, mut cut, mut i) = (String::new(), margin, 0);
                while i < c.len() {
                    if c[i] == '$' && i + 1 < c.len() && c[i + 1].is_ascii_digit() {
                        out.push(c[i]);
                        out.push(c[i + 1]);
                        i += 2;
                    } else if cut > 0 && c[i] == ' ' {
                        cut -= 1;
                        i += 1;
                    } else {
                        out.extend(&c[i..]);
                        break;
                    }
                }
                out
            })
            .collect();
    }
    let cols = fin.iter().map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])).collect();
    Ok((text_rows.join("\n"), cols))
}
