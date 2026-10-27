//! Template language for headers, footers, watermarks and stamps.
//!
//! `{expr}` / `{expr|filter}` print a value, `{if c}…{elif c}…{else}…{end}` branch,
//! `{for x in list sep ", "}…{end}` loops, `{set x = expr}` assigns, `{br}` breaks the line,
//! `{# … }` is a comment and `{{` / `}}` are literal braces.

use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Num(f64),
    Str(String),
    List(Vec<Value>),
}

impl Value {
    pub fn truthy(&self) -> bool {
        match self {
            Value::Num(n) => *n != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::List(l) => !l.is_empty(),
        }
    }

    fn as_num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            Value::Str(s) => s.trim().parse().ok(),
            Value::List(_) => None,
        }
    }

    fn num(&self) -> f64 {
        match self {
            Value::List(l) => l.len() as f64,
            v => v.as_num().unwrap_or(0.0),
        }
    }

    pub fn text(&self) -> String {
        match self {
            Value::Num(n) => fmt_num(*n),
            Value::Str(s) => s.clone(),
            Value::List(l) => l.iter().map(|v| v.text()).collect::<Vec<_>>().join(", "),
        }
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Num(if b { 1.0 } else { 0.0 })
    }
}
impl From<usize> for Value {
    fn from(n: usize) -> Self {
        Value::Num(n as f64)
    }
}
impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.to_string())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(s)
    }
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        let s = format!("{n:.6}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

pub type Vars = HashMap<String, Value>;

// ---------------------------------------------------------------- clock

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Clock {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    /// 0 = Sunday
    pub weekday: u16,
}

impl Clock {
    pub fn now() -> Self {
        #[repr(C)]
        #[derive(Default)]
        struct SystemTime {
            year: u16,
            month: u16,
            dow: u16,
            day: u16,
            hour: u16,
            minute: u16,
            second: u16,
            ms: u16,
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GetLocalTime(st: *mut SystemTime);
        }
        let mut st = SystemTime::default();
        unsafe { GetLocalTime(&mut st) };
        Self { year: st.year, month: st.month, day: st.day, hour: st.hour, minute: st.minute, second: st.second, weekday: st.dow }
    }

    pub fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    pub fn time(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    pub fn weekday_name(&self) -> &'static str {
        ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"][(self.weekday % 7) as usize]
    }

    pub fn month_name(&self) -> &'static str {
        const M: [&str; 12] = [
            "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December",
        ];
        M[(self.month.clamp(1, 12) - 1) as usize]
    }
}

// ---------------------------------------------------------------- lexer

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Id(String),
    Op(&'static str),
}

const OPS: [&str; 24] = [
    "..", "==", "!=", "<>", "<=", ">=", "&&", "||", "+", "-", "*", "/", "%", "<", ">", "=", "!", "(", ")", "[", "]", ",", "|", ":",
];

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let c: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch.is_ascii_digit() {
            let st = i;
            while i < c.len() && c[i].is_ascii_digit() {
                i += 1;
            }
            if i + 1 < c.len() && c[i] == '.' && c[i + 1].is_ascii_digit() {
                i += 1;
                while i < c.len() && c[i].is_ascii_digit() {
                    i += 1;
                }
            }
            out.push(Tok::Num(c[st..i].iter().collect::<String>().parse().map_err(|_| "bad number")?));
        } else if ch == '"' || ch == '\'' {
            i += 1;
            let mut s = String::new();
            loop {
                let Some(&d) = c.get(i) else { return Err("text in quotes is not closed".into()) };
                i += 1;
                if d == ch {
                    break;
                }
                if d == '\\' && i < c.len() {
                    s.push(match c[i] {
                        'n' => '\n',
                        't' => '\t',
                        o => o,
                    });
                    i += 1;
                } else {
                    s.push(d);
                }
            }
            out.push(Tok::Str(s));
        } else if ch.is_alphabetic() || ch == '_' {
            let st = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            out.push(Tok::Id(c[st..i].iter().collect::<String>().to_lowercase()));
        } else if let Some(op) = OPS.iter().find(|op| op.chars().enumerate().all(|(k, oc)| c.get(i + k) == Some(&oc))) {
            out.push(Tok::Op(op));
            i += op.len();
        } else {
            return Err(format!("unexpected character '{ch}'"));
        }
    }
    Ok(out)
}

fn tok_text(t: &Tok) -> String {
    match t {
        Tok::Num(n) => fmt_num(*n),
        Tok::Str(s) => format!("\"{s}\""),
        Tok::Id(s) => s.clone(),
        Tok::Op(o) => o.to_string(),
    }
}

// ---------------------------------------------------------------- expressions

#[derive(Clone, Debug)]
enum Expr {
    Lit(Value),
    Var(String),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(&'static str, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    List(Vec<Expr>),
}

struct Parser {
    t: Vec<Tok>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }

    fn eat_op(&mut self, op: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Op(o)) if *o == op) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn eat_id(&mut self, id: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Id(s)) if s == id) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expect_op(&mut self, op: &str) -> Result<(), String> {
        if self.eat_op(op) {
            Ok(())
        } else {
            Err(match self.peek() {
                Some(t) => format!("expected '{op}' but found '{}'", tok_text(t)),
                None => format!("expected '{op}'"),
            })
        }
    }

    fn expr(&mut self) -> Result<Expr, String> {
        let mut l = self.and()?;
        while self.eat_id("or") || self.eat_op("||") {
            l = Expr::Bin("or", Box::new(l), Box::new(self.and()?));
        }
        Ok(l)
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut l = self.not()?;
        while self.eat_id("and") || self.eat_op("&&") {
            l = Expr::Bin("and", Box::new(l), Box::new(self.not()?));
        }
        Ok(l)
    }

    fn not(&mut self) -> Result<Expr, String> {
        if self.eat_id("not") || self.eat_op("!") {
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.cmp()
    }

    fn cmp(&mut self) -> Result<Expr, String> {
        let l = self.range()?;
        let op = match self.peek() {
            Some(Tok::Op(o)) if ["=", "==", "!=", "<>", "<", "<=", ">", ">="].contains(o) => match *o {
                "==" => "=",
                "<>" => "!=",
                o => o,
            },
            Some(Tok::Id(s)) => match s.as_str() {
                "contains" => "contains",
                "startswith" => "startswith",
                "endswith" => "endswith",
                "in" => "in",
                _ => return Ok(l),
            },
            _ => return Ok(l),
        };
        self.i += 1;
        Ok(Expr::Bin(op, Box::new(l), Box::new(self.range()?)))
    }

    fn range(&mut self) -> Result<Expr, String> {
        let l = self.add()?;
        if self.eat_op("..") {
            return Ok(Expr::Bin("..", Box::new(l), Box::new(self.add()?)));
        }
        Ok(l)
    }

    fn add(&mut self) -> Result<Expr, String> {
        let mut l = self.mul()?;
        loop {
            let op = if self.eat_op("+") {
                "+"
            } else if self.eat_op("-") {
                "-"
            } else {
                return Ok(l);
            };
            l = Expr::Bin(op, Box::new(l), Box::new(self.mul()?));
        }
    }

    fn mul(&mut self) -> Result<Expr, String> {
        let mut l = self.unary()?;
        loop {
            let op = if self.eat_op("*") {
                "*"
            } else if self.eat_op("/") {
                "/"
            } else if self.eat_op("%") {
                "%"
            } else {
                return Ok(l);
            };
            l = Expr::Bin(op, Box::new(l), Box::new(self.unary()?));
        }
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.eat_op("-") {
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        self.postfix()
    }

    /// `atom | filter | filter:arg | filter(arg, …)`
    fn postfix(&mut self) -> Result<Expr, String> {
        let mut e = self.atom()?;
        while self.eat_op("|") {
            let Some(Tok::Id(name)) = self.peek().cloned() else { return Err("expected a filter name after '|'".into()) };
            self.i += 1;
            let mut args = vec![e];
            if self.eat_op(":") {
                args.push(self.unary()?);
            } else if self.eat_op("(") {
                args.extend(self.args(")")?);
            }
            e = Expr::Call(name, args);
        }
        Ok(e)
    }

    fn args(&mut self, close: &str) -> Result<Vec<Expr>, String> {
        let mut v = Vec::new();
        if self.eat_op(close) {
            return Ok(v);
        }
        loop {
            v.push(self.expr()?);
            if self.eat_op(close) {
                return Ok(v);
            }
            self.expect_op(",")?;
        }
    }

    fn atom(&mut self) -> Result<Expr, String> {
        let Some(t) = self.peek().cloned() else { return Err("expression is incomplete".into()) };
        self.i += 1;
        match t {
            Tok::Num(n) => Ok(Expr::Lit(Value::Num(n))),
            Tok::Str(s) => Ok(Expr::Lit(Value::Str(s))),
            Tok::Id(id) => {
                if self.eat_op("(") {
                    Ok(Expr::Call(id, self.args(")")?))
                } else {
                    Ok(match id.as_str() {
                        "true" | "yes" => Expr::Lit(Value::Num(1.0)),
                        "false" | "no" => Expr::Lit(Value::Num(0.0)),
                        _ => Expr::Var(id),
                    })
                }
            }
            Tok::Op("(") => {
                let e = self.expr()?;
                self.expect_op(")")?;
                Ok(e)
            }
            Tok::Op("[") => Ok(Expr::List(self.args("]")?)),
            t => Err(format!("unexpected '{}'", tok_text(&t))),
        }
    }

    fn done(&self) -> Result<(), String> {
        match self.peek() {
            None => Ok(()),
            Some(t) => Err(format!("unexpected '{}'", tok_text(t))),
        }
    }
}

fn parse_expr(src: &str) -> Result<Expr, String> {
    let mut p = Parser { t: lex(src)?, i: 0 };
    if p.t.is_empty() {
        return Err("empty expression".into());
    }
    let e = p.expr()?;
    p.done()?;
    Ok(e)
}

// ---------------------------------------------------------------- functions

fn roman(n: i64) -> String {
    if !(1..4000).contains(&n) {
        return n.to_string();
    }
    let table = [
        (1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"),
        (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I"),
    ];
    let mut n = n;
    let mut s = String::new();
    for (v, r) in table {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

fn alpha(n: i64) -> String {
    let mut n = n;
    let mut s = Vec::new();
    while n > 0 {
        n -= 1;
        s.push((b'A' + (n % 26) as u8) as char);
        n /= 26;
    }
    s.iter().rev().collect()
}

fn words(n: i64) -> String {
    const ONES: [&str; 20] = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve",
        "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen",
    ];
    const TENS: [&str; 10] = ["", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];
    if n < 0 {
        return format!("minus {}", words(-n));
    }
    if n < 20 {
        return ONES[n as usize].into();
    }
    if n < 100 {
        let t = TENS[(n / 10) as usize];
        return if n % 10 == 0 { t.into() } else { format!("{t}-{}", ONES[(n % 10) as usize]) };
    }
    for (unit, name) in [(1_000_000_000, "billion"), (1_000_000, "million"), (1_000, "thousand"), (100, "hundred")] {
        if n >= unit {
            let rest = n % unit;
            let head = format!("{} {name}", words(n / unit));
            return if rest == 0 { head } else { format!("{head} {}", words(rest)) };
        }
    }
    n.to_string()
}

fn ordinal(n: i64) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut start = true;
    for c in s.chars() {
        if start {
            out.extend(c.to_uppercase());
        } else {
            out.extend(c.to_lowercase());
        }
        start = !c.is_alphanumeric();
    }
    out
}

fn call(name: &str, a: Vec<Value>) -> Result<Value, String> {
    let need = |n: usize| -> Result<(), String> {
        if a.len() < n {
            Err(format!("{name}() needs {n} value(s)"))
        } else {
            Ok(())
        }
    };
    let s0 = || a.first().map(|v| v.text()).unwrap_or_default();
    let n0 = || a.first().map(|v| v.num()).unwrap_or(0.0);
    let n1 = |d: f64| a.get(1).map(|v| v.num()).unwrap_or(d);
    Ok(match name {
        "upper" => Value::Str(s0().to_uppercase()),
        "lower" => Value::Str(s0().to_lowercase()),
        "title" => Value::Str(title_case(&s0())),
        "trim" => Value::Str(s0().trim().into()),
        "len" | "length" | "count" => Value::Num(match a.first() {
            Some(Value::List(l)) => l.len() as f64,
            _ => s0().chars().count() as f64,
        }),
        "roman" => Value::Str(roman(n0().round() as i64)),
        "alpha" | "letter" => Value::Str(alpha(n0().round() as i64)),
        "words" => Value::Str(words(n0().round() as i64)),
        "ordinal" => Value::Str(ordinal(n0().round() as i64)),
        "pad" => {
            need(2)?;
            let w = n1(0.0).clamp(0.0, 64.0) as usize;
            let s = s0();
            let (sign, digits) = match s.strip_prefix('-') {
                Some(d) => ("-", d.to_string()),
                None => ("", s),
            };
            Value::Str(format!("{sign}{digits:0>w$}"))
        }
        "fixed" => Value::Str(format!("{:.*}", n1(2.0).clamp(0.0, 10.0) as usize, n0())),
        "left" => Value::Str(s0().chars().take(n1(0.0).max(0.0) as usize).collect()),
        "right" => {
            let s: Vec<char> = s0().chars().collect();
            let k = (n1(0.0).max(0.0) as usize).min(s.len());
            Value::Str(s[s.len() - k..].iter().collect())
        }
        "replace" => {
            need(3)?;
            Value::Str(s0().replace(&a[1].text(), &a[2].text()))
        }
        "default" => {
            need(2)?;
            if a[0].truthy() { a[0].clone() } else { a[1].clone() }
        }
        "join" => {
            let sep = a.get(1).map(|v| v.text()).unwrap_or_else(|| ", ".into());
            match a.first() {
                Some(Value::List(l)) => Value::Str(l.iter().map(|v| v.text()).collect::<Vec<_>>().join(&sep)),
                _ => Value::Str(s0()),
            }
        }
        "min" | "max" => {
            need(1)?;
            let mut items: Vec<f64> = Vec::new();
            for v in &a {
                match v {
                    Value::List(l) => items.extend(l.iter().map(|x| x.num())),
                    x => items.push(x.num()),
                }
            }
            let f = if name == "min" { f64::min } else { f64::max };
            Value::Num(items.into_iter().reduce(f).unwrap_or(0.0))
        }
        "abs" => Value::Num(n0().abs()),
        "round" => {
            let k = 10f64.powi(n1(0.0).clamp(0.0, 10.0) as i32);
            Value::Num((n0() * k).round() / k)
        }
        "floor" => Value::Num(n0().floor()),
        "ceil" => Value::Num(n0().ceil()),
        "iif" => {
            need(3)?;
            if a[0].truthy() { a[1].clone() } else { a[2].clone() }
        }
        "repeat" => {
            need(2)?;
            Value::Str(s0().repeat(n1(0.0).clamp(0.0, 1000.0) as usize))
        }
        _ => return Err(format!("unknown function or filter '{name}'")),
    })
}

fn compare(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a.as_num(), b.as_num()) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.text().to_lowercase().cmp(&b.text().to_lowercase()),
    }
}

// ---------------------------------------------------------------- evaluation

const MAX_STEPS: usize = 200_000;
const MAX_OUT: usize = 20_000;
const MAX_LIST: usize = 10_000;

struct Env<'a> {
    vars: &'a Vars,
    locals: Vec<(String, Value)>,
    steps: usize,
    error: Option<String>,
}

impl Env<'_> {
    fn get(&self, name: &str) -> Option<&Value> {
        self.locals.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v).or_else(|| self.vars.get(name))
    }

    fn eval(&self, e: &Expr) -> Result<Value, String> {
        Ok(match e {
            Expr::Lit(v) => v.clone(),
            Expr::Var(n) => self.get(n).cloned().ok_or_else(|| format!("unknown variable '{n}'"))?,
            Expr::Neg(x) => Value::Num(-self.eval(x)?.num()),
            Expr::Not(x) => Value::from(!self.eval(x)?.truthy()),
            Expr::List(items) => Value::List(items.iter().map(|x| self.eval(x)).collect::<Result<_, _>>()?),
            Expr::Call(n, args) => call(n, args.iter().map(|x| self.eval(x)).collect::<Result<_, _>>()?)?,
            Expr::Bin(op, l, r) => {
                let a = self.eval(l)?;
                match *op {
                    "and" => return Ok(Value::from(a.truthy() && self.eval(r)?.truthy())),
                    "or" => return Ok(Value::from(a.truthy() || self.eval(r)?.truthy())),
                    _ => {}
                }
                let b = self.eval(r)?;
                let lower = |v: &Value| v.text().to_lowercase();
                match *op {
                    "+" => match (&a, &b, a.as_num(), b.as_num()) {
                        (Value::List(x), Value::List(y), _, _) => Value::List(x.iter().chain(y).cloned().collect()),
                        (_, _, Some(x), Some(y)) => Value::Num(x + y),
                        _ => Value::Str(a.text() + &b.text()),
                    },
                    "-" => Value::Num(a.num() - b.num()),
                    "*" => Value::Num(a.num() * b.num()),
                    "/" | "%" => {
                        let d = b.num();
                        if d == 0.0 {
                            return Err("division by zero".into());
                        }
                        Value::Num(if *op == "/" { a.num() / d } else { a.num().rem_euclid(d) })
                    }
                    "=" => Value::from(compare(&a, &b).is_eq()),
                    "!=" => Value::from(!compare(&a, &b).is_eq()),
                    "<" => Value::from(compare(&a, &b).is_lt()),
                    "<=" => Value::from(compare(&a, &b).is_le()),
                    ">" => Value::from(compare(&a, &b).is_gt()),
                    ">=" => Value::from(compare(&a, &b).is_ge()),
                    "contains" | "in" => {
                        let (hay, needle) = if *op == "in" { (&b, &a) } else { (&a, &b) };
                        Value::from(match hay {
                            Value::List(l) => l.iter().any(|v| compare(v, needle).is_eq()),
                            h => lower(h).contains(&lower(needle)),
                        })
                    }
                    "startswith" => Value::from(lower(&a).starts_with(&lower(&b))),
                    "endswith" => Value::from(lower(&a).ends_with(&lower(&b))),
                    ".." => {
                        let (x, y) = (a.num().round() as i64, b.num().round() as i64);
                        if (x - y).unsigned_abs() as usize >= MAX_LIST {
                            return Err(format!("range {x}..{y} is too long"));
                        }
                        let v: Vec<Value> = if x <= y {
                            (x..=y).map(|n| Value::Num(n as f64)).collect()
                        } else {
                            (y..=x).rev().map(|n| Value::Num(n as f64)).collect()
                        };
                        Value::List(v)
                    }
                    _ => unreachable!(),
                }
            }
        })
    }

    fn fail(&mut self, e: String) {
        if self.error.is_none() {
            self.error = Some(e);
        }
    }

    fn run(&mut self, nodes: &[Node], out: &mut String) {
        for n in nodes {
            self.steps += 1;
            if self.steps > MAX_STEPS || out.len() > MAX_OUT {
                self.fail("template produces too much output".into());
                return;
            }
            match n {
                Node::Text(t) => out.push_str(t),
                Node::Out(e) => match self.eval(e) {
                    Ok(v) => out.push_str(&v.text()),
                    Err(m) => self.fail(m),
                },
                Node::Set(name, e) => match self.eval(e) {
                    Ok(v) => self.locals.push((name.clone(), v)),
                    Err(m) => self.fail(m),
                },
                Node::If(arms, other) => {
                    let mut taken = false;
                    for (c, body) in arms {
                        match self.eval(c) {
                            Ok(v) if v.truthy() => {
                                self.run(body, out);
                                taken = true;
                                break;
                            }
                            Ok(_) => {}
                            Err(m) => self.fail(m),
                        }
                    }
                    if !taken {
                        self.run(other, out);
                    }
                }
                Node::For(var, list, sep, body) => {
                    let items = match self.eval(list) {
                        Ok(Value::List(l)) => l,
                        Ok(Value::Num(k)) => (1..=(k.max(0.0) as usize).min(MAX_LIST)).map(|i| Value::Num(i as f64)).collect(),
                        Ok(Value::Str(s)) => s.chars().map(|c| Value::Str(c.to_string())).collect(),
                        Err(m) => {
                            self.fail(m);
                            continue;
                        }
                    };
                    let sep = match sep.as_ref().map(|e| self.eval(e)) {
                        Some(Ok(v)) => v.text(),
                        Some(Err(m)) => {
                            self.fail(m);
                            String::new()
                        }
                        None => String::new(),
                    };
                    let base = self.locals.len();
                    let count = items.len();
                    for (k, it) in items.into_iter().enumerate() {
                        if k > 0 {
                            out.push_str(&sep);
                        }
                        self.locals.extend([
                            (var.clone(), it),
                            ("loop_index".into(), Value::from(k + 1)),
                            ("loop_first".into(), Value::from(k == 0)),
                            ("loop_last".into(), Value::from(k + 1 == count)),
                            ("loop_count".into(), Value::from(count)),
                        ]);
                        self.run(body, out);
                        self.locals.truncate(base);
                        if self.error.is_some() && self.steps > MAX_STEPS {
                            return;
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- template structure

#[derive(Clone, Debug)]
enum Node {
    Text(String),
    Out(Expr),
    Set(String, Expr),
    If(Vec<(Expr, Vec<Node>)>, Vec<Node>),
    For(String, Expr, Option<Expr>, Vec<Node>),
}

enum Part {
    Text(String),
    Tag(String),
}

fn split(src: &str) -> Result<Vec<Part>, String> {
    let c: Vec<char> = src.chars().collect();
    let mut parts = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '{' if c.get(i + 1) == Some(&'{') => {
                buf.push('{');
                i += 2;
            }
            '}' if c.get(i + 1) == Some(&'}') => {
                buf.push('}');
                i += 2;
            }
            '{' => {
                let mut j = i + 1;
                let mut quote = None;
                while j < c.len() {
                    match (quote, c[j]) {
                        (None, '}') => break,
                        (None, q @ ('"' | '\'')) => quote = Some(q),
                        (Some(q), d) if d == q => quote = None,
                        _ => {}
                    }
                    j += 1;
                }
                if j >= c.len() {
                    return Err("a '{' has no matching '}' (write {{ for a literal brace)".into());
                }
                let tag: String = c[i + 1..j].iter().collect();
                if !tag.trim_start().starts_with('#') {
                    if !buf.is_empty() {
                        parts.push(Part::Text(std::mem::take(&mut buf)));
                    }
                    parts.push(Part::Tag(tag.trim().to_string()));
                }
                i = j + 1;
            }
            ch => {
                buf.push(ch);
                i += 1;
            }
        }
    }
    if !buf.is_empty() {
        parts.push(Part::Text(buf));
    }
    Ok(parts)
}

#[derive(PartialEq)]
enum Stop {
    Eof,
    Elif(String),
    Else,
    End,
}

/// Splits a tag into its lower-cased keyword and the rest.
fn keyword(tag: &str) -> (String, &str) {
    let end = tag.find(|c: char| c.is_whitespace()).unwrap_or(tag.len());
    (tag[..end].to_lowercase(), tag[end..].trim())
}

fn block(parts: &[Part], i: &mut usize) -> Result<(Vec<Node>, Stop), String> {
    let mut out = Vec::new();
    while *i < parts.len() {
        let p = &parts[*i];
        *i += 1;
        let tag = match p {
            Part::Text(t) => {
                out.push(Node::Text(t.clone()));
                continue;
            }
            Part::Tag(t) => t.as_str(),
        };
        let (kw, rest) = keyword(tag);
        match kw.as_str() {
            "if" if !rest.is_empty() => {
                let mut arms = Vec::new();
                let mut cond = parse_expr(rest).map_err(|e| format!("in {{if {rest}}}: {e}"))?;
                let other = loop {
                    let (body, stop) = block(parts, i)?;
                    arms.push((cond, body));
                    match stop {
                        Stop::Elif(c) => cond = parse_expr(&c).map_err(|e| format!("in {{elif {c}}}: {e}"))?,
                        Stop::Else => {
                            let (body, stop) = block(parts, i)?;
                            if stop != Stop::End {
                                return Err("{if} … {else} is missing its {end}".into());
                            }
                            break body;
                        }
                        Stop::End => break Vec::new(),
                        Stop::Eof => return Err("{if} is missing its {end}".into()),
                    }
                };
                out.push(Node::If(arms, other));
            }
            "elif" | "elseif" => return Ok((out, Stop::Elif(rest.to_string()))),
            "else" => {
                let (kw2, rest2) = keyword(rest);
                if kw2 == "if" {
                    return Ok((out, Stop::Elif(rest2.to_string())));
                }
                if !rest.is_empty() {
                    return Err(format!("unexpected text after {{else}}: '{rest}'"));
                }
                return Ok((out, Stop::Else));
            }
            "end" | "endif" | "endfor" | "/if" | "/for" if rest.is_empty() => return Ok((out, Stop::End)),
            "for" if !rest.is_empty() => {
                let mut p = Parser { t: lex(rest)?, i: 0 };
                let Some(Tok::Id(var)) = p.peek().cloned() else { return Err("write {for NAME in LIST}".into()) };
                p.i += 1;
                if !p.eat_id("in") {
                    return Err("write {for NAME in LIST}".into());
                }
                let list = p.expr()?;
                let sep = if p.eat_id("sep") { Some(p.expr()?) } else { None };
                p.done().map_err(|e| format!("in {{for {rest}}}: {e}"))?;
                let (body, stop) = block(parts, i)?;
                if stop != Stop::End {
                    return Err(if stop == Stop::Eof { "{for} is missing its {end}" } else { "{else} / {elif} outside of an {if}" }.into());
                }
                out.push(Node::For(var, list, sep, body));
            }
            "set" if !rest.is_empty() => {
                let mut p = Parser { t: lex(rest)?, i: 0 };
                let Some(Tok::Id(var)) = p.peek().cloned() else { return Err("write {set NAME = VALUE}".into()) };
                p.i += 1;
                p.expect_op("=")?;
                let e = p.expr()?;
                p.done()?;
                out.push(Node::Set(var, e));
            }
            "br" | "newline" if rest.is_empty() => out.push(Node::Text("\n".into())),
            _ => out.push(Node::Out(parse_expr(tag).map_err(|e| format!("in {{{tag}}}: {e}"))?)),
        }
    }
    Ok((out, Stop::Eof))
}

pub struct Template {
    nodes: Vec<Node>,
}

impl Template {
    pub fn parse(src: &str) -> Result<Self, String> {
        let parts = split(src)?;
        let mut i = 0;
        let (nodes, stop) = block(&parts, &mut i)?;
        match stop {
            Stop::Eof => Ok(Self { nodes }),
            Stop::End => Err("{end} without a matching {if} or {for}".into()),
            _ => Err("{else} / {elif} without a matching {if}".into()),
        }
    }

    /// Output plus the first runtime error, if any.
    pub fn render(&self, vars: &Vars) -> (String, Option<String>) {
        let mut env = Env { vars, locals: Vec::new(), steps: 0, error: None };
        let mut out = String::new();
        env.run(&self.nodes, &mut out);
        (out, env.error)
    }
}

/// Render `src`; text with a syntax error is printed unchanged so nothing silently disappears.
pub fn render(src: &str, vars: &Vars) -> String {
    if !src.contains('{') && !src.contains('}') {
        return src.to_string();
    }
    match Template::parse(src) {
        Ok(t) => t.render(vars).0,
        Err(_) => src.to_string(),
    }
}

fn eval_condition(src: &str, vars: &Vars) -> Result<bool, String> {
    let src = src.trim();
    let src = src.strip_prefix('{').and_then(|s| s.strip_suffix('}')).unwrap_or(src);
    let e = parse_expr(src)?;
    let env = Env { vars, locals: Vec::new(), steps: 0, error: None };
    Ok(env.eval(&e)?.truthy())
}

/// Evaluate a "show when" condition. Empty means always; a broken condition also shows
/// the item, so a typo never hides content without the user noticing.
pub fn condition(src: &str, vars: &Vars) -> bool {
    src.trim().is_empty() || eval_condition(src, vars).unwrap_or(true)
}

/// First problem found when parsing and evaluating a template against `vars`.
pub fn check(src: &str, vars: &Vars) -> Option<String> {
    match Template::parse(src) {
        Ok(t) => t.render(vars).1,
        Err(e) => Some(e),
    }
}

pub fn check_condition(src: &str, vars: &Vars) -> Option<String> {
    if src.trim().is_empty() {
        return None;
    }
    eval_condition(src, vars).err()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> Vars {
        let mut v = Vars::new();
        v.insert("page".into(), Value::from(3usize));
        v.insert("pages".into(), Value::from(12usize));
        v.insert("odd".into(), Value::from(true));
        v.insert("even".into(), Value::from(false));
        v.insert("file".into(), Value::from("Report-DRAFT.pdf"));
        v.insert("sheetpages".into(), Value::List(vec![Value::Num(3.0), Value::Num(4.0)]));
        v
    }

    fn r(src: &str) -> String {
        let t = Template::parse(src).unwrap_or_else(|e| panic!("{src}: {e}"));
        let (out, err) = t.render(&vars());
        assert!(err.is_none(), "{src}: {err:?}");
        out
    }

    #[test]
    fn output_and_filters() {
        assert_eq!(r("Page {page} of {pages}"), "Page 3 of 12");
        assert_eq!(r("{page|roman} {page|roman|lower} {page|alpha} {(page + 24)|alpha}"), "III iii C AA");
        assert_eq!(r("{page|pad:4} {page|ordinal} {page|words|title}"), "0003 3rd Three");
        assert_eq!(r("{(page + 1) * 2} {pages / 5} {pages % 5}"), "8 2.4 2");
        assert_eq!(r("{file|upper|left:6} {file|replace(\".pdf\", \"\")}"), "REPORT Report-DRAFT");
        assert_eq!(r("{{literal}} {# comment } x"), "{literal}  x");
        assert_eq!(r("a{br}b"), "a\nb");
        assert_eq!(r("{\"Sheet \" + page}"), "Sheet 3");
        assert_eq!(r("{iif(odd, \"R\", \"L\")} {max(page, 7, 2)}"), "R 7");
    }

    #[test]
    fn branches_and_loops() {
        assert_eq!(r("{if odd}odd{else}even{end}"), "odd");
        assert_eq!(r("{if page = 1}one{elif page < 5}small{else}big{end}"), "small");
        assert_eq!(r("{if page > 1 and not even}A{end}{if page in [1, 2]}B{end}"), "A");
        assert_eq!(r("{if file contains \"draft\"}D{end}{if file endswith '.PDF'}P{end}"), "DP");
        assert_eq!(r("{for p in sheetpages sep \", \"}{p}{end}"), "3, 4");
        assert_eq!(r("{for i in 1..3}{i}{if not loop_last}-{end}{end}"), "1-2-3");
        assert_eq!(r("{for i in 3..1}{i}{end}|{for i in 3}*{end}"), "321|***");
        assert_eq!(r("{set n = page * 10}{n + 1}"), "31");
        assert_eq!(r("{if odd}{if page = 3}x{end}{endif}"), "x");
    }

    #[test]
    fn errors() {
        assert!(Template::parse("{if odd}x").is_err());
        assert!(Template::parse("{end}").is_err());
        assert!(Template::parse("{page").is_err());
        assert!(Template::parse("{page +}").is_err());
        assert!(check("{nope}", &vars()).is_some());
        assert!(check("{page|bogus}", &vars()).is_some());
        assert!(check("{page / 0}", &vars()).is_some());
        assert_eq!(render("{broken", &vars()), "{broken");
        assert!(condition("", &vars()));
        assert!(condition("{odd}", &vars()));
        assert!(!condition("even", &vars()));
        assert!(condition("page >", &vars()));
        assert!(check("{for i in 1..100000}{end}", &vars()).is_some());
    }
}
