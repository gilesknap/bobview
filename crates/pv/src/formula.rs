//! Formula PVs: `=expr` as in CS-Studio BOY, or `eq://expr` as in Phoebus.
//!
//! An expression of numbers, `"strings"` and PVs, which are named by
//! `pv("name")`, `'name'`, `` `name` `` or a bare name such as `X:Y`, with
//! arithmetic (`+ - * / % ^`), comparisons, `! && ||`, `c ? a : b`, the
//! string functions `concat` and `toString`, and the usual maths functions.
//!
//! A PV's name may itself be computed, as the DLS diagnostics screens do to
//! pick a PV by a `loc://` PV that a menu sets:
//! `pv(concat("SR01C-DI-PIN-17:", toString('loc://mode<VString>("RATE")')))`.
//! The formula then follows whichever PV the name names.
//!
//! There is no task: the formula is evaluated again whenever a PV it reads
//! changes, by the thread that changed it once that has let go of the PV's
//! lock: a listener only leaves the input's state. So evaluating, which
//! takes the formula's own lock and subscribes to inputs, holds no other
//! PV's lock, and formulas reading each other cannot wait for each other.
//! One thread at a time evaluates a formula; an input changing meanwhile
//! leaves its state for that thread.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use parking_lot::Mutex;

use crate::{Inner, Listener, Pv, PvManager, PvName, PvState, Severity, Shared, Value};

/// A parsed formula.
#[derive(Clone, Debug, PartialEq)]
enum Expr {
    Num(f64),
    Str(String),
    /// A PV named in the text.
    Pv(String),
    /// A PV whose name is computed.
    DynPv(Box<Expr>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Binary(Op, Box<Expr>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    /// A quoted or bare PV name, or a function's name before `(`.
    Name(String),
    Ident(String),
    Sym(&'static str),
}

const SYMS: [&str; 20] = [
    "&&", "||", "==", "!=", "<=", ">=", "<", ">", "+", "-", "*", "/", "%", "^", "!", "?", ":", "(",
    ")", ",",
];

fn lex(text: &str) -> Result<Vec<Tok>, String> {
    let mut toks = Vec::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c.is_whitespace() {
            rest = &rest[c.len_utf8()..];
        } else if c.is_ascii_digit()
            || (c == '.' && rest[1..].starts_with(|d: char| d.is_ascii_digit()))
        {
            let mut end = 0;
            let b = rest.as_bytes();
            while end < b.len() && (b[end].is_ascii_digit() || b[end] == b'.') {
                end += 1;
            }
            // An exponent, as in 1e6 or 2.5E-3.
            if end < b.len() && (b[end] == b'e' || b[end] == b'E') {
                let mut e = end + 1;
                if e < b.len() && (b[e] == b'+' || b[e] == b'-') {
                    e += 1;
                }
                if e < b.len() && b[e].is_ascii_digit() {
                    end = e;
                    while end < b.len() && b[end].is_ascii_digit() {
                        end += 1;
                    }
                }
            }
            let n = rest[..end]
                .parse()
                .map_err(|_| format!("bad number '{}'", &rest[..end]))?;
            toks.push(Tok::Num(n));
            rest = &rest[end..];
        } else if c == '"' {
            let mut s = String::new();
            let mut chars = rest[1..].char_indices();
            let end = loop {
                match chars.next() {
                    Some((i, '"')) => break i + 2,
                    Some((_, '\\')) => match chars.next() {
                        Some((_, 'n')) => s.push('\n'),
                        Some((_, 't')) => s.push('\t'),
                        Some((_, e)) => s.push(e),
                        None => return Err("unterminated string".into()),
                    },
                    Some((_, ch)) => s.push(ch),
                    None => return Err("unterminated string".into()),
                }
            };
            toks.push(Tok::Str(s));
            rest = &rest[end..];
        } else if c == '\'' || c == '`' {
            // A PV name, which may hold double quotes: 'loc://x<VString>("a")'.
            let end = rest[1..]
                .find(c)
                .ok_or_else(|| format!("unterminated PV name {rest}"))?;
            toks.push(Tok::Name(rest[1..end + 1].trim().to_string()));
            rest = &rest[end + 2..];
        } else if c.is_ascii_alphabetic() || c == '_' {
            let end = rest
                .find(|ch: char| !(ch.is_ascii_alphanumeric() || "_:.".contains(ch)))
                .unwrap_or(rest.len());
            let word = &rest[..end];
            rest = &rest[end..];
            if rest.trim_start().starts_with('(') {
                toks.push(Tok::Ident(word.to_string()));
            } else {
                toks.push(Tok::Name(word.to_string()));
            }
        } else if let Some(sym) = SYMS.iter().find(|s| rest.starts_with(**s)) {
            toks.push(Tok::Sym(sym));
            rest = &rest[sym.len()..];
        } else {
            return Err(format!("unexpected '{c}'"));
        }
    }
    Ok(toks)
}

/// A recursive descent parser, lowest precedence first.
struct Parser {
    toks: Vec<Tok>,
    at: usize,
    /// Nesting of brackets, `?:`, prefix operators and `^`, bounded by
    /// `MAX_DEPTH`.
    depth: usize,
}

/// Deepest nesting of brackets, `?:`, prefix operators and `^` a formula
/// may have. The parser, the evaluator and `Drop` all recurse once per level,
/// and a display from the web is not trusted to keep within the stack.
const MAX_DEPTH: usize = 64;
/// Most tokens a formula may have: a long chain (`1+1+1+...`) builds a deep
/// left-leaning tree that the evaluator and `Drop` walk recursively.
const MAX_TOKENS: usize = 1024;

impl Parser {
    /// `f`, one level deeper.
    fn nested(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<Expr, String>,
    ) -> Result<Expr, String> {
        if self.depth >= MAX_DEPTH {
            return Err("formula nests too deeply".into());
        }
        self.depth += 1;
        let e = f(self);
        self.depth -= 1;
        e
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    fn eat(&mut self, sym: &str) -> bool {
        let found = matches!(self.peek(), Some(Tok::Sym(s)) if *s == sym);
        if found {
            self.at += 1;
        }
        found
    }

    fn expect(&mut self, sym: &str) -> Result<(), String> {
        if self.eat(sym) {
            Ok(())
        } else {
            Err(format!("expected '{sym}'"))
        }
    }

    fn cond(&mut self) -> Result<Expr, String> {
        self.nested(Self::cond_inner)
    }

    fn cond_inner(&mut self) -> Result<Expr, String> {
        let c = self.binary(0)?;
        if !self.eat("?") {
            return Ok(c);
        }
        let a = self.cond()?;
        self.expect(":")?;
        let b = self.cond()?;
        Ok(Expr::Cond(Box::new(c), Box::new(a), Box::new(b)))
    }

    /// Binary operators at `level` and above.
    fn binary(&mut self, level: usize) -> Result<Expr, String> {
        const LEVELS: [&[(&str, Op)]; 6] = [
            &[("||", Op::Or)],
            &[("&&", Op::And)],
            &[("==", Op::Eq), ("!=", Op::Ne)],
            &[("<=", Op::Le), (">=", Op::Ge), ("<", Op::Lt), (">", Op::Gt)],
            &[("+", Op::Add), ("-", Op::Sub)],
            &[("*", Op::Mul), ("/", Op::Div), ("%", Op::Rem)],
        ];
        let Some(ops) = LEVELS.get(level) else {
            return self.unary();
        };
        let mut left = self.binary(level + 1)?;
        'more: loop {
            for (sym, op) in ops.iter() {
                if self.eat(sym) {
                    let right = self.binary(level + 1)?;
                    left = Expr::Binary(*op, Box::new(left), Box::new(right));
                    continue 'more;
                }
            }
            return Ok(left);
        }
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.eat("-") {
            Ok(Expr::Neg(Box::new(self.nested(Self::unary)?)))
        } else if self.eat("+") {
            self.nested(Self::unary)
        } else if self.eat("!") {
            Ok(Expr::Not(Box::new(self.nested(Self::unary)?)))
        } else {
            let base = self.primary()?;
            if self.eat("^") {
                let exp = self.nested(Self::unary)?;
                return Ok(Expr::Binary(Op::Pow, Box::new(base), Box::new(exp)));
            }
            Ok(base)
        }
    }

    fn primary(&mut self) -> Result<Expr, String> {
        let tok = self.peek().cloned().ok_or("unexpected end")?;
        self.at += 1;
        match tok {
            Tok::Num(n) => Ok(Expr::Num(n)),
            Tok::Str(s) => Ok(Expr::Str(s)),
            Tok::Name(n) => Ok(Expr::Pv(n)),
            Tok::Sym("(") => {
                let e = self.cond()?;
                self.expect(")")?;
                Ok(e)
            }
            Tok::Ident(f) => {
                self.expect("(")?;
                let mut args = Vec::new();
                if !self.eat(")") {
                    loop {
                        args.push(self.cond()?);
                        if self.eat(")") {
                            break;
                        }
                        self.expect(",")?;
                    }
                }
                call(f, args)
            }
            Tok::Sym(s) => Err(format!("unexpected '{s}'")),
        }
    }
}

/// A call of function `f`, checked here so a bad formula fails at once.
fn call(f: String, mut args: Vec<Expr>) -> Result<Expr, String> {
    let arity = |n: usize| {
        if args.len() == n {
            Ok(())
        } else {
            Err(format!("{f}() takes {n} arguments"))
        }
    };
    match f.as_str() {
        "pv" => {
            arity(1)?;
            Ok(match args.pop().unwrap() {
                Expr::Str(name) | Expr::Pv(name) => Expr::Pv(name),
                other => Expr::DynPv(Box::new(other)),
            })
        }
        "concat" | "min" | "max" => Ok(Expr::Call(f, args)),
        "atan2" | "hypot" | "pow" => {
            arity(2)?;
            Ok(Expr::Call(f, args))
        }
        _ if unary_fn(&f).is_some() || f == "toString" => {
            arity(1)?;
            Ok(Expr::Call(f, args))
        }
        _ => Err(format!("unknown function {f}()")),
    }
}

fn unary_fn(f: &str) -> Option<fn(f64) -> f64> {
    Some(match f {
        "abs" => f64::abs,
        "acos" => f64::acos,
        "asin" => f64::asin,
        "atan" => f64::atan,
        "cbrt" => f64::cbrt,
        "ceil" => f64::ceil,
        "cos" => f64::cos,
        "cosh" => f64::cosh,
        "exp" => f64::exp,
        "expm1" => f64::exp_m1,
        "floor" => f64::floor,
        "log" => f64::ln,
        "log10" => f64::log10,
        "log1p" => f64::ln_1p,
        "round" => f64::round,
        "signum" => f64::signum,
        "sin" => f64::sin,
        "sinh" => f64::sinh,
        "sqrt" => f64::sqrt,
        "tan" => f64::tan,
        "tanh" => f64::tanh,
        "toDegrees" => f64::to_degrees,
        "toRadians" => f64::to_radians,
        _ => return None,
    })
}

fn parse(text: &str) -> Result<Expr, String> {
    let toks = lex(text)?;
    if toks.len() > MAX_TOKENS {
        return Err(format!("formula has more than {MAX_TOKENS} tokens"));
    }
    let mut p = Parser {
        toks,
        at: 0,
        depth: 0,
    };
    let e = p.cond()?;
    match p.peek() {
        None => Ok(e),
        Some(t) => Err(format!("unexpected {t:?}")),
    }
}

/// An intermediate value.
#[derive(Clone, Debug)]
enum Val {
    Num(f64),
    Str(String),
    /// A PV's state, kept whole so `=pv(...)` alone passes on its
    /// metadata (enum labels, precision, units, limits).
    Pv(Box<PvState>),
}

impl Val {
    fn num(&self) -> f64 {
        match self {
            Val::Num(n) => *n,
            Val::Str(s) => s.trim().parse().unwrap_or(f64::NAN),
            Val::Pv(s) => s.value.as_f64().unwrap_or(f64::NAN),
        }
    }

    fn text(&self) -> String {
        match self {
            Val::Num(n) => n.to_string(),
            Val::Str(s) => s.clone(),
            Val::Pv(s) => s.format(None, false),
        }
    }

    fn is_str(&self) -> bool {
        match self {
            Val::Str(_) => true,
            Val::Pv(s) => matches!(s.value, Value::Str(_) | Value::StrArray(_)),
            Val::Num(_) => false,
        }
    }
}

fn bool_val(b: bool) -> Val {
    Val::Num(if b { 1.0 } else { 0.0 })
}

/// One evaluation over the latest states of the PVs read.
struct Eval<'a> {
    /// By [`PvName::key`], so every spelling of a PV reads one state.
    states: &'a HashMap<String, PvState>,
    /// Every PV the formula reads now: its key, and its name as written.
    wanted: Vec<(String, String)>,
    severity: Severity,
    timestamp: Option<web_time::SystemTime>,
}

/// Why a formula has no value: a PV it reads has none yet.
struct Missing;

impl Eval<'_> {
    fn read(&mut self, name: String) -> Result<Val, Missing> {
        let key = PvName::parse(&name).key();
        let state = self
            .states
            .get(&key)
            .filter(|s| s.connected && s.value != Value::None);
        self.wanted.push((key, name));
        let state = state.ok_or(Missing)?;
        self.severity = self.severity.max(state.severity);
        self.timestamp = self.timestamp.max(state.timestamp);
        Ok(Val::Pv(Box::new(state.clone())))
    }

    /// Every argument is evaluated, even when one is missing, so `wanted`
    /// holds every PV a name can be computed for.
    fn eval(&mut self, e: &Expr) -> Result<Val, Missing> {
        Ok(match e {
            Expr::Num(n) => Val::Num(*n),
            Expr::Str(s) => Val::Str(s.clone()),
            Expr::Pv(name) => return self.read(name.clone()),
            Expr::DynPv(name) => {
                let name = self.eval(name)?.text();
                return self.read(name.trim().to_string());
            }
            Expr::Neg(a) => Val::Num(-self.eval(a)?.num()),
            Expr::Not(a) => bool_val(self.eval(a)?.num() == 0.0),
            Expr::Cond(c, a, b) => {
                let (c, a, b) = (self.eval(c), self.eval(a), self.eval(b));
                if c?.num() != 0.0 { a? } else { b? }
            }
            Expr::Binary(op, a, b) => {
                let (a, b) = (self.eval(a), self.eval(b));
                binary(*op, a?, b?)
            }
            Expr::Call(f, args) => {
                let args: Vec<_> = args.iter().map(|a| self.eval(a)).collect();
                let args = args.into_iter().collect::<Result<Vec<_>, _>>()?;
                let nums = || args.iter().map(Val::num);
                match f.as_str() {
                    "concat" => Val::Str(args.iter().map(Val::text).collect()),
                    "toString" => Val::Str(args[0].text()),
                    "min" => Val::Num(nums().fold(f64::NAN, f64::min)),
                    "max" => Val::Num(nums().fold(f64::NAN, f64::max)),
                    "atan2" => Val::Num(args[0].num().atan2(args[1].num())),
                    "hypot" => Val::Num(args[0].num().hypot(args[1].num())),
                    "pow" => Val::Num(args[0].num().powf(args[1].num())),
                    _ => Val::Num(unary_fn(f).expect("checked by the parser")(args[0].num())),
                }
            }
        })
    }
}

fn binary(op: Op, a: Val, b: Val) -> Val {
    let strings = a.is_str() && b.is_str();
    match op {
        Op::Add if a.is_str() || b.is_str() => Val::Str(a.text() + &b.text()),
        Op::Eq if strings => bool_val(a.text() == b.text()),
        Op::Ne if strings => bool_val(a.text() != b.text()),
        _ => {
            let (x, y) = (a.num(), b.num());
            match op {
                Op::Add => Val::Num(x + y),
                Op::Sub => Val::Num(x - y),
                Op::Mul => Val::Num(x * y),
                Op::Div => Val::Num(x / y),
                Op::Rem => Val::Num(x % y),
                Op::Pow => Val::Num(x.powf(y)),
                Op::Lt => bool_val(x < y),
                Op::Le => bool_val(x <= y),
                Op::Gt => bool_val(x > y),
                Op::Ge => bool_val(x >= y),
                Op::Eq => bool_val(x == y),
                Op::Ne => bool_val(x != y),
                Op::And => bool_val(x != 0.0 && y != 0.0),
                Op::Or => bool_val(x != 0.0 || y != 0.0),
            }
        }
    }
}

/// Most times a formula is evaluated in one run of a thread's left work.
const MAX_EVALUATIONS: u32 = 1000;

/// Whether two states are the same, a NaN value being the same as itself.
fn same(a: &PvState, b: &PvState) -> bool {
    let nan = |s: &PvState| matches!(s.value, Value::Double(v) if v.is_nan());
    if nan(a) && nan(b) {
        let strip = |s: &PvState| PvState {
            value: Value::None,
            ..s.clone()
        };
        strip(a) == strip(b)
    } else {
        a == b
    }
}

/// A PV a formula reads, with the listener feeding its state to the formula.
type Input = (Pv, Listener);

/// A formula PV: its inputs, and the state its value is written to.
pub(crate) struct Formula {
    /// `None` when the text does not parse.
    expr: Option<Expr>,
    shared: Arc<Shared>,
    /// For subscribing to the inputs: weak, so a formula does not keep the
    /// manager alive.
    manager: Weak<Inner>,
    me: Weak<Formula>,
    key: String,
    /// The latest state of each input, by its key, as the evaluating thread
    /// last took them from `pending`.
    states: Mutex<HashMap<String, PvState>>,
    /// The inputs by key, with the listener feeding `pending` from each.
    inputs: Mutex<HashMap<String, Input>>,
    /// Input states not yet evaluated. Never held while calling out.
    pending: Mutex<HashMap<String, PvState>>,
    /// Whether an input changed since the last evaluation began, whether
    /// an evaluation is queued to run outside listeners, and whether a
    /// thread is evaluating.
    dirty: AtomicBool,
    queued: AtomicBool,
    busy: AtomicBool,
    /// Whether reading itself or not settling has been reported, so it is
    /// said once.
    warned: AtomicBool,
    /// The thread's run of left work it last evaluated in, and how many
    /// times in that run.
    runs: Mutex<(u64, u32)>,
}

impl Formula {
    pub(crate) fn new(name: &PvName, shared: Arc<Shared>, manager: Weak<Inner>) -> Arc<Self> {
        let expr = match parse(&name.pv) {
            Ok(e) => Some(e),
            Err(e) => {
                log::warn!("{name}: {e}");
                shared.update(|s| s.error = Some(format!("bad formula: {e}")));
                None
            }
        };
        Arc::new_cyclic(|me| Self {
            expr,
            shared,
            manager,
            me: me.clone(),
            key: name.key(),
            states: Mutex::new(HashMap::new()),
            inputs: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            dirty: AtomicBool::new(false),
            queued: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            warned: AtomicBool::new(false),
            runs: Mutex::new((0, 0)),
        })
    }

    /// Subscribe to the inputs. Not done in [`Formula::new`], which is
    /// called with the manager's subscriptions locked.
    pub(crate) fn start(&self) {
        self.changed();
    }

    /// Evaluate again once this thread holds no PV's lock, unless a thread
    /// is evaluating already: then it will, as it finds `dirty` set.
    fn changed(&self) {
        self.dirty.store(true, Ordering::SeqCst);
        if self.queued.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.me.clone();
        crate::outside_listeners(Box::new(move || {
            if let Some(me) = me.upgrade() {
                me.queued.store(false, Ordering::SeqCst);
                me.evaluate();
            }
        }));
    }

    /// Whether this is one evaluation too many in this thread's run of left
    /// work: formulas reading each other whose values never settle (NaN,
    /// or 1 - each other) would otherwise evaluate each other for ever. It
    /// waits for the next change of an input from outside the loop.
    fn looping(&self) -> bool {
        let run = crate::drain_number();
        let mut runs = self.runs.lock();
        if runs.0 != run {
            *runs = (run, 0);
        }
        runs.1 += 1;
        let looping = runs.1 > MAX_EVALUATIONS;
        if looping && !self.warned.swap(true, Ordering::Relaxed) {
            log::warn!("{}: formulas reading each other do not settle", self.key);
        }
        looping
    }

    /// Evaluate while `dirty`, unless another thread is evaluating.
    fn evaluate(&self) {
        /// Lets go of the formula, even if evaluating it panicked.
        struct Busy<'a>(&'a Formula);
        impl Drop for Busy<'_> {
            fn drop(&mut self) {
                self.0.busy.store(false, Ordering::SeqCst);
            }
        }
        while self
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let busy = Busy(self);
            while self.dirty.swap(false, Ordering::SeqCst) {
                if self.looping() {
                    break;
                }
                self.refresh();
            }
            drop(busy);
            // A change after the last check but before letting go is ours.
            if !self.dirty.load(Ordering::SeqCst) {
                break;
            }
        }
    }

    /// Evaluate, publish the result, and follow the PVs it now reads. Only
    /// one thread at a time, from [`Formula::evaluate`].
    fn refresh(&self) {
        let Some(expr) = &self.expr else { return };
        let evaluated = {
            let mut states = self.states.lock();
            states.extend(self.pending.lock().drain());
            let mut ev = Eval {
                states: &states,
                wanted: Vec::new(),
                severity: Severity::NoAlarm,
                timestamp: None,
            };
            let result = ev.eval(expr);
            let (severity, timestamp) = (ev.severity, ev.timestamp);
            let old = self.shared.snapshot();
            let new = match result {
                Ok(Val::Pv(pv)) => PvState {
                    severity,
                    writable: false,
                    writable_known: false,
                    write_error: None,
                    ..*pv
                },
                Ok(v) => PvState {
                    connected: true,
                    value: match v {
                        Val::Str(t) => Value::Str(t),
                        v => Value::Double(v.num()),
                    },
                    severity,
                    timestamp,
                    ..PvState::default()
                },
                Err(Missing) => PvState {
                    connected: false,
                    ..old.clone()
                },
            };
            (old, new, ev.wanted)
        };
        let (old, new, wanted) = evaluated;
        // Only a change goes out, so formulas reading each other settle.
        if !same(&new, &old) {
            self.shared.update(|s| *s = new);
        }
        self.follow(wanted);
    }

    /// Subscribe to the PVs in `wanted` not yet subscribed, and drop those
    /// no longer read (when a computed name changes).
    fn follow(&self, wanted: Vec<(String, String)>) {
        let mut inputs = self.inputs.lock();
        // Keyed, so two spellings of one PV are one input; the one giving a
        // local PV's initial value is the one subscribed.
        let mut names: HashMap<String, String> = HashMap::new();
        for (key, name) in wanted {
            // Its own updates would evaluate it again for ever.
            if key == self.key {
                if !self.warned.swap(true, Ordering::Relaxed) {
                    log::warn!("{}: a formula cannot read itself", self.key);
                }
                continue;
            }
            let has_args = |n: &str| PvName::parse(n).args().is_some();
            match names.get(&key) {
                Some(n) if has_args(n) || !has_args(&name) => {}
                _ => {
                    names.insert(key, name);
                }
            }
        }
        let new: Vec<(String, String)> = names
            .iter()
            .filter(|(k, _)| !inputs.contains_key(*k))
            .map(|(k, n)| (k.clone(), n.clone()))
            .collect();
        let gone: Vec<String> = inputs
            .keys()
            .filter(|k| !names.contains_key(*k))
            .cloned()
            .collect();
        let old: Vec<Input> = gone.iter().filter_map(|k| inputs.remove(k)).collect();
        drop(inputs);
        if !old.is_empty() {
            self.states.lock().retain(|k, _| names.contains_key(k));
            self.pending.lock().retain(|k, _| names.contains_key(k));
        }
        drop(old);
        if new.is_empty() {
            return;
        }
        self.subscribe(new);
    }

    /// Subscribe to the inputs `new`, by key and name, and listen to each.
    fn subscribe(&self, new: Vec<(String, String)>) {
        let Some(inner) = self.manager.upgrade() else {
            return;
        };
        let manager = PvManager { inner };
        for (key, name) in new {
            // Subscribed meanwhile, by an evaluation after the one asking.
            if self.inputs.lock().contains_key(&key) {
                continue;
            }
            let pv = manager.subscribe(&name);
            let me = self.me.clone();
            let state_key = key.clone();
            // Called under the input's lock: it only leaves the state, for
            // this thread to evaluate once it has let go.
            let listener: Listener = Arc::new(move |state: &PvState| {
                if let Some(me) = me.upgrade() {
                    me.pending.lock().insert(state_key.clone(), state.clone());
                    me.changed();
                }
            });
            self.inputs
                .lock()
                .insert(key, (pv.clone(), listener.clone()));
            pv.listen(&listener);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(text: &str, states: &[(&str, Value)]) -> (Option<Val>, Vec<String>) {
        let states = states
            .iter()
            .map(|(n, v)| {
                let s = PvState {
                    connected: true,
                    value: v.clone(),
                    ..PvState::default()
                };
                (PvName::parse(n).key(), s)
            })
            .collect();
        let mut ev = Eval {
            states: &states,
            wanted: Vec::new(),
            severity: Severity::NoAlarm,
            timestamp: None,
        };
        let v = ev.eval(&parse(text).unwrap()).ok();
        (v, ev.wanted.into_iter().map(|(_, name)| name).collect())
    }

    fn num(text: &str) -> f64 {
        eval(text, &[]).0.unwrap().num()
    }

    #[test]
    fn arithmetic_and_logic() {
        assert_eq!(num("1 + 2 * 3"), 7.0);
        assert_eq!(num("(1 + 2) * 3"), 9.0);
        assert_eq!(num("124913500/2.5e2"), 499654.0);
        assert_eq!(num("-2^2"), -4.0);
        assert_eq!(num("7 % 4 - 1"), 2.0);
        assert_eq!(num("1 < 2 && 3 >= 3 || 0"), 1.0);
        assert_eq!(num("!(1 == 1)"), 0.0);
        assert_eq!(num("0 ? 5 : 1 ? 6 : 7"), 6.0);
        assert_eq!(num("max(1, 4, 2) + abs(-1) + sqrt(16)"), 9.0);
        assert_eq!(
            eval("concat(\"a\", 1.5, \"b\")", &[]).0.unwrap().text(),
            "a1.5b"
        );
        assert_eq!(num("\"a\" == \"a\""), 1.0);
    }

    #[test]
    fn reads_pvs_by_any_spelling() {
        let pvs = [("X:Y", Value::Double(2.0)), ("loc://m(1)", Value::Long(3))];
        let (v, wanted) = eval("(pv(\"X:Y\")*1000) + 'loc://m(1)' + `X:Y` + X:Y", &pvs);
        assert_eq!(v.unwrap().num(), 2007.0);
        assert_eq!(wanted, ["X:Y", "loc://m(1)", "X:Y", "X:Y"]);
    }

    #[test]
    fn computes_pv_names() {
        let text = r#"pv(concat("SR01C-DI-PIN-17:", toString('loc://ll<VString>("RATE")')))"#;
        let mode = ("loc://ll<VString>(\"RATE\")", Value::Str("LOG".into()));
        let (v, wanted) = eval(text, std::slice::from_ref(&mode));
        // Not there yet, but now wanted.
        assert!(v.is_none());
        assert_eq!(
            wanted,
            ["loc://ll<VString>(\"RATE\")", "SR01C-DI-PIN-17:LOG"]
        );
        let (v, _) = eval(text, &[mode, ("SR01C-DI-PIN-17:LOG", Value::Double(5.0))]);
        assert_eq!(v.unwrap().num(), 5.0);
    }

    #[test]
    fn missing_inputs_are_still_wanted() {
        let (v, wanted) = eval("pv(\"A\") + pv(\"B\")", &[("B", Value::Double(1.0))]);
        assert!(v.is_none());
        assert_eq!(wanted, ["A", "B"]);
    }

    #[test]
    fn bad_formulas_fail_to_parse() {
        for text in ["1 +", "nope(1)", "pv(1, 2)", "(1", "'x", "1 $ 2"] {
            assert!(parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn hostile_formulas_fail_to_parse_not_overflow() {
        // On a known stack, so unbounded recursion fails this test instead
        // of passing on a larger one. Release builds need a tenth of this
        // for the deepest formulas allowed.
        let deep = std::thread::Builder::new()
            .stack_size(4 * 1024 * 1024)
            .spawn(|| {
                // Nested past the cap, both within the token limit and far
                // beyond it.
                for n in [100, 100_000] {
                    for text in [
                        format!("{}1{}", "(".repeat(n), ")".repeat(n)),
                        format!("{}1", "-".repeat(n)),
                        format!("{}1", "!".repeat(n)),
                        format!("{}1", "+".repeat(n)),
                        "2^".repeat(n) + "2",
                        "1?".repeat(n) + "1" + &":1".repeat(n),
                        "1?1:".repeat(n) + "1",
                        "abs(".repeat(n) + "1" + &")".repeat(n),
                    ] {
                        assert!(parse(&text).is_err(), "{}", &text[..20]);
                    }
                }
                // Long, flat formulas.
                assert!(parse(&vec!["1"; 100_000].join("+")).is_err());
                assert!(parse(&format!("max({})", vec!["1"; 100_000].join(","))).is_err());
                // Reasonable nesting still parses.
                let ok = format!("{}1{}", "(".repeat(30), ")".repeat(30));
                assert_eq!(parse(&ok), Ok(Expr::Num(1.0)));
                assert!(parse(&format!("{}1", "-".repeat(30))).is_ok());
                // The longest chain allowed evaluates, and is dropped.
                assert_eq!(num(&vec!["1"; 512].join("+")), 512.0);
            })
            .unwrap()
            .join();
        assert!(deep.is_ok());
    }

    #[test]
    fn a_formula_pv_follows_its_inputs() {
        let pvs = PvManager::new();
        let f = pvs.subscribe("=(pv(\"loc://fa(2)\")*1000) + 'loc://fb(1)'");
        assert_eq!(f.state().value, Value::Double(2001.0));
        assert!(f.connected());
        pvs.subscribe("loc://fa").write("3");
        assert_eq!(f.state().value, Value::Double(3001.0));
        assert!(f.state().format(None, false).starts_with("3001"));
    }

    #[test]
    fn a_formula_pv_switches_with_a_computed_name() {
        let pvs = PvManager::new();
        let text = r#"=pv(concat("loc://fsw_", toString('loc://fsw_mode<VString>("a")')))"#;
        let a = pvs.subscribe("loc://fsw_a(1)");
        let b = pvs.subscribe("loc://fsw_b<VString>(\"bee\")");
        let f = pvs.subscribe(text);
        assert_eq!(f.state().value, Value::Double(1.0));
        pvs.subscribe("loc://fsw_mode").write("b");
        // Passed through whole: a string PV stays a string.
        assert_eq!(f.state().value, Value::Str("bee".into()));
        a.write("5");
        b.write("buzz");
        assert_eq!(f.state().value, Value::Str("buzz".into()));
        // Only the PV now named is followed.
        let Some(formula) = &f.0.formula else {
            panic!()
        };
        let inputs = formula.inputs.lock();
        assert!(inputs.contains_key("loc://fsw_b") && !inputs.contains_key("loc://fsw_a"));
    }

    #[test]
    fn a_formula_reads_two_spellings_of_a_pv_as_one() {
        // Each spelling once deadlocked, listening to the PV from its own
        // listener.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let pvs = PvManager::new();
            let f = pvs.subscribe("='loc://fsp(1)' + 'loc://fsp' + pv(\" loc://fsp\")");
            let _ = tx.send(f.state().value);
        });
        let value = rx.recv_timeout(std::time::Duration::from_secs(5));
        assert_eq!(value, Ok(Value::Double(3.0)));
    }

    #[test]
    fn a_computed_name_switching_on_two_threads_does_not_deadlock() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let pvs = PvManager::new();
            let x = pvs.subscribe("loc://fdx(1)");
            let y = pvs.subscribe("loc://fdy(2)");
            let sel = pvs.subscribe("loc://fdsel<VString>(\"loc://fdx\")");
            let f = pvs.subscribe("=pv(toString('loc://fdsel')) + 0 * 'loc://fdz(0)'");
            let z = pvs.subscribe("loc://fdz");
            let flip = std::thread::spawn(move || {
                for i in 0..2000 {
                    sel.write(if i % 2 == 0 { "loc://fdy" } else { "loc://fdx" });
                }
                sel.write("loc://fdy");
            });
            for i in 0..2000 {
                z.write(i.to_string());
                x.write("1");
            }
            flip.join().unwrap();
            y.write("2");
            let _ = tx.send(f.state().value);
        });
        let value = rx.recv_timeout(std::time::Duration::from_secs(20));
        assert_eq!(value, Ok(Value::Double(2.0)));
    }

    #[test]
    fn a_formula_reading_a_formula_keeps_up() {
        let pvs = PvManager::new();
        let g = pvs.subscribe("=pv(\"=2*'loc://fch(1)'\") + 100");
        assert_eq!(g.state().value, Value::Double(102.0));
        let x = pvs.subscribe("loc://fch");
        for v in [5.0, 7.0, 9.0] {
            x.write(v.to_string());
            assert_eq!(g.state().value, Value::Double(2.0 * v + 100.0));
        }
    }

    #[test]
    fn formulas_reading_each_other_settle() {
        // NaN is never equal to itself, and 1 - B with B = A never settles;
        // both once evaluated each other for ever.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let pvs = PvManager::new();
            let a = "=1 ? 0/0 : pv(toString('loc://fsna'))";
            let b = "=1 ? 0/0 : pv(toString('loc://fsnb'))";
            pvs.subscribe("loc://fsnb<VString>(\"x\")").write(a);
            pvs.subscribe("loc://fsna<VString>(\"x\")").write(b);
            let _nan = pvs.subscribe(a);
            let a = "='loc://fso(1)' ? 1 : 1 - pv(toString('loc://fsob'))";
            let b = "=pv(toString('loc://fsoa')) + 0";
            pvs.subscribe("loc://fsoa<VString>(\"x\")").write(a);
            pvs.subscribe("loc://fsob<VString>(\"x\")").write(b);
            let (_a, _b) = (pvs.subscribe(a), pvs.subscribe(b));
            pvs.subscribe("loc://fso").write("0");
            let _ = tx.send(());
        });
        assert!(rx.recv_timeout(std::time::Duration::from_secs(10)).is_ok());
    }

    #[test]
    fn formulas_naming_each_other_do_not_deadlock() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let pvs = PvManager::new();
            let _n = pvs.subscribe("loc://fcn<VString>(\"=pv(toString('loc://fcm'))\")");
            let _m = pvs.subscribe("loc://fcm<VString>(\"=pv(toString('loc://fcn'))\")");
            let f = pvs.subscribe("=pv(toString('loc://fcn'))");
            let _ = tx.send(f.connected());
        });
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(5)),
            Ok(false)
        );
    }

    #[test]
    fn a_formula_waits_for_its_inputs_and_cannot_be_written() {
        let pvs = PvManager::new();
        let f = pvs.subscribe("=pv(\"sim://nope\") + 1");
        assert!(!f.connected());
        f.write("1");
        assert!(pvs.notice().1.contains("cannot be written"));
        let bad = pvs.subscribe("=1 +");
        assert!(bad.state().error.unwrap().contains("bad formula"));
    }
}
