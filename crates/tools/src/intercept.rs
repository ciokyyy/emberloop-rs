//! A routing gate in front of `bash`: a shell command that belongs to another tool is refused
//! before it runs, and the reply names the tool that does it. It is routing, not security: it
//! reads a flat command list and refuses the shapes it knows.
//!
//! A quoted `>` or a redirection of another file descriptor is not a write, and a compound shell
//! construct it cannot read is left to run as it always did. Reads are never refused, so their
//! views keep feeding the seen-line store the `edit` tool checks.

/// One refusal: which tool owns the command and what the model reads instead.
pub struct Rule {
    pub tool: &'static str,
    pub message: &'static str,
}

const SED_IN_PLACE: Rule = Rule {
    tool: "edit",
    message: "sed -i edits a file in place; use the edit tool: read the file for its tag, then send a [path#TAG] header and PUT/CUT operations",
};

const PERL_IN_PLACE: Rule = Rule {
    tool: "edit",
    message: "perl -i edits a file in place; use the edit tool: read the file for its tag, then send a [path#TAG] header and PUT/CUT operations",
};

const AWK_IN_PLACE: Rule = Rule {
    tool: "edit",
    message: "awk -i inplace edits a file in place; use the edit tool: read the file for its tag, then send a [path#TAG] header and PUT/CUT operations",
};

const PYTHON_SCRIPT: Rule = Rule {
    tool: "execute",
    message: "python through bash is refused: write the script in the execute tool, which runs Python with the tools available and no shell in between",
};

const WRITE_REDIRECT: Rule = Rule {
    tool: "write",
    message: "a redirect writes a file; use write for a new file and edit for a change to an existing one",
};

const TEE: Rule = Rule {
    tool: "write",
    message:
        "tee writes a file; use write for a new file and edit for an append to an existing one",
};

const BACKGROUND: Rule = Rule {
    tool: "background",
    message: "a server, watcher or long-running command belongs in bash with background: true, which returns a process id you can read later",
};

/// blocked names the tool a command belongs to, or `None` when the command may run. The narrowest
/// shapes come first, so the reply names the closest tool. `EL_BASH_INTERCEPT=off` disables the
/// whole gate for one daemon.
pub fn blocked(command: &str) -> Option<&'static Rule> {
    if std::env::var("EL_BASH_INTERCEPT").is_ok_and(|value| value.eq_ignore_ascii_case("off")) {
        return None;
    }
    if whitespace_words(command).is_empty() {
        return None;
    }
    if in_place(command, "sed") {
        return Some(&SED_IN_PLACE);
    }
    if in_place(command, "perl") {
        return Some(&PERL_IN_PLACE);
    }
    if word_is(command, "awk") && command.contains("inplace") {
        return Some(&AWK_IN_PLACE);
    }
    if python_runs_inline(command) {
        return Some(&PYTHON_SCRIPT);
    }
    if writes_through_redirect(command) {
        return Some(&WRITE_REDIRECT);
    }
    if any_word_is(command, "tee") {
        return Some(&TEE);
    }
    if runs_in_background(command) {
        return Some(&BACKGROUND);
    }
    None
}

/// units splits a command into the pieces a shell would run, so a check can look at what actually
/// runs rather than the whole line. Quotes keep their separators.
fn units(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut single = false;
    let mut double = false;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' if !double => {
                single = !single;
                current.push(c);
            }
            '"' if !single => {
                double = !double;
                current.push(c);
            }
            '\n' if !single && !double => out.push(std::mem::take(&mut current)),
            ';' if !single && !double => out.push(std::mem::take(&mut current)),
            // `&&` separates units; a lone `&` belongs to its unit (`>&2`, a trailing background
            // marker), so splitting on it would read `echo x >&2` as a file write.
            '&' if !single && !double && chars.peek() == Some(&'&') => {
                chars.next();
                out.push(std::mem::take(&mut current));
            }
            '|' if !single && !double => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                }
                out.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    out.push(current);
    out
}

/// units_with_heredocs also splits `<<` apart, so `python3 <<EOF` reads as the inline run it is.
fn units_with_heredocs(command: &str) -> Vec<String> {
    units(command)
        .into_iter()
        .map(|unit| unit.replace("<<", " << "))
        .collect()
}

/// words splits one unit into words with quotes dropped.
fn words(unit: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut single = false;
    let mut double = false;
    for c in unit.chars() {
        match c {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            c if c.is_whitespace() && !single && !double => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// whitespace_words is every word of the whole command, for the shapes that read the line rather
/// than one unit (a command after a pipe).
fn whitespace_words(command: &str) -> Vec<String> {
    command.split_whitespace().map(str::to_string).collect()
}

fn word_is(command: &str, word: &str) -> bool {
    units(command)
        .iter()
        .filter_map(|unit| words(unit).into_iter().next())
        .any(|first| first == word)
}

fn any_word_is(command: &str, word: &str) -> bool {
    units(command)
        .iter()
        .flat_map(|unit| words(unit))
        .any(|each| each == word)
}

/// in_place spots `<tool> -i` (or `--in-place`) on the command word. For perl a bundled short flag
/// such as `-pi` counts too.
fn in_place(command: &str, tool: &str) -> bool {
    units(command).iter().any(|unit| {
        let mut fields = words(unit).into_iter();
        if fields.next().as_deref() != Some(tool) {
            return false;
        }
        fields.any(|arg| match tool {
            "perl" => arg.starts_with('-') && !arg.starts_with("--") && arg.contains('i'),
            _ => arg == "-i" || arg == "--in-place" || arg.starts_with("-i."),
        })
    })
}

/// python_runs_inline reports the shapes where Python is a script on the command line: `-c`,
/// stdin (`-`), or a heredoc. `python3 file.py` and `python3 -m http.server` stay allowed.
fn python_runs_inline(command: &str) -> bool {
    units_with_heredocs(command).iter().any(|unit| {
        let mut fields = words(unit).into_iter();
        let Some(first) = fields.next() else {
            return false;
        };
        if !is_python(&first) {
            return false;
        }
        fields.any(|arg| arg == "-c" || arg == "-" || arg == "<<" || arg.starts_with("-c"))
    })
}

fn is_python(word: &str) -> bool {
    let name = word.rsplit('/').next().unwrap_or(word);
    name == "python" || name == "python3"
}

/// writes_through_redirect reports a cat/echo/printf whose standard output goes to a file.
fn writes_through_redirect(command: &str) -> bool {
    units(command).iter().any(|unit| {
        matches!(
            words(unit).first().map(String::as_str),
            Some("cat" | "echo" | "printf")
        ) && redirects_to_file(unit)
    })
}

/// redirects_to_file reads one unit character by character: a quoted `>`, `>&2` and `2>file` do
/// not write a file the model meant to author.
fn redirects_to_file(unit: &str) -> bool {
    let chars: Vec<char> = unit.chars().collect();
    let (mut single, mut double) = (false, false);
    for (index, c) in chars.iter().enumerate() {
        match c {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '>' if !single && !double => {
                let next = chars.get(index + 1);
                let previous = index.checked_sub(1).and_then(|at| chars.get(at));
                if next == Some(&'&') || previous == Some(&'&') {
                    continue;
                }
                if let Some(digit) = previous.filter(|c| c.is_ascii_digit()) {
                    if *digit != '1' {
                        continue;
                    }
                }
                return true;
            }
            _ => {}
        }
    }
    false
}

/// runs_in_background reports nohup and a trailing `&` that is not `&&`.
fn runs_in_background(command: &str) -> bool {
    if word_is(command, "nohup") {
        return true;
    }
    let trimmed = command.trim_end();
    trimmed.ends_with('&') && !trimmed.ends_with("&&")
}
