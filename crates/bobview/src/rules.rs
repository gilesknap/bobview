//! Phoebus rules: expressions over a widget's rule PVs that override one property.
//!
//! The expression language covers what rule editors produce: `pv0`, `pvStr0`,
//! `pvInt0`, `pvSev0`, `pvLegacySev0` (BOY's numbering), numbers, strings, `true`/`false`, comparison,
//! arithmetic, bitwise `&`, `|` and `^`, `&&`/`and`, `||`/`or`, `!`/`not`
//! and parentheses.

use bob::{Element, Rule};
use pv::{Pv, PvManager};

pub struct CompiledRule {
    pub prop_id: String,
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

    pub fn new(rule: &Rule, expand: &dyn Fn(&str) -> String, pvs: &PvManager) -> Self {
        let exprs = rule
            .exprs
            .iter()
            .map(|e| {
                let parsed = parse(&e.condition);
                if parsed.is_none() {
                    log::warn!("rule '{}': cannot parse '{}'", rule.name, e.condition);
                }
                let out = rule.out_exp.then(|| parse(&e.value.text)).flatten();
                if rule.out_exp && out.is_none() {
                    log::warn!("rule '{}': cannot parse '{}'", rule.name, e.value.text);
                }
                (parsed, e.value.clone(), out)
            })
            .collect();
        Self {
            prop_id: rule.prop_id.clone(),
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
        let env = Env { states: &states };
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
    Str(String),
    Bool(bool),
}

impl Val {
    fn truthy(&self) -> bool {
        match self {
            Val::Bool(b) => *b,
            Val::Num(n) => *n != 0.0,
            Val::Str(s) => !s.is_empty(),
        }
    }

    fn num(&self) -> f64 {
        match self {
            Val::Num(n) => *n,
            Val::Bool(b) => *b as i32 as f64,
            Val::Str(s) => s.trim().parse().unwrap_or(f64::NAN),
        }
    }

    fn to_text(&self) -> String {
        match self {
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
        match kind {
            "pv" => match &s.value {
                pv::Value::Str(t) => Val::Str(t.clone()),
                v => Val::Num(v.as_f64().unwrap_or(0.0)),
            },
            "pvInt" => Val::Num(s.value.as_i64().unwrap_or(0) as f64),
            "pvStr" => Val::Str(s.format(None, false)),
            "pvSev" => Val::Num(s.severity as i32 as f64),
            "pvLegacySev" => Val::Num(legacy_severity(s.severity)),
            _ => Val::Num(0.0),
        }
    }
}

/// A severity as BOY numbered it: 0 none, 1 major, 2 minor, -1 invalid.
fn legacy_severity(s: pv::Severity) -> f64 {
    match s {
        pv::Severity::NoAlarm => 0.0,
        pv::Severity::Major => 1.0,
        pv::Severity::Minor => 2.0,
        pv::Severity::Invalid => -1.0,
    }
}

fn eval(e: &Expr, env: &Env) -> Val {
    match e {
        Expr::Lit(v) => v.clone(),
        Expr::Var(n) => env.var(n),
        Expr::Not(x) => Val::Bool(!eval(x, env).truthy()),
        Expr::Neg(x) => Val::Num(-eval(x, env).num()),
        Expr::Call(f, args) => {
            let args: Vec<f64> = args.iter().map(|a| eval(a, env).num()).collect();
            Val::Num(f.call(&args))
        }
        Expr::Bin(a, op, b) => {
            let a = eval(a, env);
            // Short-circuit the logical operators.
            match op.as_str() {
                "&&" => return Val::Bool(a.truthy() && eval(b, env).truthy()),
                "||" => return Val::Bool(a.truthy() || eval(b, env).truthy()),
                _ => {}
            }
            let b = eval(b, env);
            let strings = matches!((&a, &b), (Val::Str(_), Val::Str(_)));
            match op.as_str() {
                "==" if strings => Val::Bool(a == b),
                "!=" if strings => Val::Bool(a != b),
                "==" => Val::Bool(a.num() == b.num()),
                "!=" => Val::Bool(a.num() != b.num()),
                "<" => Val::Bool(a.num() < b.num()),
                ">" => Val::Bool(a.num() > b.num()),
                "<=" => Val::Bool(a.num() <= b.num()),
                ">=" => Val::Bool(a.num() >= b.num()),
                "+" if strings => Val::Str(a.to_text() + &b.to_text()),
                "+" => Val::Num(a.num() + b.num()),
                "-" => Val::Num(a.num() - b.num()),
                "*" => Val::Num(a.num() * b.num()),
                "/" => Val::Num(a.num() / b.num()),
                "%" => Val::Num(a.num() % b.num()),
                // Bitwise, on the integer parts as JavaScript does.
                "&" => Val::Num((a.num() as i64 & b.num() as i64) as f64),
                "|" => Val::Num((a.num() as i64 | b.num() as i64) as f64),
                "^" => Val::Num((a.num() as i64 ^ b.num() as i64) as f64),
                _ => Val::Bool(false),
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Op(String),
    Open,
    Close,
    Comma,
}

fn tokenize(s: &str) -> Option<Vec<Tok>> {
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
            out.push(Tok::Num(
                cs[start..i].iter().collect::<String>().parse().ok()?,
            ));
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
            if ["==", "!=", "<=", ">=", "&&", "||"].contains(&two.as_str()) {
                out.push(Tok::Op(two));
                i += 2;
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

fn parse(s: &str) -> Option<Expr> {
    let toks = tokenize(s)?;
    // A long chain (`1+1+1+...`) builds a deep left-leaning tree that the
    // evaluator and `Drop` walk recursively, so bound the length too.
    if toks.len() > MAX_TOKENS {
        return None;
    }
    let mut p = Parser {
        toks,
        pos: 0,
        depth: 0,
    };
    let e = p.binary(0)?;
    (p.pos == p.toks.len()).then_some(e)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    /// Nesting of parentheses and prefix operators, bounded by `MAX_DEPTH`.
    depth: usize,
}

/// Deepest nesting of `(`, `!` and `-` a rule expression may have.
const MAX_DEPTH: usize = 64;
/// Most tokens a rule expression may have.
const MAX_TOKENS: usize = 1024;

const LEVELS: &[&[&str]] = &[
    &["||"],
    &["&&"],
    &["|"],
    &["^"],
    &["&"],
    &["==", "!="],
    &["<", ">", "<=", ">="],
    &["+", "-"],
    &["*", "/", "%"],
];

impl Parser {
    fn binary(&mut self, level: usize) -> Option<Expr> {
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut lhs = self.binary(level + 1)?;
        while let Some(Tok::Op(op)) = self.toks.get(self.pos) {
            if !LEVELS[level].contains(&op.as_str()) {
                break;
            }
            let op = op.clone();
            self.pos += 1;
            let rhs = self.binary(level + 1)?;
            lhs = Expr::Bin(Box::new(lhs), op, Box::new(rhs));
        }
        Some(lhs)
    }

    fn unary(&mut self) -> Option<Expr> {
        if self.depth >= MAX_DEPTH {
            return None;
        }
        self.depth += 1;
        let e = self.unary_inner();
        self.depth -= 1;
        e
    }

    fn unary_inner(&mut self) -> Option<Expr> {
        let tok = self.toks.get(self.pos)?.clone();
        self.pos += 1;
        Some(match tok {
            Tok::Op(op) if op == "!" => Expr::Not(Box::new(self.unary()?)),
            Tok::Op(op) if op == "-" => Expr::Neg(Box::new(self.unary()?)),
            Tok::Num(n) => Expr::Lit(Val::Num(n)),
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
                let e = self.binary(0)?;
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
            args.push(self.binary(0)?);
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

    fn run(expr: &str, values: &[pv::Value]) -> Val {
        let states: Vec<pv::PvState> = values
            .iter()
            .map(|v| pv::PvState {
                connected: true,
                value: v.clone(),
                ..Default::default()
            })
            .collect();
        eval(&parse(expr).expect("parses"), &Env { states: &states })
    }

    /// Run `f` on a thread with a small stack, so unbounded recursion fails
    /// this test instead of passing on the test runner's larger stack.
    fn small_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
        std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap()
    }

    #[test]
    fn hostile_expressions_are_rejected_not_overflowed() {
        small_stack(|| {
            let nested = format!("{}1{}", "(".repeat(5000), ")".repeat(5000));
            assert!(parse(&nested).is_none());
            assert!(parse(&format!("{}1", "!".repeat(5000))).is_none());
            assert!(parse(&format!("{}1", "-".repeat(5000))).is_none());
            assert!(parse(&vec!["1"; 20000].join("+")).is_none());
            let calls = format!("{}1{}", "Math.abs(".repeat(5000), ")".repeat(5000));
            assert!(parse(&calls).is_none());
            assert!(parse(&format!("max({})", vec!["1"; 20000].join(","))).is_none());
            // Reasonable nesting still works.
            let ok = format!("{}1{}", "(".repeat(30), ")".repeat(30));
            assert!(parse(&ok).is_some());
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
        assert_eq!(run("pv0 | 1 ^ 3", &[Long(4)]), Val::Num(6.0));
    }

    #[test]
    fn legacy_severities_use_boy_numbers() {
        let state = |severity| pv::PvState {
            connected: true,
            severity,
            ..Default::default()
        };
        let states = [state(pv::Severity::Major), state(pv::Severity::Invalid)];
        let env = Env { states: &states };
        let ev = |e: &str| eval(&parse(e).unwrap(), &env);
        assert_eq!(ev("pvLegacySev0"), Val::Num(1.0));
        assert_eq!(ev("pvSev0"), Val::Num(2.0));
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
        let phoebus = CompiledRule::new(&rule(true), &|t| t.to_string(), &pvs);
        assert_eq!(phoebus.evaluate(), Outcome::Keep);
        let boy = CompiledRule::new(&rule(false), &|t| t.to_string(), &pvs);
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
        let compiled = CompiledRule::new(&rule(true), &|t| t.to_string(), &pvs);
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
            assert!(parse(bad).is_none(), "{bad}");
        }
        assert!(run("Math.min(pv0, 1) == 1 && max(1, 2) > 1", &both).truthy());
        assert!(parse("Math.nope(1)").is_none());
        assert!(parse("Math.min(1,").is_none());
        assert!(parse("Math.min(1 2)").is_none());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("pv0 ==").is_none());
        assert!(parse("pv0 = 1").is_none());
        assert!(parse("(pv0").is_none());
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
