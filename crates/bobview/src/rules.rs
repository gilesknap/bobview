//! Phoebus rules: expressions over a widget's rule PVs that override one property.
//!
//! The expression language covers what rule editors produce: `pv0`, `pvStr0`,
//! `pvInt0`, `pvSev0`, numbers, strings, `true`/`false`, comparison,
//! arithmetic, `&&`/`and`, `||`/`or`, `!`/`not` and parentheses.

use bob::{Element, Rule};
use pv::{Pv, PvManager};

pub struct CompiledRule {
    pub prop_id: String,
    out_exp: bool,
    exprs: Vec<(Option<Expr>, Element)>,
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
    pub fn new(rule: &Rule, expand: &dyn Fn(&str) -> String, pvs: &PvManager) -> Self {
        let exprs = rule
            .exprs
            .iter()
            .map(|e| {
                let parsed = parse(&e.condition);
                if parsed.is_none() {
                    log::warn!("rule '{}': cannot parse '{}'", rule.name, e.condition);
                }
                (parsed, e.value.clone())
            })
            .collect();
        Self {
            prop_id: rule.prop_id.clone(),
            out_exp: rule.out_exp,
            exprs,
            pvs: rule
                .pv_names
                .iter()
                .map(|n| pvs.subscribe(&expand(n)))
                .collect(),
        }
    }

    pub fn evaluate(&self) -> Outcome {
        let states: Vec<pv::PvState> = self.pvs.iter().map(|p| p.state()).collect();
        // Phoebus does not evaluate a rule until all its PVs have values.
        if states.iter().any(|s| !s.connected) {
            return Outcome::Keep;
        }
        let env = Env { states: &states };
        for (expr, value) in &self.exprs {
            let Some(expr) = expr else { continue };
            if self.out_exp {
                let v = eval(expr, &env);
                return Outcome::Set(Element {
                    name: self.prop_id.clone(),
                    text: v.to_text(),
                    ..Default::default()
                });
            }
            if eval(expr, &env).truthy() {
                let mut el = value.clone();
                el.name = self.prop_id.clone();
                return Outcome::Set(el);
            }
        }
        Outcome::Keep
    }
}

/// Apply rule outcomes to a widget's properties.
pub fn apply(props: &mut Element, outcomes: &[(String, Outcome)]) {
    for (prop, outcome) in outcomes {
        if let Outcome::Set(el) = outcome {
            match props.children.iter_mut().find(|c| &c.name == prop) {
                Some(existing) => *existing = el.clone(),
                None => props.children.push(el.clone()),
            }
        }
    }
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
            "pvSev" | "pvLegacySev" => Val::Num(s.severity as i32 as f64),
            _ => Val::Num(0.0),
        }
    }
}

fn eval(e: &Expr, env: &Env) -> Val {
    match e {
        Expr::Lit(v) => v.clone(),
        Expr::Var(n) => env.var(n),
        Expr::Not(x) => Val::Bool(!eval(x, env).truthy()),
        Expr::Neg(x) => Val::Num(-eval(x, env).num()),
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
            while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_') {
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
        } else {
            let two: String = cs[i..(i + 2).min(cs.len())].iter().collect();
            if ["==", "!=", "<=", ">=", "&&", "||"].contains(&two.as_str()) {
                out.push(Tok::Op(two));
                i += 2;
            } else if "<>!+-*/%".contains(c) {
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
    let mut p = Parser { toks, pos: 0 };
    let e = p.binary(0)?;
    (p.pos == p.toks.len()).then_some(e)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

const LEVELS: &[&[&str]] = &[
    &["||"],
    &["&&"],
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
}
