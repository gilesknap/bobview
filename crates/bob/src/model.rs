use crate::{Element, Macros};

/// An RGBA colour, optionally with its Phoebus name.
#[derive(Clone, Debug, PartialEq)]
pub struct Color {
    pub name: Option<String>,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::rgba_const(r, g, b, 255)
    }

    const fn rgba_const(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            name: None,
            r,
            g,
            b,
            a,
        }
    }

    /// Read `<color name=".." red=".." green=".." blue=".." alpha=".."/>`.
    pub fn from_element(el: &Element) -> Self {
        let c = |k: &str, d: u8| {
            el.attr(k)
                .and_then(|v| v.trim().parse::<f64>().ok())
                .map(|v| v.clamp(0.0, 255.0) as u8)
                .unwrap_or(d)
        };
        let name = el.attr("name").map(str::to_string);
        // A named colour without RGB values is looked up in the default palette.
        if el.attr("red").is_none()
            && let Some(n) = &name
        {
            return named_color(n);
        }
        Self {
            name,
            r: c("red", 0),
            g: c("green", 0),
            b: c("blue", 0),
            a: c("alpha", 255),
        }
    }

    pub fn rgba(&self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

/// Colours from the default Phoebus `color.def`.
pub fn named_color(name: &str) -> Color {
    let (r, g, b, a) = match name {
        "OK" | "On" => (0, 255, 0, 255),
        "MINOR" => (255, 128, 0, 255),
        "MAJOR" | "STOP" => (255, 0, 0, 255),
        "INVALID" => (255, 0, 255, 255),
        "DISCONNECTED" => (200, 0, 200, 200),
        "ActiveText" => (255, 255, 0, 255),
        "Background" => (255, 255, 255, 255),
        "Read_Background" => (240, 240, 240, 255),
        "Write_Background" => (128, 255, 255, 255),
        "Button_Background" => (210, 210, 210, 255),
        "Header_Background" => (77, 77, 77, 255),
        "Header_ForeGround" => (255, 255, 255, 255),
        "Attention" => (255, 160, 0, 255),
        "Off" => (60, 100, 60, 255),
        "Grid" => (128, 128, 128, 255),
        "Transparent" => (255, 255, 255, 0),
        _ => (0, 0, 0, 255), // "Text" and anything unknown
    };
    Color {
        name: Some(name.to_string()),
        ..Color::rgba_const(r, g, b, a)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontStyle {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    pub name: Option<String>,
    pub family: String,
    pub style: FontStyle,
    pub size: f32,
}

impl Default for Font {
    /// The Phoebus "Default" font.
    fn default() -> Self {
        Self {
            name: Some("Default".into()),
            family: "Liberation Sans".into(),
            style: FontStyle::Regular,
            size: 14.0,
        }
    }
}

impl Font {
    pub fn from_element(el: &Element) -> Self {
        let name = el.attr("name").map(str::to_string);
        if el.attr("size").is_none()
            && let Some(n) = &name
        {
            return named_font(n);
        }
        Self {
            name,
            family: el.attr("family").unwrap_or("Liberation Sans").to_string(),
            style: match el.attr("style").unwrap_or("REGULAR") {
                "BOLD" => FontStyle::Bold,
                "ITALIC" => FontStyle::Italic,
                "BOLD_ITALIC" => FontStyle::BoldItalic,
                _ => FontStyle::Regular,
            },
            size: el.attr("size").and_then(|s| s.parse().ok()).unwrap_or(14.0),
        }
    }

    pub fn is_bold(&self) -> bool {
        matches!(self.style, FontStyle::Bold | FontStyle::BoldItalic)
    }

    pub fn is_italic(&self) -> bool {
        matches!(self.style, FontStyle::Italic | FontStyle::BoldItalic)
    }
}

/// Fonts from the default Phoebus `font.def`.
fn named_font(name: &str) -> Font {
    let (style, size) = match name {
        "Default Bold" => (FontStyle::Bold, 14.0),
        "Header 1" => (FontStyle::Bold, 22.0),
        "Header 2" => (FontStyle::Bold, 18.0),
        "Header 3" => (FontStyle::Bold, 16.0),
        "Comment" => (FontStyle::Italic, 14.0),
        "Fine Print" => (FontStyle::Regular, 12.0),
        _ => (FontStyle::Regular, 14.0),
    };
    Font {
        name: Some(name.to_string()),
        style,
        size,
        ..Font::default()
    }
}

/// A widget action, e.g. from an action button.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    OpenDisplay {
        file: String,
        macros: Macros,
        /// "replace", "tab" or "window".
        target: String,
        description: String,
    },
    WritePv {
        pv_name: String,
        value: String,
        description: String,
    },
    OpenWebpage {
        url: String,
        description: String,
    },
    /// Run a script: `file` names it, and an embedded one (`EmbeddedJs`,
    /// `EmbeddedPy`) has its source in `text`.
    Execute {
        file: String,
        text: String,
        description: String,
    },
    /// An external command, run in `directory` (the display's own when empty).
    Command {
        command: String,
        directory: String,
        description: String,
    },
    /// Close the display, as BOY's `ScriptUtil.closeAssociatedOPI` scripts
    /// (EDM "Exit" buttons) do: type `close_display`, bobview's own.
    CloseDisplay {
        description: String,
    },
    /// Open a trend of `pv_names`, as BOY scripts that open a Data Browser
    /// do: type `trend`, bobview's own, with a `<pv_name>` for each PV.
    Trend {
        pv_names: Vec<String>,
        description: String,
    },
    /// Anything else (other scripts, …): shown but does nothing.
    Unsupported {
        kind: String,
        description: String,
    },
}

impl Action {
    pub fn from_element(el: &Element) -> Self {
        let text = |n: &str| el.child_text(n).unwrap_or("").to_string();
        let description = text("description");
        match el.attr("type").unwrap_or("") {
            "open_display" => Action::OpenDisplay {
                file: text("file"),
                macros: el
                    .child("macros")
                    .map(Macros::from_element)
                    .unwrap_or_default(),
                target: el.child_text("target").unwrap_or("replace").to_string(),
                description,
            },
            "write_pv" => Action::WritePv {
                pv_name: text("pv_name"),
                value: text("value"),
                description,
            },
            "open_webpage" => Action::OpenWebpage {
                url: text("url"),
                description,
            },
            "execute" => {
                let script = el.child("script");
                Action::Execute {
                    file: script
                        .and_then(|s| s.attr("file"))
                        .unwrap_or("")
                        .to_string(),
                    text: script
                        .and_then(|s| s.child_text("text"))
                        .unwrap_or("")
                        .to_string(),
                    description,
                }
            }
            "command" => Action::Command {
                command: text("command"),
                directory: text("command_directory"),
                description,
            },
            "close_display" => Action::CloseDisplay { description },
            "trend" => Action::Trend {
                pv_names: el
                    .children
                    .iter()
                    .filter(|c| c.name == "pv_name")
                    .map(|c| c.text.clone())
                    .collect(),
                description,
            },
            other => Action::Unsupported {
                kind: other.to_string(),
                description,
            },
        }
    }

    pub fn description(&self) -> &str {
        match self {
            Action::OpenDisplay { description, .. }
            | Action::WritePv { description, .. }
            | Action::OpenWebpage { description, .. }
            | Action::Execute { description, .. }
            | Action::Command { description, .. }
            | Action::CloseDisplay { description }
            | Action::Trend { description, .. }
            | Action::Unsupported { description, .. } => description,
        }
    }
}

/// A rule: sets property `prop_id` from the first expression that holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub name: String,
    pub prop_id: String,
    /// When true the expression's value is the property value (`out_exp`).
    pub out_exp: bool,
    pub exprs: Vec<RuleExpr>,
    pub pv_names: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuleExpr {
    pub condition: String,
    /// The `<value>` element: text, or a nested `<color>`/`<font>`; for an
    /// output expression rule, the `<expression>` (or BOY's `<value>`).
    pub value: Element,
}

impl Rule {
    pub fn from_element(el: &Element) -> Self {
        Self {
            name: el.attr("name").unwrap_or("").to_string(),
            prop_id: el.attr("prop_id").unwrap_or("").to_string(),
            out_exp: el.attr("out_exp") == Some("true"),
            exprs: el
                .children
                .iter()
                .filter(|c| c.name == "exp")
                .map(|e| RuleExpr {
                    condition: e.attr("bool_exp").unwrap_or("false").to_string(),
                    value: e
                        .child("expression")
                        .or_else(|| e.child("value"))
                        .cloned()
                        .unwrap_or_default(),
                })
                .collect(),
            pv_names: el
                .children
                .iter()
                .filter(|c| c.name == "pv_name")
                .map(|c| c.text.clone())
                .collect(),
        }
    }
}
