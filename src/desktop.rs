//! Keep the installed launcher's right-click menu in step with the saved locations.
//!
//! `install.sh` writes `~/.local/share/applications/terminalgoblin.desktop`. Each `[[locations]]`
//! entry becomes a `[Desktop Action]` there that runs `terminalgoblin --location NAME`, which the
//! app menu, dock and taskbar show when the launcher is right-clicked.

use std::path::PathBuf;

use crate::config::LocationDef;

const ACTION_PREFIX: &str = "location-";

fn desktop_file() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("applications/terminalgoblin.desktop"))
}

/// Rewrite the installed desktop file's actions for `locations`. Does nothing when TerminalGoblin
/// isn't installed there or the actions are already current.
pub fn sync_actions(locations: &[LocationDef]) {
    let Some(path) = desktop_file() else { return };
    let Ok(old) = std::fs::read_to_string(&path) else {
        return;
    };
    let new = with_actions(&old, locations);
    if new != old
        && let Err(e) = std::fs::write(&path, new)
    {
        log::warn!("couldn't update {}: {e}", path.display());
    }
}

/// `text` with its location actions replaced by ones for `locations`.
fn with_actions(text: &str, locations: &[LocationDef]) -> String {
    // Keep everything but our own `Actions=` line and `[Desktop Action location-N]` sections.
    let mut kept = Vec::new();
    let mut other_actions = Vec::new();
    let mut in_ours = false;
    for line in text.lines() {
        if line.starts_with('[') {
            in_ours = line.starts_with(&format!("[Desktop Action {ACTION_PREFIX}"));
        }
        if in_ours {
            continue;
        }
        if let Some(list) = line.strip_prefix("Actions=") {
            other_actions.extend(
                list.split(';')
                    .filter(|a| !a.is_empty() && !a.starts_with(ACTION_PREFIX))
                    .map(str::to_string),
            );
            continue;
        }
        kept.push(line);
    }
    while kept.last().is_some_and(|l| l.trim().is_empty()) {
        kept.pop();
    }
    let exe = kept
        .iter()
        .find_map(|l| l.strip_prefix("Exec="))
        .and_then(|e| e.split_whitespace().next())
        .unwrap_or("terminalgoblin")
        .to_string();

    let named: Vec<&LocationDef> = locations.iter().filter(|l| !l.name.trim().is_empty()).collect();
    let mut actions = other_actions;
    actions.extend((0..named.len()).map(|i| format!("{ACTION_PREFIX}{i}")));

    let mut out = String::new();
    for line in kept {
        out.push_str(line);
        out.push('\n');
        if line == "[Desktop Entry]" && !actions.is_empty() {
            out.push_str(&format!("Actions={};\n", actions.join(";")));
        }
    }
    for (i, loc) in named.iter().enumerate() {
        let name = loc.name.trim();
        out.push_str(&format!(
            "\n[Desktop Action {ACTION_PREFIX}{i}]\nName={}\nExec={exe} --location {}\n",
            escape_value(name),
            escape_value(&quote_arg(name)),
        ));
    }
    out
}

/// Quote one Exec argument (desktop entry spec, "The Exec key").
fn quote_arg(arg: &str) -> String {
    let mut q = String::from("\"");
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            q.push('\\');
        }
        q.push(c);
    }
    q.push('"');
    // `%` starts a field code anywhere in Exec.
    q.replace('%', "%%")
}

/// Escape a string value: backslashes and line breaks.
fn escape_value(v: &str) -> String {
    v.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "\\r")
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "[Desktop Entry]\nType=Application\nExec=/home/u/.local/bin/terminalgoblin\nIcon=x\n";

    fn loc(name: &str) -> LocationDef {
        LocationDef {
            name: name.into(),
            path: "~".into(),
            ..Default::default()
        }
    }

    #[test]
    fn adds_and_replaces_actions() {
        let once = with_actions(BASE, &[loc("Home"), loc("My \"web\" 100%")]);
        assert!(once.contains("Actions=location-0;location-1;\n"));
        assert!(once.contains(
            "[Desktop Action location-1]\nName=My \"web\" 100%\nExec=/home/u/.local/bin/terminalgoblin --location \"My \\\\\"web\\\\\" 100%%\"\n"
        ));
        // Idempotent, and removing locations removes their actions.
        assert_eq!(with_actions(&once, &[loc("Home"), loc("My \"web\" 100%")]), once);
        assert_eq!(with_actions(&once, &[]), BASE);
    }

    #[test]
    fn keeps_other_actions_and_groups() {
        let text = "[Desktop Entry]\nExec=tg\nActions=new-window;\n\n[Desktop Action new-window]\nName=New\nExec=tg --new-instance\n";
        let out = with_actions(text, &[loc("Home")]);
        assert!(
            out.starts_with("[Desktop Entry]\nActions=new-window;location-0;\nExec=tg\n\n[Desktop Action new-window]")
        );
        assert!(out.ends_with("[Desktop Action location-0]\nName=Home\nExec=tg --location \"Home\"\n"));
    }
}
