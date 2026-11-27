//! Multi-operator fixed-point (`1000`-scaled) & rational expression evaluator (`+ - * / ^ () sqrt`).

pub fn eval_expr(expr: &str) -> Result<i64, &'static str> {
    let mut p = Parser {
        src: expr.as_bytes(),
        pos: 0,
    };
    let val = p.parse_add_sub()?;
    p.skip_ws();
    if p.pos != p.src.len() {
        return Err("Syntax error");
    }
    Ok(val)
}

pub fn format_decimal(scaled: i64, out: &mut [u8]) -> usize {
    let neg = scaled < 0;
    let abs = scaled.unsigned_abs();
    let whole = (abs / 1000) as u32;
    let frac = (abs % 1000) as u32;

    let mut idx = 0usize;
    let mut push = |b: u8| {
        if idx < out.len() {
            out[idx] = b;
            idx += 1;
        }
    };
    if neg {
        push(b'-');
    }
    let mut w_buf = [0u8; 10];
    let w_len = fmt_u32(whole, &mut w_buf);
    for &b in &w_buf[..w_len] {
        push(b);
    }
    if frac != 0 {
        push(b'.');
        push(b'0' + (frac / 100) as u8);
        let d2 = (frac / 10) % 10;
        let d3 = frac % 10;
        if d2 != 0 || d3 != 0 {
            push(b'0' + d2 as u8);
        }
        if d3 != 0 {
            push(b'0' + d3 as u8);
        }
    }
    idx
}

pub fn format_fraction(scaled: i64, out: &mut [u8]) -> usize {
    let neg = scaled < 0;
    let abs = scaled.unsigned_abs();
    let g = gcd(abs, 1000);
    let num = (abs / g) as u32;
    let den = (1000 / g) as u32;

    let mut idx = 0usize;
    let mut push = |b: u8| {
        if idx < out.len() {
            out[idx] = b;
            idx += 1;
        }
    };
    if neg {
        push(b'-');
    }
    let mut tmp = [0u8; 10];
    let nlen = fmt_u32(num, &mut tmp);
    for &b in &tmp[..nlen] {
        push(b);
    }
    if den > 1 {
        push(b'/');
        let dlen = fmt_u32(den, &mut tmp);
        for &b in &tmp[..dlen] {
            push(b);
        }
    }
    idx
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.src.len() && self.src[self.pos] == b' ' {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.src.get(self.pos).copied()
    }

    fn parse_add_sub(&mut self) -> Result<i64, &'static str> {
        let mut lhs = self.parse_mul_div()?;
        while let Some(op) = self.peek() {
            if op == b'+' {
                self.pos += 1;
                lhs = lhs.saturating_add(self.parse_mul_div()?);
            } else if op == b'-' {
                self.pos += 1;
                lhs = lhs.saturating_sub(self.parse_mul_div()?);
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_mul_div(&mut self) -> Result<i64, &'static str> {
        let mut lhs = self.parse_pow()?;
        while let Some(op) = self.peek() {
            if op == b'*' {
                self.pos += 1;
                let rhs = self.parse_pow()?;
                lhs = (lhs.saturating_mul(rhs)) / 1000;
            } else if op == b'/' {
                self.pos += 1;
                let rhs = self.parse_pow()?;
                if rhs == 0 {
                    return Err("Div by 0");
                }
                lhs = (lhs.saturating_mul(1000)) / rhs;
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_pow(&mut self) -> Result<i64, &'static str> {
        let base = self.parse_unary()?;
        if self.peek() == Some(b'^') {
            self.pos += 1;
            let exp_scaled = self.parse_pow()?;
            let exp = (exp_scaled / 1000).clamp(0, 12);
            let mut acc = 1000i64;
            for _ in 0..exp {
                acc = acc.saturating_mul(base) / 1000;
            }
            return Ok(acc);
        }
        Ok(base)
    }

    fn parse_unary(&mut self) -> Result<i64, &'static str> {
        match self.peek() {
            Some(b'-') => {
                self.pos += 1;
                Ok(-self.parse_unary()?)
            }
            Some(b'+') => {
                self.pos += 1;
                self.parse_unary()
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<i64, &'static str> {
        self.skip_ws();
        if self.src[self.pos..].starts_with(b"sqrt(") {
            self.pos += 5;
            let inner = self.parse_add_sub()?;
            if self.peek() != Some(b')') {
                return Err("Missing )");
            }
            self.pos += 1;
            if inner < 0 {
                return Err("Neg sqrt");
            }
            // sqrt(scaled * 1000) keeps 1000x scaling accurate.
            return Ok(isqrt(inner as u64 * 1000) as i64);
        }
        if self.peek() == Some(b'(') {
            self.pos += 1;
            let v = self.parse_add_sub()?;
            if self.peek() != Some(b')') {
                return Err("Missing )");
            }
            self.pos += 1;
            return Ok(v);
        }

        let start = self.pos;
        let mut whole = 0i64;
        let mut frac = 0i64;
        let mut frac_div = 1i64;
        let mut seen_dot = false;
        while self.pos < self.src.len() {
            let b = self.src[self.pos];
            if b.is_ascii_digit() {
                let d = i64::from(b - b'0');
                if !seen_dot {
                    whole = whole.saturating_mul(10).saturating_add(d);
                } else if frac_div < 1000 {
                    frac = frac * 10 + d;
                    frac_div *= 10;
                }
                self.pos += 1;
            } else if b == b'.' && !seen_dot {
                seen_dot = true;
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err("Expected num");
        }
        Ok(whole * 1000 + frac * (1000 / frac_div))
    }
}

fn isqrt(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a.max(1)
}

fn fmt_u32(mut v: u32, out: &mut [u8; 10]) -> usize {
    if v == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut rev = [0u8; 10];
    let mut n = 0usize;
    while v > 0 && n < 10 {
        rev[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    for i in 0..n {
        out[i] = rev[n - 1 - i];
    }
    n
}
