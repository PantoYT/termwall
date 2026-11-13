//! A QR code encoder (byte mode, error correction M, versions 1-10): enough for a link.

/// version: (EC codewords per block, data codewords of each block)
fn qr_m(ver: usize) -> (usize, &'static [usize]) {
    match ver {
        1 => (10, &[16]),
        2 => (16, &[28]),
        3 => (26, &[44]),
        4 => (18, &[32, 32]),
        5 => (24, &[43, 43]),
        6 => (16, &[27, 27, 27, 27]),
        7 => (18, &[31, 31, 31, 31]),
        8 => (22, &[38, 38, 39, 39]),
        9 => (22, &[36, 36, 36, 37, 37]),
        _ => (26, &[43, 43, 43, 43, 44]),
    }
}

fn qr_align(ver: usize) -> &'static [usize] {
    match ver {
        1 => &[],
        2 => &[6, 18],
        3 => &[6, 22],
        4 => &[6, 26],
        5 => &[6, 30],
        6 => &[6, 34],
        7 => &[6, 22, 38],
        8 => &[6, 24, 42],
        9 => &[6, 26, 46],
        _ => &[6, 28, 50],
    }
}

fn gf_mul(x: u32, y: u32) -> u32 {
    let mut z: u32 = 0;
    for i in (0..8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x11D);
        z ^= ((y >> i) & 1) * x;
    }
    z
}

fn rs_ec(data: &[u32], degree: usize) -> Vec<u32> {
    let mut div = vec![0u32; degree - 1];
    div.push(1);
    let mut root = 1;
    for _ in 0..degree {
        for j in 0..degree {
            div[j] = gf_mul(div[j], root);
            if j + 1 < degree {
                div[j] ^= div[j + 1];
            }
        }
        root = gf_mul(root, 2);
    }
    let mut rem = vec![0u32; degree];
    for &b in data {
        let factor = b ^ rem.remove(0);
        rem.push(0);
        for (i, c) in div.iter().enumerate() {
            rem[i] ^= gf_mul(*c, factor);
        }
    }
    rem
}

/// The modules of a QR code for text (true = dark), without the quiet zone.
pub fn qr_matrix(text: &str) -> Result<Vec<Vec<bool>>, String> {
    let data = text.as_bytes();
    let ver = (1..=10)
        .find(|&v| {
            let cap: usize = qr_m(v).1.iter().sum();
            let cc_bits = if v < 10 { 8 } else { 16 };
            4 + cc_bits + 8 * data.len() <= cap * 8
        })
        .ok_or("too long for a QR code here")?;
    let (ec_len, blocks) = qr_m(ver);
    let cap: usize = blocks.iter().sum();
    let cc_bits = if ver < 10 { 8 } else { 16 };
    let mut bits: Vec<u32> = vec![0, 1, 0, 0];
    bits.extend((0..cc_bits).rev().map(|i| ((data.len() >> i) & 1) as u32));
    for &b in data {
        bits.extend((0..8).rev().map(|i| u32::from((b >> i) & 1)));
    }
    let term = 4.min(cap * 8 - bits.len());
    bits.extend(std::iter::repeat_n(0, term));
    let pad_bits = (8 - bits.len() % 8) % 8;
    bits.extend(std::iter::repeat_n(0, pad_bits));
    let mut words: Vec<u32> = bits.chunks(8).map(|c| c.iter().fold(0, |acc, b| acc << 1 | b)).collect();
    let mut pad = 0;
    while words.len() < cap {
        words.push(if pad % 2 == 0 { 0xEC } else { 0x11 });
        pad += 1;
    }
    let mut split = Vec::new();
    let mut at = 0;
    for &n in blocks {
        split.push(words[at..at + n].to_vec());
        at += n;
    }
    let ecs: Vec<Vec<u32>> = split.iter().map(|b| rs_ec(b, ec_len)).collect();
    let mut fin: Vec<u32> = Vec::new();
    let longest = blocks.iter().copied().max().unwrap_or(0);
    for i in 0..longest {
        for b in &split {
            if i < b.len() {
                fin.push(b[i]);
            }
        }
    }
    for i in 0..ec_len {
        for e in &ecs {
            fin.push(e[i]);
        }
    }

    let size = ver * 4 + 17;
    let mut q = Qr { size, m: vec![vec![false; size]; size], f: vec![vec![false; size]; size] };
    for i in 0..size {
        q.setf(6, i, i % 2 == 0);
        q.setf(i, 6, i % 2 == 0);
    }
    for (cx, cy) in [(3i64, 3i64), (size as i64 - 4, 3), (3, size as i64 - 4)] {
        for dy in -4i64..=4 {
            for dx in -4i64..=4 {
                let (x, y) = (cx + dx, cy + dy);
                if 0 <= x && x < size as i64 && 0 <= y && y < size as i64 {
                    let d = dx.abs().max(dy.abs());
                    q.setf(x as usize, y as usize, d != 2 && d != 4);
                }
            }
        }
    }
    let al = qr_align(ver);
    for (i, &ax) in al.iter().enumerate() {
        for (j, &ay) in al.iter().enumerate() {
            let last = al.len() - 1;
            if (i, j) == (0, 0) || (i, j) == (0, last) || (i, j) == (last, 0) {
                continue;
            }
            for dy in -2i64..=2 {
                for dx in -2i64..=2 {
                    q.setf((ax as i64 + dx) as usize, (ay as i64 + dy) as usize, dx.abs().max(dy.abs()) != 1);
                }
            }
        }
    }
    q.draw_format(0);
    if ver >= 7 {
        let mut rem = ver as u32;
        for _ in 0..12 {
            rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
        }
        let vb = (ver as u32) << 12 | rem;
        for i in 0..18 {
            let b = (vb >> i) & 1 == 1;
            let (a, c) = (size - 11 + i % 3, i / 3);
            q.setf(a, c, b);
            q.setf(c, a, b);
        }
    }

    let mut i = 0usize;
    let nbits = fin.len() * 8;
    let mut right = size as i64 - 1;
    while right >= 1 {
        if right == 6 {
            right = 5;
        }
        for vert in 0..size {
            for j in 0..2 {
                let x = (right - j) as usize;
                let y = if ((right + 1) & 2) == 0 { size - 1 - vert } else { vert };
                if !q.f[y][x] && i < nbits {
                    q.m[y][x] = (fin[i >> 3] >> (7 - (i & 7))) & 1 == 1;
                    i += 1;
                }
            }
        }
        right -= 2;
    }

    let mut best: Option<(i64, u32)> = None;
    for m in 0..8 {
        q.apply(m);
        q.draw_format(m);
        let score = q.penalty();
        if best.is_none_or(|(s, _)| score < s) {
            best = Some((score, m));
        }
        q.apply(m); // undo (the mask is its own inverse)
    }
    let m = best.map(|b| b.1).unwrap_or(0);
    q.apply(m);
    q.draw_format(m);
    Ok(q.m)
}

struct Qr {
    size: usize,
    m: Vec<Vec<bool>>,
    f: Vec<Vec<bool>>,
}

impl Qr {
    fn setf(&mut self, x: usize, y: usize, dark: bool) {
        self.m[y][x] = dark;
        self.f[y][x] = true;
    }

    fn draw_format(&mut self, mask: u32) {
        let d = mask; // error correction M = 00
        let mut rem = d;
        for _ in 0..10 {
            rem = (rem << 1) ^ ((rem >> 9) * 0x537);
        }
        let fb = (d << 10 | rem) ^ 0x5412;
        let bit = |i: usize| (fb >> i) & 1 == 1;
        let size = self.size;
        for i in 0..6 {
            self.setf(8, i, bit(i));
        }
        self.setf(8, 7, bit(6));
        self.setf(8, 8, bit(7));
        self.setf(7, 8, bit(8));
        for i in 9..15 {
            self.setf(14 - i, 8, bit(i));
        }
        for i in 0..8 {
            self.setf(size - 1 - i, 8, bit(i));
        }
        for i in 8..15 {
            self.setf(8, size - 15 + i, bit(i));
        }
        self.setf(8, size - 8, true);
    }

    fn apply(&mut self, m: u32) {
        let mask = |x: usize, y: usize| -> bool {
            match m {
                0 => (x + y).is_multiple_of(2),
                1 => y.is_multiple_of(2),
                2 => x.is_multiple_of(3),
                3 => (x + y).is_multiple_of(3),
                4 => (x / 3 + y / 2).is_multiple_of(2),
                5 => x * y % 2 + x * y % 3 == 0,
                6 => (x * y % 2 + x * y % 3).is_multiple_of(2),
                _ => ((x + y) % 2 + x * y % 3).is_multiple_of(2),
            }
        };
        for y in 0..self.size {
            for x in 0..self.size {
                if !self.f[y][x] && mask(x, y) {
                    self.m[y][x] = !self.m[y][x];
                }
            }
        }
    }

    fn penalty(&self) -> i64 {
        let size = self.size;
        let mut p = 0i64;
        let mut lines: Vec<Vec<bool>> = self.m.clone();
        lines.extend((0..size).map(|x| (0..size).map(|y| self.m[y][x]).collect()));
        for line in &lines {
            let mut run = 0;
            let mut prev: Option<bool> = None;
            for &v in line {
                run = if Some(v) == prev { run + 1 } else { 1 };
                prev = Some(v);
                if run == 5 {
                    p += 3;
                } else if run > 5 {
                    p += 1;
                }
            }
            let s: String = line.iter().map(|&v| if v { '1' } else { '0' }).collect();
            p += 40 * (s.matches("10111010000").count() + s.matches("00001011101").count()) as i64;
        }
        for y in 0..size - 1 {
            for x in 0..size - 1 {
                let v = self.m[y][x];
                if v == self.m[y][x + 1] && v == self.m[y + 1][x] && v == self.m[y + 1][x + 1] {
                    p += 3;
                }
            }
        }
        let dark = self.m.iter().flatten().filter(|&&v| v).count() as i64;
        let n = (size * size) as i64;
        p += 10 * ((dark * 20 - n * 10).abs() / n);
        p
    }
}

/// The QR code for a terminal: two rows per line with half blocks, dark on light, with a quiet
/// zone. Black and white are forced so a dark terminal theme can't invert it.
pub fn qr_text(matrix: &[Vec<bool>]) -> String {
    let q = 2i64;
    let size = matrix.len() as i64;
    let get = |x: i64, y: i64| 0 <= x && x < size && 0 <= y && y < size && matrix[y as usize][x as usize];
    let mut lines = Vec::new();
    let mut y = -q;
    while y < size + q {
        let row: String = (-q..size + q)
            .map(|x| match (get(x, y), get(x, y + 1)) {
                (false, false) => ' ',
                (true, false) => '▀',
                (false, true) => '▄',
                (true, true) => '█',
            })
            .collect();
        lines.push(format!("\x1b[30;107m{row}\x1b[0m"));
        y += 2;
    }
    lines.join("\n")
}
