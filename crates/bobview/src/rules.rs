//! Phoebus rules: expressions over a widget's rule PVs that override one property.
//!
//! The expression language covers what rule editors produce: `pv0`, `pvStr0`,
//! `pvInt0`, `pvSev0`, `pvLegacySev0` (BOY's numbering), numbers, strings, `true`/`false`, comparison,
//! arithmetic, bitwise `&`, `|`, `^`, `<<` and `>>`, `&&`/`and`,
//! `||`/`or`, `!`/`not` and parentheses.
//!
//! Phoebus runs a rule as a Jython script, so a `.bob` rule's operators bind
//! as Python's do ([`Lang::Python`]): comparisons chain (`1 < pv0 < 5`) and
//! bind looser than the bitwise operators (`pv0 & 4 == 4`), `!`/`not` looser
//! than comparisons (`!pv0 == 1` is `not (pv0 == 1)`), and `/` and `%` of
//! integers floor. BOY ran rules as JavaScript, so an `.opi` rule's bind as
//! JavaScript's do ([`Lang::JavaScript`]), as the display was written for.
//! In both, as in Phoebus, macros in the expressions are expanded first.
//! As Phoebus turns a rule's JavaScript-style logic into Python, a lone
//! `=` in a `.bob` rule is `==`.
//!
//! The bitwise operators take the integer part of a double, so `pv0 & 4`
//! works although `pv0` is a double. Jython raises a `TypeError` there
//! instead, and the rule falls through to the property's own value; bobview
//! is the more lenient.

use bob::{Element, Rule};
use pv::{Pv, PvManager};

/// The language a rule's expressions are evaluated as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    /// Phoebus (`.bob`): rules run as Jython.
    Python,
    /// BOY (`.opi`): rules ran as JavaScript.
    JavaScript,
}

pub struct CompiledRule {
    pub prop_id: String,
    lang: Lang,
    out_exp: bool,
    check_connections: bool,
    /// Each condition, the value it sets, and for an output expression
    /// rule that value parsed as an expression.
    exprs: Vec<(Option<Expr>, Element, Option<Expr>)>,
    pvs: Vec<Pv>,
}

/// What a rule wants the property to be right now.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Use this property element instead of the widget's own.
    Set(Element),
    /// No expression holds: keep the widget's own value.
    Keep,
}

impl CompiledRule {
    /// The PVs the rule reads.
    pub fn pvs(&self) -> &[Pv] {
        &self.pvs
    }

    /// Compile `rule`, its expressions in `lang`. `expand` expands macros
    /// in its expressions and PV names.
    pub fn new(rule: &Rule, lang: Lang, expand: &dyn Fn(&str) -> String, pvs: &PvManager) -> Self {
        let parse_expanded = |text: &str| {
            let text = expand(text);
            let parsed = parse(&text, lang);
            if parsed.is_none() {
                log::warn!("rule '{}': cannot parse '{text}'", rule.name);
            }
            parsed
        };
        let exprs = rule
            .exprs
            .iter()
            .map(|e| {
                let parsed = parse_expanded(&e.condition);
                let out = rule
                    .out_exp
                    .then(|| parse_expanded(&e.value.text))
                    .flatten();
                (parsed, e.value.clone(), out)
            })
            .collect();
        Self {
            prop_id: rule.prop_id.clone(),
            lang,
            out_exp: rule.out_exp,
            check_connections: rule.check_connections,
            exprs,
            pvs: rule
                .pv_names
                .iter()
                .map(|n| pvs.subscribe(&expand(n)))
                .collect(),
        }
    }

    pub fn evaluate(&self) -> Outcome {
        let mut states: Vec<pv::PvState> = self.pvs.iter().map(|p| p.state()).collect();
        // Phoebus does not evaluate a rule until all its PVs have values;
        // BOY does, with the disconnected ones INVALID.
        for s in states.iter_mut().filter(|s| !s.connected) {
            if self.check_connections {
                return Outcome::Keep;
            }
            s.severity = pv::Severity::Invalid;
        }
        let env = Env {
            states: &states,
            lang: self.lang,
        };
        for (expr, value, out) in &self.exprs {
            let Some(expr) = expr else { continue };
            if !eval(expr, &env).truthy() {
                continue;
            }
            if self.out_exp {
                // The value is an expression, giving the property's value.
                let Some(out) = out else { continue };
                return Outcome::Set(Element {
                    name: self.prop_id.clone(),
                    text: eval(out, &env).to_text(),
                    ..Default::default()
                });
            }
            let mut el = value.clone();
            el.name = self.prop_id.clone();
            return Outcome::Set(el);
        }
        Outcome::Keep
    }
}

/// Apply rule outcomes to a widget's properties. A property such as
/// `symbols[1]` names one item of a list property, and `traces[0].visible`
/// a property of one item.
pub fn apply(props: &mut Element, outcomes: &[(String, Outcome)]) {
    for (prop, outcome) in outcomes {
        if let Outcome::Set(el) = outcome {
            if let Some((list, index, sub)) = list_item(prop) {
                let item = props
                    .children
                    .iter_mut()
                    .find(|c| c.name == list)
                    .and_then(|l| l.children.get_mut(index));
                match (item, sub) {
                    (Some(item), None) => item.text = el.text.clone(),
                    (Some(item), Some(sub)) => {
                        let mut el = el.clone();
                        el.name = sub.to_string();
                        match item.children.iter_mut().find(|c| c.name == sub) {
                            Some(existing) => *existing = el,
                            None => item.children.push(el),
                        }
                    }
                    (None, _) => log::warn!("rule sets '{prop}', which the widget does not have"),
                }
                continue;
            }
            match props.children.iter_mut().find(|c| &c.name == prop) {
                Some(existing) => *existing = el.clone(),
                None => props.children.push(el.clone()),
            }
        }
    }
}

/// `symbols[1]` as the list `symbols` and index 1; `traces[0].visible`
/// also names the item's property `visible`.
fn list_item(prop: &str) -> Option<(&str, usize, Option<&str>)> {
    let (list, rest) = prop.split_once('[')?;
    let (index, sub) = rest.split_once(']')?;
    let sub = match sub {
        "" => None,
        s => Some(s.strip_prefix('.')?),
    };
    Some((list, index.parse().ok()?, sub))
}

#[derive(Clone, Debug, PartialEq)]
enum Val {
    Num(f64),
    /// A Python integer (JavaScript has only [`Val::Num`]).
    Int(i64),
    Str(String),
    Bool(bool),
}

impl Val {
    fn truthy(&self) -> bool {
        match self {
            Val::Bool(b) => *b,
            Val::Num(n) => *n != 0.0,
            Val::Int(n) => *n != 0,
            Val::Str(s) => !s.is_empty(),
        }
    }

    fn num(&self) -> f64 {
        match self {
            Val::Num(n) => *n,
            Val::Int(n) => *n as f64,
            Val::Bool(b) => *b as i32 as f64,
            Val::Str(s) => s.trim().parse().unwrap_or(f64::NAN),
        }
    }

    /// The value as a Python integer, if it is one (`bool` is one there).
    fn int(&self) -> Option<i64> {
        match self {
            Val::Int(n) => Some(*n),
            Val::Bool(b) => Some(*b as i64),
            _ => None,
        }
    }

    fn to_text(&self) -> String {
        match self {
            Val::Int(n) => n.to_string(),
            Val::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{}", *n as i64),
            Val::Num(n) => n.to_string(),
            Val::Str(s) => s.clone(),
            Val::Bool(b) => b.to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Expr {
    Lit(Val),
    Var(String),
    Not(Box<Expr>),
    Neg(Box<Expr>),
    Bin(Box<Expr>, String, Box<Expr>),
    /// A Python chained comparison, `a < b < c`: one more operand than
    /// comparisons.
    Chain(Vec<Expr>, Vec<String>),
    Call(Func, Vec<Expr>),
}

/// The math functions rule scripts call, from JavaScript's `Math` (BOY,
/// Phoebus) and Python's built-ins and `math` module (Phoebus).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Func {
    Min,
    Max,
    Abs,
    /// JavaScript's `Math.round`: halves round up.
    Round,
    /// Python's `round`, to whole numbers or to as many decimal places as
    /// its second argument: halves round to even.
    RoundEven,
    Floor,
    Ceil,
    Sqrt,
    Pow,
    /// JavaScript's `Math.log`: the natural logarithm.
    Ln,
    /// Python's `math.log`, to base e or to its second argument.
    Log,
    Exp,
}

impl Func {
    /// The function `name` called with `args` arguments, if it can be: a
    /// Python one with as many as it takes, a JavaScript one with at least
    /// as many as it uses (JavaScript ignores more).
    fn named(name: &str, args: usize) -> Option<Func> {
        let (module, f) = match name.split_once('.') {
            Some((m, f)) => (Some(m), f),
            None => (None, name),
        };
        let js = module == Some("Math");
        let (func, takes) = match (module, f) {
            (Some("Math"), "min") => (Func::Min, 0..=0),
            (Some("Math"), "max") => (Func::Max, 0..=0),
            // Python's need two numbers; one would be a list.
            (Some("math") | None, "min") => (Func::Min, 2..=usize::MAX),
            (Some("math") | None, "max") => (Func::Max, 2..=usize::MAX),
            (Some("Math" | "math") | None, "abs") => (Func::Abs, 1..=1),
            (Some("math"), "fabs") => (Func::Abs, 1..=1),
            (Some("Math"), "round") => (Func::Round, 1..=1),
            (None, "round") => (Func::RoundEven, 1..=2),
            (Some("Math" | "math"), "floor") => (Func::Floor, 1..=1),
            (Some("Math" | "math"), "ceil") => (Func::Ceil, 1..=1),
            (Some("Math" | "math"), "sqrt") => (Func::Sqrt, 1..=1),
            (Some("Math" | "math") | None, "pow") => (Func::Pow, 2..=2),
            (Some("Math"), "log") => (Func::Ln, 1..=1),
            (Some("math"), "log") => (Func::Log, 1..=2),
            (Some("Math" | "math"), "exp") => (Func::Exp, 1..=1),
            _ => return None,
        };
        let fits = if js {
            args >= *takes.start()
        } else {
            takes.contains(&args)
        };
        fits.then_some(func)
    }

    fn call(self, args: &[f64]) -> f64 {
        let arg = |i: usize| args.get(i).copied().unwrap_or(f64::NAN);
        match self {
            // A NaN argument makes the result NaN, as in JavaScript.
            Func::Min => args.iter().fold(f64::INFINITY, |m, &x| {
                if m.is_nan() || x.is_nan() {
                    f64::NAN
                } else {
                    m.min(x)
                }
            }),
            Func::Max => args.iter().fold(f64::NEG_INFINITY, |m, &x| {
                if m.is_nan() || x.is_nan() {
                    f64::NAN
                } else {
                    m.max(x)
                }
            }),
            Func::Abs => arg(0).abs(),
            Func::Round => (arg(0) + 0.5).floor(),
            Func::RoundEven => match args.get(1) {
                None => arg(0).round_ties_even(),
                Some(&digits) => round_to(arg(0), digits),
            },
            Func::Floor => arg(0).floor(),
            Func::Ceil => arg(0).ceil(),
            Func::Sqrt => arg(0).sqrt(),
            Func::Pow => arg(0).powf(arg(1)),
            Func::Ln => arg(0).ln(),
            Func::Log => match args.get(1) {
                None => arg(0).ln(),
                Some(base) => arg(0).ln() / base.ln(),
            },
            Func::Exp => arg(0).exp(),
        }
    }
}

/// Python's `round(x, digits)`: `x` to `digits` decimal places (tens,
/// hundreds and so on when negative), halves to even. As Python, `digits`
/// must be a whole number; Rust's formatting rounds as Python does.
fn round_to(x: f64, digits: f64) -> f64 {
    if digits.fract() != 0.0 || digits.is_nan() {
        return f64::NAN;
    }
    if !x.is_finite() || digits > 400.0 {
        return x;
    }
    if digits >= 0.0 {
        return format!("{x:.*}", digits as usize).parse().unwrap_or(x);
    }
    let unit = 10f64.powf(-digits);
    if unit.is_infinite() {
        return 0.0f64.copysign(x);
    }
    (x / unit).round_ties_even() * unit
}

struct Env<'a> {
    states: &'a [pv::PvState],
    lang: Lang,
}

impl Env<'_> {
    fn var(&self, name: &str) -> Val {
        let split = name
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(name.len());
        let (kind, idx) = name.split_at(split);
        let Some(s) = idx.parse::<usize>().ok().and_then(|i| self.states.get(i)) else {
            return Val::Num(0.0);
        };
        // `pv0` is a double (`PVUtil.getDouble`), the others integers.
        match kind {
            "pv" => match &s.value {
                pv::Value::Str(t) => Val::Str(t.clone()),
                v => Val::Num(v.as_f64().unwrap_or(0.0)),
            },
            "pvInt" => self.int(s.value.as_i64().unwrap_or(0)),
            "pvStr" => Val::Str(s.format(None, false)),
            "pvSev" => self.int(s.severity as i64),
            "pvLegacySev" => self.int(legacy_severity(s.severity)),
            _ => Val::Num(0.0),
        }
    }

    /// An integer: a Python `int`, or a JavaScript number.
    fn int(&self, n: i64) -> Val {
        match self.lang {
            Lang::Python => Val::Int(n),
            Lang::JavaScript => Val::Num(n as f64),
        }
    }
}

/// A severity as BOY numbered it: 0 none, 1 major, 2 minor, -1 invalid.
fn legacy_severity(s: pv::Severity) -> i64 {
    match s {
        pv::Severity::NoAlarm => 0,
        pv::Severity::Major => 1,
        pv::Severity::Minor => 2,
        pv::Severity::Invalid => -1,
    }
}

fn eval(e: &Expr, env: &Env) -> Val {
    match e {
        Expr::Lit(v) => v.clone(),
        Expr::Var(n) => env.var(n),
        Expr::Not(x) => Val::Bool(!eval(x, env).truthy()),
        Expr::Neg(x) => match eval(x, env) {
            Val::Int(n) => n.checked_neg().map_or(Val::Num(-(n as f64)), Val::Int),
            // A Python bool is an int.
            Val::Bool(b) if env.lang == Lang::Python => Val::Int(-(b as i64)),
            v => Val::Num(-v.num()),
        },
        Expr::Call(f, args) => {
            let args: Vec<Val> = args.iter().map(|a| eval(a, env)).collect();
            // Python's min, max and abs keep integers integers.
            let ints: Option<Vec<i64>> = match (env.lang, f) {
                (Lang::Python, Func::Min | Func::Max | Func::Abs) => {
                    args.iter().map(Val::int).collect()
                }
                _ => None,
            };
            let int = ints.and_then(|ns| match f {
                Func::Min => ns.into_iter().min(),
                Func::Max => ns.into_iter().max(),
                _ => ns.first()?.checked_abs(),
            });
            int.map_or_else(
                || Val::Num(f.call(&args.iter().map(Val::num).collect::<Vec<_>>())),
                Val::Int,
            )
        }
        Expr::Bin(a, op, b) => {
            let a = eval(a, env);
            // Short-circuit the logical operators, which give the operand
            // that decided, in Python as in JavaScript.
            match op.as_str() {
                "&&" => return if a.truthy() { eval(b, env) } else { a },
                "||" => return if a.truthy() { a } else { eval(b, env) },
                _ => {}
            }
            binary(&a, op, &eval(b, env), env.lang)
        }
        // As Python, each operand is evaluated once, left to right, and
        // the first false comparison stops the chain.
        Expr::Chain(operands, ops) => {
            let mut a = eval(&operands[0], env);
            let mut result = Val::Bool(true);
            for (op, b) in ops.iter().zip(&operands[1..]) {
                let b = eval(b, env);
                result = binary(&a, op, &b, env.lang);
                if !result.truthy() {
                    break;
                }
                a = b;
            }
            result
        }
    }
}

fn binary(a: &Val, op: &str, b: &Val, lang: Lang) -> Val {
    let strings = matches!((a, b), (Val::Str(_), Val::Str(_)));
    // Python keeps integers integers; JavaScript has only doubles.
    let ints = match (lang, a.int(), b.int()) {
        (Lang::Python, Some(x), Some(y)) => Some((x, y)),
        _ => None,
    };
    let int_or = |r: Option<i64>, f: f64| r.map_or(Val::Num(f), Val::Int);
    let (x, y) = (a.num(), b.num());
    match op {
        "==" if strings => Val::Bool(a == b),
        "!=" if strings => Val::Bool(a != b),
        "==" => Val::Bool(x == y),
        "!=" => Val::Bool(x != y),
        "<" if strings => Val::Bool(a.to_text() < b.to_text()),
        ">" if strings => Val::Bool(a.to_text() > b.to_text()),
        "<=" if strings => Val::Bool(a.to_text() <= b.to_text()),
        ">=" if strings => Val::Bool(a.to_text() >= b.to_text()),
        "<" => Val::Bool(x < y),
        ">" => Val::Bool(x > y),
        "<=" => Val::Bool(x <= y),
        ">=" => Val::Bool(x >= y),
        "+" if strings => Val::Str(a.to_text() + &b.to_text()),
        "+" => match ints {
            Some((i, j)) => int_or(i.checked_add(j), x + y),
            None => Val::Num(x + y),
        },
        "-" => match ints {
            Some((i, j)) => int_or(i.checked_sub(j), x - y),
            None => Val::Num(x - y),
        },
        "*" => match ints {
            Some((i, j)) => int_or(i.checked_mul(j), x * y),
            None => Val::Num(x * y),
        },
        // Python 2 (Jython) divides integers to the floor.
        "/" | "//" => match ints {
            Some((_, 0)) => Val::Num(f64::NAN),
            Some((i, j)) => int_or(floor_div(i, j), (x / y).floor()),
            None if op == "//" => Val::Num((x / y).floor()),
            None => Val::Num(x / y),
        },
        // Python's remainder has the divisor's sign, JavaScript's the
        // dividend's.
        "%" => match (lang, ints) {
            (_, Some((_, 0))) => Val::Num(f64::NAN),
            (_, Some((i, j))) => match floor_div(i, j) {
                Some(q) => Val::Int(i - j * q),
                None => Val::Int(0),
            },
            (Lang::Python, None) => Val::Num(x - y * (x / y).floor()),
            (Lang::JavaScript, None) => Val::Num(x % y),
        },
        // Python's bitwise operators keep two bools a bool.
        "&" | "|" | "^" if lang == Lang::Python => match (a, b) {
            (Val::Bool(p), Val::Bool(q)) => Val::Bool(match op {
                "&" => p & q,
                "|" => p | q,
                _ => p ^ q,
            }),
            _ => bitwise(op, x as i64, y as i64, lang),
        },
        "&" | "|" | "^" => bitwise(op, x as i64, y as i64, lang),
        "<<" | ">>" => shift(op, x, y, lang),
        _ => Val::Bool(false),
    }
}

/// `i op j` for the bitwise operators, on the integer parts.
fn bitwise(op: &str, i: i64, j: i64, lang: Lang) -> Val {
    let r = match op {
        "&" => i & j,
        "|" => i | j,
        _ => i ^ j,
    };
    match lang {
        Lang::Python => Val::Int(r),
        Lang::JavaScript => Val::Num(r as f64),
    }
}

/// `x << y` or `x >> y`, on the integer parts. Python's integers do not
/// overflow (a shift too big for 64 bits gives a double) and a negative
/// count is an error (NaN here, as for a division by zero); JavaScript's are
/// 32-bit, with the count taken modulo 32.
fn shift(op: &str, x: f64, y: f64, lang: Lang) -> Val {
    let left = op == "<<";
    match lang {
        Lang::Python => {
            let (i, j) = (x as i64, y as i64);
            if j < 0 {
                return Val::Num(f64::NAN);
            }
            if !left {
                return Val::Int(i >> j.min(63));
            }
            let r = if j < 64 {
                i.checked_shl(j as u32)
            } else {
                None
            };
            match r.filter(|r| r >> j == i) {
                Some(r) => Val::Int(r),
                None => Val::Num(i as f64 * 2f64.powi(j.min(2000) as i32)),
            }
        }
        Lang::JavaScript => {
            let (i, n) = (x as i64 as i32, (y as i64 as u32) & 31);
            Val::Num(if left { i.wrapping_shl(n) } else { i >> n } as f64)
        }
    }
}

/// `i // j`, rounding down as Python does (Rust rounds towards zero); none
/// where it overflows or `j` is 0.
fn floor_div(i: i64, j: i64) -> Option<i64> {
    let q = i.checked_div(j)?;
    Some(if i % j != 0 && (i < 0) != (j < 0) {
        q - 1
    } else {
        q
    })
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Int(i64),
    Str(String),
    Ident(String),
    Op(String),
    Open,
    Close,
    Comma,
}

fn tokenize(s: &str, lang: Lang) -> Option<Vec<Tok>> {
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit()
            || (c == '.' && cs.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            let start = i;
            while i < cs.len() && (cs[i].is_ascii_alphanumeric() || cs[i] == '.') {
                // Exponent sign, as in 1e-3.
                if (cs[i] == 'e' || cs[i] == 'E') && matches!(cs.get(i + 1), Some('-' | '+')) {
                    i += 1;
                }
                i += 1;
            }
            let text: String = cs[start..i].iter().collect();
            out.push(
                if let Some(hex) = text.strip_prefix("0x").or(text.strip_prefix("0X")) {
                    Tok::Int(i64::from_str_radix(hex, 16).ok()?)
                } else if let Ok(n) = text.parse::<i64>() {
                    Tok::Int(n)
                } else {
                    Tok::Num(text.parse().ok()?)
                },
            );
        } else if c == '"' || c == '\'' {
            let end = cs[i + 1..].iter().position(|&d| d == c)? + i + 1;
            out.push(Tok::Str(cs[i + 1..end].iter().collect()));
            i = end + 1;
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            // Dotted names, as in Math.min.
            while i < cs.len()
                && (cs[i].is_alphanumeric()
                    || cs[i] == '_'
                    || (cs[i] == '.' && cs.get(i + 1).is_some_and(|d| d.is_alphabetic())))
            {
                i += 1;
            }
            let word: String = cs[start..i].iter().collect();
            out.push(match word.as_str() {
                "and" => Tok::Op("&&".into()),
                "or" => Tok::Op("||".into()),
                "not" => Tok::Op("!".into()),
                _ => Tok::Ident(word),
            });
        } else if c == '(' {
            out.push(Tok::Open);
            i += 1;
        } else if c == ')' {
            out.push(Tok::Close);
            i += 1;
        } else if c == ',' {
            out.push(Tok::Comma);
            i += 1;
        } else {
            let two: String = cs[i..(i + 2).min(cs.len())].iter().collect();
            if ["==", "!=", "<=", ">=", "&&", "||", "//", "<<", ">>"].contains(&two.as_str()) {
                out.push(Tok::Op(two));
                i += 2;
            } else if c == '=' && lang == Lang::Python {
                // As Phoebus's `javascriptToPythonLogic` makes it.
                out.push(Tok::Op("==".into()));
                i += 1;
            } else if "<>!+-*/%&|^".contains(c) {
                out.push(Tok::Op(c.to_string()));
                i += 1;
            } else {
                return None;
            }
        }
    }
    Some(out)
}

fn parse(s: &str, lang: Lang) -> Option<Expr> {
    let toks = tokenize(s, lang)?;
    // A long chain (`1+1+1+...`) builds a deep left-leaning tree that the
    // evaluator and `Drop` walk recursively, so bound the length too.
    if toks.len() > MAX_TOKENS {
        return None;
    }
    let mut p = Parser {
        toks,
        pos: 0,
        lang,
        depth: 0,
    };
    let e = p.or()?;
    (p.pos == p.toks.len()).then_some(e)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    lang: Lang,
    /// Nesting of brackets, calls and prefix operators, bounded by
    /// `MAX_DEPTH`.
    depth: usize,
}

/// Deepest nesting of brackets, calls and prefix operators (`!`, `not`,
/// `-`, `+`) a rule expression may have.
const MAX_DEPTH: usize = 64;
/// Most tokens a rule expression may have.
const MAX_TOKENS: usize = 1024;

const COMPARISONS: &[&str] = &["==", "!=", "<", ">", "<=", ">="];

/// Binary operators above the logical ones, loosest first. Python puts
/// all comparisons, which chain, below the bitwise operators.
const PYTHON: &[&[&str]] = &[
    COMPARISONS,
    &["|"],
    &["^"],
    &["&"],
    &["<<", ">>"],
    &["+", "-"],
    &["*", "/", "//", "%"],
];

const JAVASCRIPT: &[&[&str]] = &[
    &["|"],
    &["^"],
    &["&"],
    &["==", "!="],
    &["<", ">", "<=", ">="],
    &["<<", ">>"],
    &["+", "-"],
    &["*", "/", "%"],
];

impl Parser {
    fn op(&self) -> Option<&str> {
        match self.toks.get(self.pos) {
            Some(Tok::Op(op)) => Some(op),
            _ => None,
        }
    }

    fn or(&mut self) -> Option<Expr> {
        let mut lhs = self.and()?;
        while self.op() == Some("||") {
            self.pos += 1;
            lhs = Expr::Bin(Box::new(lhs), "||".into(), Box::new(self.and()?));
        }
        Some(lhs)
    }

    fn and(&mut self) -> Option<Expr> {
        let mut lhs = self.not()?;
        while self.op() == Some("&&") {
            self.pos += 1;
            lhs = Expr::Bin(Box::new(lhs), "&&".into(), Box::new(self.not()?));
        }
        Some(lhs)
    }

    /// Python's `not`, looser than comparisons; JavaScript's `!` is unary.
    fn not(&mut self) -> Option<Expr> {
        if self.lang == Lang::Python && self.op() == Some("!") {
            self.pos += 1;
            return self.nested(|p| Some(Expr::Not(Box::new(p.not()?))));
        }
        self.binary(0)
    }

    fn levels(&self) -> &'static [&'static [&'static str]] {
        match self.lang {
            Lang::Python => PYTHON,
            Lang::JavaScript => JAVASCRIPT,
        }
    }

    fn binary(&mut self, level: usize) -> Option<Expr> {
        let levels = self.levels();
        if level == levels.len() {
            return self.unary();
        }
        let ops = levels[level];
        let mut lhs = self.binary(level + 1)?;
        // Python chains comparisons: `a < b < c` is `a < b and b < c`, with
        // `b` evaluated once.
        if self.lang == Lang::Python && ops == COMPARISONS {
            let mut operands = vec![lhs];
            let mut chain_ops = Vec::new();
            while let Some(op) = self.op().filter(|op| ops.contains(op)).map(str::to_string) {
                self.pos += 1;
                chain_ops.push(op);
                operands.push(self.binary(level + 1)?);
            }
            return Some(match chain_ops.len() {
                0 => operands.pop()?,
                1 => {
                    let b = operands.pop()?;
                    let a = operands.pop()?;
                    Expr::Bin(Box::new(a), chain_ops.pop()?, Box::new(b))
                }
                _ => Expr::Chain(operands, chain_ops),
            });
        }
        while let Some(op) = self.op().filter(|op| ops.contains(op)).map(str::to_string) {
            self.pos += 1;
            let rhs = self.binary(level + 1)?;
            lhs = Expr::Bin(Box::new(lhs), op, Box::new(rhs));
        }
        Some(lhs)
    }

    /// Parse with `f` one level deeper, failing past `MAX_DEPTH`: every
    /// recursion back into the grammar (brackets, call arguments and prefix
    /// operators) passes through here, so hostile nesting can't exhaust the
    /// stack.
    fn nested(&mut self, f: impl FnOnce(&mut Self) -> Option<Expr>) -> Option<Expr> {
        if self.depth >= MAX_DEPTH {
            return None;
        }
        self.depth += 1;
        let e = f(self);
        self.depth -= 1;
        e
    }

    fn unary(&mut self) -> Option<Expr> {
        self.nested(Self::unary_inner)
    }

    fn unary_inner(&mut self) -> Option<Expr> {
        let tok = self.toks.get(self.pos)?.clone();
        self.pos += 1;
        let int = |n: i64| match self.lang {
            Lang::Python => Val::Int(n),
            Lang::JavaScript => Val::Num(n as f64),
        };
        Some(match tok {
            Tok::Op(op) if op == "!" => Expr::Not(Box::new(self.unary()?)),
            Tok::Op(op) if op == "-" => Expr::Neg(Box::new(self.unary()?)),
            Tok::Op(op) if op == "+" => self.unary()?,
            Tok::Num(n) => Expr::Lit(Val::Num(n)),
            Tok::Int(n) => Expr::Lit(int(n)),
            Tok::Str(s) => Expr::Lit(Val::Str(s)),
            Tok::Ident(w) => match w.as_str() {
                "true" | "True" => Expr::Lit(Val::Bool(true)),
                "false" | "False" => Expr::Lit(Val::Bool(false)),
                "Math.PI" | "math.pi" => Expr::Lit(Val::Num(std::f64::consts::PI)),
                "Math.E" | "math.e" => Expr::Lit(Val::Num(std::f64::consts::E)),
                _ if self.toks.get(self.pos) == Some(&Tok::Open) => {
                    self.pos += 1;
                    let args = self.args()?;
                    Expr::Call(Func::named(&w, args.len())?, args)
                }
                _ if w.contains('.') => return None,
                _ => Expr::Var(w),
            },
            Tok::Open => {
                let e = self.or()?;
                (self.toks.get(self.pos) == Some(&Tok::Close)).then_some(())?;
                self.pos += 1;
                e
            }
            _ => return None,
        })
    }

    /// A call's arguments, after its opening bracket, through the closing one.
    fn args(&mut self) -> Option<Vec<Expr>> {
        let mut args = Vec::new();
        if self.toks.get(self.pos) == Some(&Tok::Close) {
            self.pos += 1;
            return Some(args);
        }
        loop {
            args.push(self.or()?);
            let tok = self.toks.get(self.pos)?.clone();
            self.pos += 1;
            match tok {
                Tok::Comma => {}
                Tok::Close => return Some(args),
                _ => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_as(lang: Lang, expr: &str, values: &[pv::Value]) -> Val {
        let states: Vec<pv::PvState> = values
            .iter()
            .map(|v| pv::PvState {
                connected: true,
                value: v.clone(),
                ..Default::default()
            })
            .collect();
        let env = Env {
            states: &states,
            lang,
        };
        eval(&parse(expr, lang).expect("parses"), &env)
    }

    fn run(expr: &str, values: &[pv::Value]) -> Val {
        run_as(Lang::Python, expr, values)
    }

    fn js(expr: &str, values: &[pv::Value]) -> Val {
        run_as(Lang::JavaScript, expr, values)
    }

    /// Run `f` on a thread with a small stack, so unbounded recursion fails
    /// this test instead of passing on the test runner's larger stack. An
    /// unoptimised build needs about 2 MiB for `MAX_DEPTH` brackets, an
    /// optimised one under 256 KiB, against the 16 MiB a display is built
    /// on; without the bound 500 brackets need about 8 MiB.
    fn small_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
        std::thread::Builder::new()
            .stack_size(4 * 1024 * 1024)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap()
    }

    #[test]
    fn nested_chained_comparisons_parse_in_linear_time() {
        // Each level's middle operand is the next chain; copying it into
        // both comparisons would double the tree at every level.
        let mut e = String::from("1");
        for _ in 0..40 {
            e = format!("(0 < {e} < 2)");
        }
        let start = std::time::Instant::now();
        let parsed = small_stack(move || parse(&e, Lang::Python));
        assert!(parsed.is_some());
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        let env = Env {
            states: &[],
            lang: Lang::Python,
        };
        assert!(eval(&parsed.unwrap(), &env).truthy());
        // The first false comparison decides, as in Python.
        assert_eq!(run("3 > 2 > 2 > 1", &[]), Val::Bool(false));
        assert_eq!(run("1 < 3 > 2 != 5", &[]), Val::Bool(true));
    }

    #[test]
    fn hostile_expressions_are_rejected_not_overflowed() {
        // Nesting within `MAX_TOKENS`, so only the depth bound rejects it.
        let deep = |open: &str, n: usize, close: &str| {
            let e = format!("{}1{}", open.repeat(n), close.repeat(n));
            assert!(
                tokenize(&e, Lang::Python).unwrap().len() <= MAX_TOKENS,
                "{e}"
            );
            e
        };
        small_stack(move || {
            for lang in [Lang::Python, Lang::JavaScript] {
                assert!(parse(&deep("(", 500, ")"), lang).is_none());
                assert!(parse(&deep("!", 1000, ""), lang).is_none());
                assert!(parse(&deep("-", 1000, ""), lang).is_none());
                assert!(parse(&deep("+", 1000, ""), lang).is_none());
                assert!(parse(&deep("Math.abs(", 300, ")"), lang).is_none());
                // Nesting reached through a call's later arguments.
                assert!(parse(&deep("max(1, ", 200, ")"), lang).is_none());
                // Long chains, bounded by `MAX_TOKENS`.
                assert!(parse(&vec!["1"; 20000].join("+"), lang).is_none());
                assert!(parse(&vec!["1"; 20000].join("<"), lang).is_none());
                assert!(parse(&format!("max({})", vec!["1"; 20000].join(",")), lang).is_none());
                // Reasonable nesting still works.
                assert!(parse(&deep("(", 30, ")"), lang).is_some());
            }
            // Python's `not`, which recurses outside `unary`, alone and
            // inside brackets and chained comparisons.
            let py = Lang::Python;
            assert!(parse(&deep("not ", 1000, ""), py).is_none());
            assert!(parse(&deep("(not ", 300, ")"), py).is_none());
            assert!(parse(&deep("(1 < 2 < not ", 120, ")"), py).is_none());
            assert!(parse(&deep("not ", 30, ""), py).is_some());
        });
    }

    #[test]
    fn evaluates_typical_rules() {
        use pv::Value::*;
        assert!(run("pv0 == 1", &[Double(1.0)]).truthy());
        assert!(!run("pv0 > 5 && pv1 < 3", &[Double(6.0), Long(4)]).truthy());
        assert!(run("pv0 > 5 or not pv1", &[Double(1.0), Long(0)]).truthy());
        assert!(run("pvStr0 == \"Done\"", &[Str("Done".into())]).truthy());
        assert!(run("!(pv0 - 1.5e-1 <= 0)", &[Double(1.0)]).truthy());
        assert_eq!(run("pv0 * 2 + 1", &[Long(3)]), Val::Num(7.0));
        assert!(run("pvSev0 == 0", &[Long(0)]).truthy());
        assert!(run("(pv0 & 4)==4", &[Long(6)]).truthy());
        assert!(!run("(pv0 & 1)!=0", &[Long(6)]).truthy());
        assert_eq!(run("pv0 | 1 ^ 3", &[Long(4)]), Val::Int(6));
        assert_eq!(js("pv0 | 1 ^ 3", &[Long(4)]), Val::Num(6.0));
    }

    #[test]
    fn bob_rules_bind_as_python() {
        use pv::Value::*;
        // Bitwise binds tighter than comparison: (pv0 & 4) == 4.
        assert!(run("pv0 & 4 == 4", &[Long(6)]).truthy());
        assert!(!run("pv0 & 4 == 4", &[Long(2)]).truthy());
        // `not` is looser than comparison: not (pv0 == 1).
        assert!(run("!pv0 == 1", &[Long(0)]).truthy());
        assert!(!run("!pv0 == 1", &[Long(1)]).truthy());
        assert!(run("not pv0 == 1", &[Long(2)]).truthy());
        // Comparisons chain.
        assert!(run("1 < pv0 < 5", &[Long(3)]).truthy());
        assert!(!run("1 < pv0 < 5", &[Long(7)]).truthy());
        assert!(!run("1 < pv0 < 5", &[Long(0)]).truthy());
        assert!(run("0 <= pv0 < pv1 <= 10", &[Long(2), Long(4)]).truthy());
        assert!(!run("0 <= pv0 < pv1 <= 10", &[Long(4), Long(2)]).truthy());
        // Integer division floors, as in Jython; pv0 is a double.
        assert_eq!(run("7 / 2", &[]), Val::Int(3));
        assert_eq!(run("-7 / 2", &[]), Val::Int(-4));
        assert_eq!(run("7 / -2", &[]), Val::Int(-4));
        assert_eq!(run("-7 % 3", &[]), Val::Int(2));
        assert_eq!(run("7 % -3", &[]), Val::Int(-2));
        assert_eq!(run("pvInt0 / 2", &[Long(7)]), Val::Int(3));
        assert_eq!(run("pv0 / 2", &[Long(7)]), Val::Num(3.5));
        assert_eq!(run("7.0 / 2", &[]), Val::Num(3.5));
        assert_eq!(run("7 // 2.0", &[]), Val::Num(3.0));
        assert_eq!(run("-7.5 % 2", &[]), Val::Num(0.5));
        assert_eq!(run("0x10 + 1", &[]), Val::Int(17));
        // `and`/`or` give the operand that decided.
        assert_eq!(
            run("pv0 > 0 and 'up' or 'down'", &[Long(1)]),
            Val::Str("up".into())
        );
        assert_eq!(
            run("pv0 > 0 and 'up' or 'down'", &[Long(0)]),
            Val::Str("down".into())
        );
        assert!(run("pvStr0 < pvStr1", &[Str("10".into()), Str("9".into())]).truthy());
    }

    #[test]
    fn shifts_bind_as_in_python() {
        use pv::Value::*;
        // Looser than `+`, tighter than `&` and comparisons.
        assert_eq!(run("1 << 2 + 1", &[]), Val::Int(8));
        assert_eq!(run("pvInt0 & 1 << 2", &[Long(6)]), Val::Int(4));
        assert_eq!(run("-8 >> 1", &[]), Val::Int(-4));
        assert_eq!(run("1 << 64", &[]), Val::Num(2f64.powi(64)));
        assert!(matches!(run("1 << -1", &[]), Val::Num(n) if n.is_nan()));
        // As a techui-support positioner display chains them.
        for (a, b, state) in [(0, 0, 0), (1, 0, 1), (0, 1, 2), (1, 1, 3)] {
            let pvs = [Long(a), Long(b)];
            assert_eq!(run("pvInt0 + (pvInt1<<1)", &pvs), Val::Int(state));
            let mid = run(" 2 <= (pvInt0 + (pvInt1<<1)) < 4", &pvs).truthy();
            assert_eq!(mid, (2..4).contains(&state), "{a} {b}");
        }
        // JavaScript's are 32-bit, below comparisons.
        assert_eq!(js("1 << 2 + 1", &[]), Val::Num(8.0));
        assert_eq!(js("1 << 33", &[]), Val::Num(2.0));
        assert!(js("1 << 2 == 4", &[]).truthy());
    }

    #[test]
    fn python_rules_turn_a_lone_equals_into_a_comparison() {
        use pv::Value::*;
        assert!(run("pv0 = 1", &[Long(1)]).truthy());
        assert!(!run("pv0 = 1", &[Long(2)]).truthy());
        assert!(run("pv0 <= 1 and pv0 >= 1 and pv0 != 2", &[Long(1)]).truthy());
    }

    #[test]
    fn python_bools_stay_bools() {
        // As Jython: `True & True` is a bool, which Phoebus sets a property
        // to as Java's Boolean, "true"; `-True` is the int -1.
        let v = run("(1 < 2) & (2 < 3)", &[]);
        assert_eq!(v, Val::Bool(true));
        assert_eq!(v.to_text(), "true");
        assert_eq!(run("(1 < 2) ^ (2 < 3)", &[]), Val::Bool(false));
        assert_eq!(run("(1 < 2) & 3", &[]), Val::Int(1));
        assert_eq!(run("-(1 < 2)", &[]), Val::Int(-1));
        assert_eq!(js("(1 < 2) & (2 < 3)", &[]), Val::Num(1.0));
    }

    #[test]
    fn opi_rules_bind_as_javascript() {
        use pv::Value::*;
        // 4 == 4 first, then pv0 & true.
        assert!(!js("pv0 & 4 == 4", &[Long(6)]).truthy());
        assert!(js("pv0 & 4 == 4", &[Long(5)]).truthy());
        // (!pv0) == 1.
        assert!(js("!pv0 == 1", &[Long(0)]).truthy());
        assert!(!js("!pv0 == 1", &[Long(2)]).truthy());
        // (1 < pv0) < 5: a boolean, always below 5.
        assert!(js("1 < pv0 < 5", &[Long(7)]).truthy());
        assert_eq!(js("7 / 2", &[]), Val::Num(3.5));
        assert_eq!(js("-7 % 3", &[]), Val::Num(-1.0));
    }

    #[test]
    fn legacy_severities_use_boy_numbers() {
        let state = |severity| pv::PvState {
            connected: true,
            severity,
            ..Default::default()
        };
        let states = [state(pv::Severity::Major), state(pv::Severity::Invalid)];
        let env = Env {
            states: &states,
            lang: Lang::Python,
        };
        let ev = |e: &str| eval(&parse(e, Lang::Python).unwrap(), &env);
        assert_eq!(ev("pvLegacySev0"), Val::Int(1));
        assert_eq!(ev("pvSev0"), Val::Int(2));
        assert!(ev("pvLegacySev1 == -1").truthy());
    }

    #[test]
    fn boy_rules_read_disconnected_pvs_as_invalid() {
        let rule = |check_connections| bob::Rule {
            name: "r".into(),
            prop_id: "background_color".into(),
            out_exp: false,
            exprs: vec![bob::RuleExpr {
                condition: "pvLegacySev0==-1".into(),
                value: Element {
                    name: "value".into(),
                    text: "white".into(),
                    ..Default::default()
                },
            }],
            pv_names: vec!["ca://bobview:test:never".into()],
            check_connections,
        };
        let pvs = PvManager::new();
        let phoebus = CompiledRule::new(&rule(true), Lang::Python, &|t| t.to_string(), &pvs);
        assert_eq!(phoebus.evaluate(), Outcome::Keep);
        let boy = CompiledRule::new(&rule(false), Lang::JavaScript, &|t| t.to_string(), &pvs);
        let Outcome::Set(el) = boy.evaluate() else {
            panic!("no outcome")
        };
        assert_eq!(el.text, "white");
    }

    #[test]
    fn output_expressions_set_the_value_where_the_condition_holds() {
        let rule = |out_exp: bool| bob::Rule {
            name: "r".into(),
            prop_id: "image_index".into(),
            out_exp,
            exprs: [("pv0 < 1", "pv0 + 5"), ("true", "0")]
                .iter()
                .map(|(c, v)| bob::RuleExpr {
                    condition: c.to_string(),
                    value: Element {
                        name: "value".into(),
                        text: v.to_string(),
                        ..Default::default()
                    },
                })
                .collect(),
            pv_names: vec!["loc://rule_out(0.5)".into()],
            check_connections: true,
        };
        let pvs = PvManager::new();
        let compiled = CompiledRule::new(&rule(true), Lang::Python, &|t| t.to_string(), &pvs);
        let start = std::time::Instant::now();
        while !compiled.pvs[0].state().connected && start.elapsed().as_secs() < 2 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let Outcome::Set(el) = compiled.evaluate() else {
            panic!("no outcome")
        };
        assert_eq!(el.text, "5.5");
        compiled.pvs[0].write("3");
        let start = std::time::Instant::now();
        while compiled.pvs[0].state().value.as_f64() != Some(3.0) && start.elapsed().as_secs() < 2 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            compiled.evaluate(),
            Outcome::Set(Element {
                name: "image_index".into(),
                text: "0".into(),
                ..Default::default()
            })
        );
    }

    #[test]
    fn calls_math_functions() {
        use pv::Value::*;
        let both = [Double(300.0), Double(400.0)];
        assert_eq!(
            run("Math.min((pv0 / pv1) * 500, 500)", &both),
            Val::Num(375.0)
        );
        assert_eq!(run("Math.max(pv0, pv1, 1)", &both), Val::Num(400.0));
        assert_eq!(run("Math.abs(-pv0)", &both), Val::Num(300.0));
        assert_eq!(
            run("Math.round(2.5) + Math.round(-2.5)", &[]),
            Val::Num(1.0)
        );
        assert_eq!(run("round(2.5)", &[]), Val::Num(2.0));
        assert_eq!(run("Math.floor(1.7) + Math.ceil(1.2)", &[]), Val::Num(3.0));
        assert_eq!(run("Math.sqrt(Math.pow(3, 2) + 16)", &[]), Val::Num(5.0));
        assert_eq!(run("Math.log(Math.exp(2))", &[]), Val::Num(2.0));
        // Python's second arguments; JavaScript ignores them.
        assert_eq!(run("round(2.675, 2)", &[]), Val::Num(2.67));
        assert_eq!(run("round(0.125, 2)", &[]), Val::Num(0.12));
        assert_eq!(run("round(1250, -2)", &[]), Val::Num(1200.0));
        assert_eq!(run("round(1e300, -400)", &[]), Val::Num(0.0));
        assert!(run("round(1, 0.5)", &[]).num().is_nan());
        assert_eq!(run("math.log(8, 2)", &[]), Val::Num(3.0));
        assert_eq!(run("math.log(Math.E)", &[]), Val::Num(1.0));
        assert_eq!(run("Math.log(Math.E, 2)", &[]), Val::Num(1.0));
        assert_eq!(run("Math.round(2.4, 3)", &[]), Val::Num(2.0));
        assert_eq!(run("Math.max()", &[]), Val::Num(f64::NEG_INFINITY));
        // Too few or too many arguments are not run, as unknown functions.
        for bad in [
            "round(1, 2, 3)",
            "math.log()",
            "math.log(1, 2, 3)",
            "math.sqrt(4, 2)",
            "pow(2)",
            "max(1)",
            "Math.pow(2)",
            "Math.abs()",
        ] {
            for lang in [Lang::Python, Lang::JavaScript] {
                assert!(parse(bad, lang).is_none(), "{bad}");
            }
        }
        assert!(run("Math.min(pv0, 1) == 1 && max(1, 2) > 1", &both).truthy());
        assert!(parse("Math.nope(1)", Lang::Python).is_none());
        assert!(parse("Math.min(1,", Lang::Python).is_none());
        assert!(parse("Math.min(1 2)", Lang::JavaScript).is_none());
    }

    #[test]
    fn calls_bind_in_both_languages() {
        use pv::Value::*;
        let v = [Double(-3.0)];
        // Python: calls inside chained comparisons, a loose `not`, and
        // integer results that floor-divide.
        assert_eq!(run("1 < abs(pv0) < 5", &v), Val::Bool(true));
        assert_eq!(run("not max(1, 2) == 1", &[]), Val::Bool(true));
        assert_eq!(run("max(7, 2) / 2", &[]), Val::Int(3));
        assert_eq!(run("abs(-7) % 4", &[]), Val::Int(3));
        assert_eq!(run("max(1, 2.5)", &[]), Val::Num(2.5));
        assert_eq!(run("Math.min(pv0 > 0 and 1 or 2, 5)", &v), Val::Int(2));
        assert_eq!(run("round(7 / 2)", &[]), Val::Num(3.0));
        // JavaScript: doubles throughout.
        assert_eq!(js("Math.max(7, 2) / 2", &[]), Val::Num(3.5));
        assert_eq!(js("Math.abs(pv0) & 2", &v), Val::Num(2.0));
        assert_eq!(js("Math.round(-2.5)", &[]), Val::Num(-2.0));
        assert_eq!(js("!Math.min(0, 1) == 1", &[]), Val::Bool(true));
    }

    #[test]
    fn rejects_garbage() {
        for lang in [Lang::Python, Lang::JavaScript] {
            assert!(parse("pv0 ==", lang).is_none());
            assert!(parse("(pv0", lang).is_none());
            assert!(parse("pv0 == $(STATE)", lang).is_none());
        }
        assert!(parse("7 // 2", Lang::JavaScript).is_none());
        // A lone `=` is `==` only in Python, as Phoebus makes it.
        assert!(parse("pv0 = 1", Lang::JavaScript).is_none());
    }

    #[test]
    fn macros_in_expressions_are_expanded() {
        let rule = bob::Rule {
            name: "r".into(),
            prop_id: "visible".into(),
            out_exp: true,
            exprs: vec![bob::RuleExpr {
                condition: "pv0 == $(STATE)".into(),
                value: Element {
                    name: "expression".into(),
                    text: "pv0 + $(OFFSET)".into(),
                    ..Default::default()
                },
            }],
            pv_names: vec!["loc://rule_macro(2)".into()],
            check_connections: true,
        };
        let expand = |t: &str| t.replace("$(STATE)", "2").replace("$(OFFSET)", "10");
        let pvs = PvManager::new();
        let compiled = CompiledRule::new(&rule, Lang::Python, &expand, &pvs);
        assert!(compiled.exprs[0].0.is_some() && compiled.exprs[0].2.is_some());
        let start = std::time::Instant::now();
        while !compiled.pvs[0].state().connected && start.elapsed().as_secs() < 2 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let Outcome::Set(el) = compiled.evaluate() else {
            panic!("no outcome")
        };
        assert_eq!(el.text, "12");
    }

    #[test]
    fn applies_overrides() {
        let mut props = Element {
            children: vec![Element {
                name: "visible".into(),
                text: "true".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let set = Element {
            name: "visible".into(),
            text: "false".into(),
            ..Default::default()
        };
        apply(&mut props, &[("visible".into(), Outcome::Set(set))]);
        assert_eq!(props.child_text("visible"), Some("false"));
    }

    #[test]
    fn output_expressions_on_boolean_properties_read_as_phoebus_does() {
        use pv::Value::Long;
        // `and` gives an operand, which Phoebus's boolean property takes
        // only as 1 or 0; another number is invalid, giving the default.
        let visible = |values: &[pv::Value]| {
            let text = js("pv0 && pv1", values).to_text();
            let mut el = Element::default();
            let set = Element {
                name: "visible".into(),
                text,
                ..Default::default()
            };
            apply(&mut el, &[("visible".into(), Outcome::Set(set))]);
            bob::Widget::from_element(el).bool("visible")
        };
        assert_eq!(visible(&[Long(1), Long(1)]), Some(true));
        assert_eq!(visible(&[Long(1), Long(0)]), Some(false));
        assert_eq!(visible(&[Long(5), Long(3)]), None);
        assert_eq!(
            run("pv0 > 1 and pv1 > 1", &[Long(5), Long(3)]).to_text(),
            "true"
        );
    }

    #[test]
    fn sets_one_item_of_a_list_property() {
        let el = |name: &str, text: &str| Element {
            name: name.into(),
            text: text.into(),
            ..Default::default()
        };
        let mut props = Element {
            children: vec![Element {
                name: "symbols".into(),
                children: vec![el("symbol", "a.svg"), el("symbol", "b.svg")],
                ..Default::default()
            }],
            ..Default::default()
        };
        let set = |text| Outcome::Set(el("symbols[1]", text));
        apply(&mut props, &[("symbols[1]".into(), set("c.svg"))]);
        apply(&mut props, &[("symbols[5]".into(), set("d.svg"))]);
        let items: Vec<_> = props.children[0].children.iter().map(|c| &c.text).collect();
        assert_eq!(items, ["a.svg", "c.svg"]);
        assert_eq!(props.children[0].children[1].name, "symbol");
        assert_eq!(props.children.len(), 1);
    }

    #[test]
    fn sets_a_property_of_one_list_item() {
        let el = |name: &str, text: &str| Element {
            name: name.into(),
            text: text.into(),
            ..Default::default()
        };
        let trace = |visible| Element {
            name: "trace".into(),
            children: vec![el("visible", visible)],
            ..Default::default()
        };
        let mut props = Element {
            children: vec![Element {
                name: "traces".into(),
                children: vec![trace("true"), trace("true")],
                ..Default::default()
            }],
            ..Default::default()
        };
        let prop = "traces[1].visible";
        apply(
            &mut props,
            &[(prop.into(), Outcome::Set(el(prop, "false")))],
        );
        let traces = &props.children[0].children;
        assert_eq!(traces[0].child_text("visible"), Some("true"));
        assert_eq!(traces[1].child_text("visible"), Some("false"));
        assert_eq!(traces[1].children.len(), 1);
    }
}
