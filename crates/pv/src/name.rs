use std::fmt;

/// Which backend serves a PV.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scheme {
    Ca,
    Pva,
    Loc,
    Sim,
    /// A formula over other PVs: `=expr` as in BOY, or `eq://expr`.
    Formula,
    /// A kind of PV bobview does not serve, such as `sys://` or an unknown
    /// `scheme://` prefix: it never connects, rather than being looked up
    /// as a Channel Access name.
    Unsupported,
}

impl Scheme {
    fn prefix(self) -> &'static str {
        match self {
            Scheme::Ca => "ca",
            Scheme::Pva => "pva",
            Scheme::Loc => "loc",
            Scheme::Sim => "sim",
            Scheme::Formula => "eq",
            Scheme::Unsupported => "",
        }
    }
}

/// A parsed PV name such as `ca://X`, `loc://x<VDouble>(3)` or `sim://sine(0, 10, 0.1)`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PvName {
    pub scheme: Scheme,
    /// Everything after the `scheme://` prefix.
    pub pv: String,
}

impl PvName {
    /// Parse a name; one without a prefix is Channel Access, as in Phoebus,
    /// and one starting `=` is a formula, as in BOY. An unknown `scheme://`
    /// prefix is [`Scheme::Unsupported`].
    pub fn parse(name: &str) -> Self {
        let name = name.trim();
        if let Some(expr) = name.strip_prefix('=') {
            return Self {
                scheme: Scheme::Formula,
                pv: expr.trim().to_string(),
            };
        }
        for scheme in [
            Scheme::Ca,
            Scheme::Pva,
            Scheme::Loc,
            Scheme::Sim,
            Scheme::Formula,
        ] {
            if let Some(rest) = name
                .strip_prefix(scheme.prefix())
                .and_then(|r| r.strip_prefix("://"))
            {
                return Self {
                    scheme,
                    pv: rest.trim().to_string(),
                };
            }
        }
        if name.split_once("://").is_some_and(|(p, _)| is_scheme(p)) {
            return Self {
                scheme: Scheme::Unsupported,
                pv: name.to_string(),
            };
        }
        Self {
            scheme: Scheme::Ca,
            pv: name.to_string(),
        }
    }

    /// The part before any `<type>` or `(arguments)`.
    pub fn base(&self) -> &str {
        let end = self.pv.find(['<', '(']).unwrap_or(self.pv.len());
        self.pv[..end].trim()
    }

    /// The `<VType>` of a local PV, if given.
    pub fn vtype(&self) -> Option<&str> {
        let start = self.pv.find('<')?;
        let end = self.pv[start..].find('>')? + start;
        Some(self.pv[start + 1..end].trim())
    }

    /// The raw text inside the outer parentheses, if any.
    pub fn args(&self) -> Option<&str> {
        let start = self.pv.find('(')?;
        let end = self.pv.rfind(')')?;
        (end > start).then(|| self.pv[start + 1..end].trim())
    }

    /// Identifies a subscription: local PVs share state by base name alone.
    pub fn key(&self) -> String {
        match self.scheme {
            Scheme::Loc => format!("loc://{}", self.base()),
            Scheme::Formula | Scheme::Unsupported => self.to_string(),
            _ => format!("{}://{}", self.scheme.prefix(), self.pv),
        }
    }
}

impl fmt::Display for PvName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.scheme {
            Scheme::Formula => write!(f, "={}", self.pv),
            Scheme::Unsupported => f.write_str(&self.pv),
            _ => write!(f, "{}://{}", self.scheme.prefix(), self.pv),
        }
    }
}

/// An RFC 3986 scheme: a letter, then letters, digits, `+`, `-` or `.`.
fn is_scheme(p: &str) -> bool {
    p.starts_with(|c: char| c.is_ascii_alphabetic())
        && p.chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
}

/// Split an argument list on commas that are outside double quotes.
pub(crate) fn split_args(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in args.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ',' if !quoted => out.push(std::mem::take(&mut cur).trim().to_string()),
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() || !out.is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_scheme_is_ca() {
        let n = PvName::parse("bl01t:SUM");
        assert_eq!(n.scheme, Scheme::Ca);
        assert_eq!(n.key(), PvName::parse("ca://bl01t:SUM").key());
    }

    #[test]
    fn local_parts() {
        let n = PvName::parse("loc://x<VEnum>(1, \"a\", \"b,c\")");
        assert_eq!(n.scheme, Scheme::Loc);
        assert_eq!(n.base(), "x");
        assert_eq!(n.vtype(), Some("VEnum"));
        assert_eq!(split_args(n.args().unwrap()), vec!["1", "\"a\"", "\"b,c\""]);
        assert_eq!(n.key(), "loc://x");
    }

    #[test]
    fn sim_args() {
        let n = PvName::parse("sim://sine(0, 10, 0.5)");
        assert_eq!(n.base(), "sine");
        assert_eq!(split_args(n.args().unwrap()), vec!["0", "10", "0.5"]);
        assert!(PvName::parse("sim://ramp").args().is_none());
    }

    #[test]
    fn formulas() {
        let n = PvName::parse(" =(pv(\"X\")*1000)");
        assert_eq!(n.scheme, Scheme::Formula);
        assert_eq!(n.pv, "(pv(\"X\")*1000)");
        assert_eq!(n.to_string(), "=(pv(\"X\")*1000)");
        assert_eq!(n.key(), PvName::parse("eq://(pv(\"X\")*1000)").key());
    }

    #[test]
    fn unknown_schemes_are_unsupported() {
        for name in ["sys://time", "mqtt://x/y", "mqtt+ssl://x"] {
            let n = PvName::parse(name);
            assert_eq!(n.scheme, Scheme::Unsupported, "{name}");
            assert_eq!(n.to_string(), name);
        }
        // A colon in a CA name is not a scheme.
        assert_eq!(PvName::parse("BL01:A://B").scheme, Scheme::Ca);
        assert_eq!(PvName::parse("BL01T-MO-01:X").scheme, Scheme::Ca);
        assert_eq!(PvName::parse("pva://X").scheme, Scheme::Pva);
        assert_eq!(PvName::parse("eq://1").scheme, Scheme::Formula);
    }
}
