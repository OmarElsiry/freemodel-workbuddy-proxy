#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    pub name: &'static str,
    pub usage: &'static str,
    pub description: &'static str,
    pub shortcut: Option<&'static str>,
    pub destructive: bool,
}

pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "help",
        usage: "/help",
        description: "Open help and command reference",
        shortcut: Some("F1 / Ctrl+K"),
        destructive: false,
    },
    CommandSpec {
        name: "shortcuts",
        usage: "/shortcuts",
        description: "Show keyboard shortcuts",
        shortcut: None,
        destructive: false,
    },
    CommandSpec {
        name: "status",
        usage: "/status",
        description: "Show connection and request status",
        shortcut: None,
        destructive: false,
    },
    CommandSpec {
        name: "diagnostics",
        usage: "/diagnostics",
        description: "Open diagnostics",
        shortcut: None,
        destructive: false,
    },
    CommandSpec {
        name: "models",
        usage: "/models",
        description: "Choose an available model",
        shortcut: Some("Ctrl+M"),
        destructive: false,
    },
    CommandSpec {
        name: "new",
        usage: "/new [title]",
        description: "Create a proxy session",
        shortcut: Some("Ctrl+N"),
        destructive: false,
    },
    CommandSpec {
        name: "sessions",
        usage: "/sessions",
        description: "Open the session picker",
        shortcut: Some("Ctrl+O"),
        destructive: false,
    },
    CommandSpec {
        name: "switch",
        usage: "/switch <id|index>",
        description: "Switch to another proxy session",
        shortcut: None,
        destructive: false,
    },
    CommandSpec {
        name: "rename",
        usage: "/rename <title>",
        description: "Rename the current proxy session",
        shortcut: None,
        destructive: false,
    },
    CommandSpec {
        name: "delete",
        usage: "/delete",
        description: "Delete the current proxy session",
        shortcut: None,
        destructive: true,
    },
    CommandSpec {
        name: "clear",
        usage: "/clear",
        description: "Clear current session history",
        shortcut: None,
        destructive: true,
    },
    CommandSpec {
        name: "project",
        usage: "/project [path]",
        description: "Choose or change project",
        shortcut: Some("Ctrl+P"),
        destructive: false,
    },
    CommandSpec {
        name: "model",
        usage: "/model [id]",
        description: "Choose or set model",
        shortcut: Some("Ctrl+M"),
        destructive: false,
    },
    CommandSpec {
        name: "retry",
        usage: "/retry",
        description: "Retry the last user request",
        shortcut: Some("Ctrl+R"),
        destructive: false,
    },
    CommandSpec {
        name: "edit",
        usage: "/edit",
        description: "Edit and resend the last user request",
        shortcut: Some("Ctrl+E"),
        destructive: false,
    },
    CommandSpec {
        name: "cancel",
        usage: "/cancel",
        description: "Cancel active generation",
        shortcut: Some("Esc / Ctrl+C"),
        destructive: false,
    },
    CommandSpec {
        name: "search",
        usage: "/search <text>",
        description: "Search conversation",
        shortcut: Some("Ctrl+F"),
        destructive: false,
    },
    CommandSpec {
        name: "copy",
        usage: "/copy [last|all]",
        description: "Copy response or transcript",
        shortcut: None,
        destructive: false,
    },
    CommandSpec {
        name: "logs",
        usage: "/logs",
        description: "Show relevant log location",
        shortcut: None,
        destructive: false,
    },
    CommandSpec {
        name: "key",
        usage: "/key",
        description: "Securely configure API key",
        shortcut: None,
        destructive: false,
    },
    CommandSpec {
        name: "settings",
        usage: "/settings",
        description: "Open TUI preferences",
        shortcut: None,
        destructive: false,
    },
    CommandSpec {
        name: "quit",
        usage: "/quit",
        description: "Exit safely",
        shortcut: Some("Ctrl+Q"),
        destructive: false,
    },
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedCommand {
    pub name: String,
    pub args: Vec<String>,
}

pub fn parse(input: &str) -> Result<Option<ParsedCommand>, String> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return Ok(None);
    }
    let words = split_words(&trimmed[1..])?;
    if words.is_empty() {
        return Err("Enter a command after '/'".into());
    }
    let name = words[0].to_lowercase();
    if !COMMANDS.iter().any(|c| c.name == name) {
        return Err(format!("Unknown command '/{name}'. Use /help."));
    }
    Ok(Some(ParsedCommand {
        name,
        args: words[1..].to_vec(),
    }))
}

pub fn completions(prefix: &str) -> Vec<&'static CommandSpec> {
    let prefix = prefix.trim_start_matches('/').to_lowercase();
    COMMANDS
        .iter()
        .filter(|c| c.name.starts_with(&prefix))
        .collect()
}

fn split_words(input: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escape = false;
    for c in input.chars() {
        if escape {
            current.push(c);
            escape = false;
            continue;
        }
        if c == '\\' {
            escape = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None
            } else {
                current.push(c)
            }
            continue;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
        } else if c.is_whitespace() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        } else {
            current.push(c);
        }
    }
    if escape {
        return Err("Command ends with an incomplete escape".into());
    }
    if quote.is_some() {
        return Err("Command contains an unclosed quote".into());
    }
    if !current.is_empty() {
        out.push(current);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    #[test]
    fn registry_is_unique_and_discoverable() {
        let mut names = HashSet::new();
        for c in COMMANDS {
            assert!(names.insert(c.name));
            assert!(c.usage.starts_with('/'));
            assert!(!c.description.is_empty());
        }
    }
    #[test]
    fn parses_quoted_arguments() {
        let c = parse("/rename \"A useful session\"").unwrap().unwrap();
        assert_eq!(c.args, vec!["A useful session"]);
    }
    #[test]
    fn rejects_unknown_and_unclosed() {
        assert!(parse("/wat").is_err());
        assert!(parse("/rename 'x").is_err());
    }
}
