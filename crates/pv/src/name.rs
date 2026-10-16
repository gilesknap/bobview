use std::fmt;

/// Which backend serves a PV.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scheme {
    Ca,
    Pva,
    Loc,
    Sim,
}

impl Scheme {
    fn prefix(self) -> &'static str {
        match self {
            Scheme::Ca => "ca",
            Scheme::Pva => "pva",
            Scheme::Loc => "loc",
            Scheme::Sim => "sim",
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
    /// Parse a name; one without a known prefix is Channel Access, as in Phoebus.
    pub fn parse(name: &str) -> Self {
        let name = name.trim();
        for scheme in [Scheme::Ca, Scheme::Pva, Scheme::Loc, Scheme::Sim] {
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
            _ => format!("{}://{}", self.scheme.prefix(), self.pv),
        }
    }
}

impl fmt::Display for PvName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}", self.scheme.prefix(), self.pv)
    }
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
}
