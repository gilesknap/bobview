//! Recognising the ways BOY screens open a trend of some PVs, which bobview
//! shows in its own trend window instead of running them: a script that
//! opens a Data Browser, and a command that starts StripTool.

/// The Eclipse command that opens a new Data Browser window.
const DATABROWSER: &str = "org.csstudio.trends.databrowser2.newWindow";

/// The PVs of a BOY script that opens a Data Browser on them:
///
/// ```python
/// from org.csstudio.opibuilder.scriptUtil import ScriptUtil
///
/// command_id="org.csstudio.trends.databrowser2.newWindow"
/// ScriptUtil.executeEclipseCommand(command_id, ['pvnames', 'A:PLOG,B:PLOG'])
/// ```
///
/// The command may also be given in place, and the script may be
/// JavaScript. The names are as written, macros and all.
pub fn databrowser_pvs(script: &str) -> Option<Vec<String>> {
    if !script.contains(DATABROWSER) {
        return None;
    }
    let call = &script[script.find("executeEclipseCommand")?..];
    // The value after the "pvnames" parameter name: the next quoted string.
    let key = call.find("pvnames")?;
    let rest = call[key + "pvnames".len()..]
        .trim_start_matches(['\'', '"'])
        .trim_start();
    let rest = rest.strip_prefix(',')?.trim_start();
    let quote = rest.chars().next().filter(|c| matches!(c, '\'' | '"'))?;
    let body = &rest[1..];
    let names = &body[..body.find(quote)?];
    let pvs: Vec<String> = names
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .collect();
    (!pvs.is_empty()).then_some(pvs)
}

/// The DLS scripts that start StripTool on the PVs they are given.
const STRIP_TOOLS: &[&str] = &["strip.py", "strip.sh", "buildStripToolconfig.sh"];

/// The PVs of a command that starts StripTool, such as
/// `strip.py SR04S-VA-GAUGE-01:PLOG SR04S-VA-GAUGE-02:PLOG log_mbar`, or
/// `None` for any other command. The last word, when it has no `:`, says
/// how to scale the chart (`log_mbar`) and is not a PV.
pub fn strip_tool_pvs(command: &str) -> Option<Vec<String>> {
    let mut words = command.split_whitespace();
    let program = words.next()?;
    let name = program.rsplit('/').next().unwrap_or(program);
    if !STRIP_TOOLS.contains(&name) {
        return None;
    }
    let mut pvs: Vec<String> = words.map(str::to_string).collect();
    if pvs.last().is_some_and(|w| !w.contains(':')) {
        pvs.pop();
    }
    (!pvs.is_empty()).then_some(pvs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn databrowser_scripts_name_their_pvs() {
        // As in the DLS vacuum screens, with the command in a variable.
        let py = "from org.csstudio.opibuilder.scriptUtil import ScriptUtil\n\n\
            command_id=\"org.csstudio.trends.databrowser2.newWindow\"\n\
            ScriptUtil.executeEclipseCommand(command_id, ['pvnames', \
            '$(dom)-VA-GAUGE-01:PLOG,$(dom)-VA-GAUGE-02:PLOG, $(dom)-VA-GAUGE-03:PLOG'])";
        assert_eq!(
            databrowser_pvs(py).unwrap(),
            [
                "$(dom)-VA-GAUGE-01:PLOG",
                "$(dom)-VA-GAUGE-02:PLOG",
                "$(dom)-VA-GAUGE-03:PLOG"
            ]
        );
        let js = "ScriptUtil.executeEclipseCommand(\"org.csstudio.trends.databrowser2.newWindow\", \
            [\"pvnames\", \"A:B\"]);";
        assert_eq!(databrowser_pvs(js).unwrap(), ["A:B"]);
        // Other commands, scripts and empty lists are not trends.
        let other = "ScriptUtil.executeEclipseCommand(\"org.csstudio.other\", ['pvnames', 'A'])";
        assert_eq!(databrowser_pvs(other), None);
        assert_eq!(
            databrowser_pvs("ScriptUtil.closeAssociatedOPI(widget)"),
            None
        );
        let empty = format!("ScriptUtil.executeEclipseCommand(\"{DATABROWSER}\", ['pvnames', ''])");
        assert_eq!(databrowser_pvs(&empty), None);
    }

    #[test]
    fn strip_tool_commands_name_their_pvs() {
        assert_eq!(
            strip_tool_pvs("strip.py $(device):PLOG log_mbar").unwrap(),
            ["$(device):PLOG"]
        );
        assert_eq!(
            strip_tool_pvs("buildStripToolconfig.sh SR04S-VA-GAUGE-01:PLOG SR04S-VA-GAUGE-03:PLOG")
                .unwrap(),
            ["SR04S-VA-GAUGE-01:PLOG", "SR04S-VA-GAUGE-03:PLOG"]
        );
        assert_eq!(
            strip_tool_pvs("/dls_sw/prod/tools/strip.sh BL01T:A:M2  BL01T:A:M4 log_mbar").unwrap(),
            ["BL01T:A:M2", "BL01T:A:M4"]
        );
        assert_eq!(strip_tool_pvs("strip.py log_mbar"), None);
        assert_eq!(strip_tool_pvs("./srCellVacuum 04"), None);
        assert_eq!(strip_tool_pvs("mystrip.py A:B"), None);
        assert_eq!(strip_tool_pvs(""), None);
    }
}
