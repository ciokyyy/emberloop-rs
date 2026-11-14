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

/// Fail-closed read-only command validator for `scout` subagents.
pub fn scout_blocked(command: &str) -> Option<String> {
    let units = match tokenize_scout_units(command) {
        Ok(units) => units,
        Err(reason) => return Some(reason),
    };
    for tokens in units {
        if let Err(reason) = validate_scout_argv(&tokens) {
            return Some(reason);
        }
    }
    None
}

enum ScoutSyntaxStep {
    Skip(usize),
    Space(usize),
    Split(usize),
}

fn match_unquoted_scout_syntax(
    rest: &str,
    has_token: bool,
) -> Result<Option<ScoutSyntaxStep>, String> {
    if !has_token {
        for safe in ["2>/dev/null", "2>&1"] {
            if let Some(after) = rest.strip_prefix(safe) {
                if after
                    .chars()
                    .next()
                    .is_none_or(|c| c.is_whitespace() || matches!(c, ';' | '|' | '&'))
                {
                    return Ok(Some(ScoutSyntaxStep::Skip(safe.len())));
                }
            }
        }
    }
    if rest.starts_with("&&") || rest.starts_with("||") {
        return Ok(Some(ScoutSyntaxStep::Split(2)));
    }
    let c = rest.chars().next().ok_or("invalid command")?;
    match c {
        ';' | '|' | '\n' => Ok(Some(ScoutSyntaxStep::Split(1))),
        '(' | ')' | '{' | '}' => {
            Err("subshells and grouping are not allowed for scout Sessions".into())
        }
        '>' => Err("output redirects are not allowed for scout Sessions".into()),
        '<' => Err("input redirects and heredocs are not allowed for scout Sessions".into()),
        '&' => Err("background execution is not allowed for scout Sessions".into()),
        c if c.is_whitespace() => Ok(Some(ScoutSyntaxStep::Space(c.len_utf8()))),
        _ => Ok(None),
    }
}

fn tokenize_scout_units(command: &str) -> Result<Vec<Vec<String>>, String> {
    let mut units = Vec::new();
    let mut unit = Vec::new();
    let mut word = String::new();
    let (mut single, mut double, mut has_token) = (false, false, false);
    let mut i = 0;
    while i < command.len() {
        let rest = &command[i..];
        let c = rest.chars().next().ok_or("invalid command")?;
        if !single && c == '\\' {
            has_token = true;
            i += c.len_utf8();
            if let Some(next) = command[i..].chars().next() {
                word.push(next);
                i += next.len_utf8();
            }
            continue;
        }
        if !double && c == '\'' {
            has_token = true;
            single = !single;
            i += 1;
            continue;
        }
        if !single && c == '"' {
            has_token = true;
            double = !double;
            i += 1;
            continue;
        }
        if !single && (c == '`' || rest.starts_with("$(")) {
            return Err("command substitution is not allowed for scout Sessions".into());
        }
        if !single && !double {
            if let Some(step) = match_unquoted_scout_syntax(rest, has_token)? {
                match step {
                    ScoutSyntaxStep::Skip(len) => i += len,
                    ScoutSyntaxStep::Space(len) => {
                        if has_token {
                            unit.push(std::mem::take(&mut word));
                            has_token = false;
                        }
                        i += len;
                    }
                    ScoutSyntaxStep::Split(len) => {
                        if has_token {
                            unit.push(std::mem::take(&mut word));
                            has_token = false;
                        }
                        if unit.is_empty() {
                            return Err("empty command unit in pipeline or chain".into());
                        }
                        units.push(std::mem::take(&mut unit));
                        i += len;
                    }
                }
                continue;
            }
        }
        has_token = true;
        word.push(c);
        i += c.len_utf8();
    }
    if single || double {
        return Err("unclosed quote in command".into());
    }
    if has_token {
        unit.push(word);
    }
    if unit.is_empty() {
        return Err("empty command unit in pipeline or chain".into());
    }
    units.push(unit);
    Ok(units)
}

fn has_short_flag(arg: &str, flag: char) -> bool {
    arg.starts_with('-') && !arg.starts_with("--") && arg[1..].contains(flag)
}

fn validate_scout_argv(words: &[String]) -> Result<(), String> {
    let cmd = words.first().map(String::as_str).unwrap_or_default();
    if cmd.is_empty() || cmd.contains('=') || cmd.contains('/') {
        return Err(format!(
            "command `{cmd}` is not allowed in read-only scout bash"
        ));
    }
    let args = &words[1..];
    match cmd {
        "ls" | "grep" | "cat" | "head" | "tail" | "wc" | "stat" | "which" | "pwd" | "jq" => Ok(()),
        "tree" => validate_scout_tree(args),
        "file" => validate_scout_file(args),
        "rg" => validate_scout_rg(args),
        "fd" => validate_scout_fd(args),
        "find" => validate_scout_find(args),
        "git" => validate_scout_git(args),
        "cargo" => validate_scout_cargo(args),
        "rtk" => validate_scout_rtk(args),
        other => Err(format!(
            "command `{other}` is not allowed in read-only scout bash"
        )),
    }
}

fn validate_scout_tree(args: &[String]) -> Result<(), String> {
    if args
        .iter()
        .any(|a| has_short_flag(a, 'o') || a == "--output" || a.starts_with("--output="))
    {
        return Err("tree -o/--output is not allowed for scout Sessions".into());
    }
    Ok(())
}

fn validate_scout_file(args: &[String]) -> Result<(), String> {
    if args
        .iter()
        .any(|a| has_short_flag(a, 'C') || a == "--compile")
    {
        return Err("file -C/--compile is not allowed for scout Sessions".into());
    }
    Ok(())
}

fn validate_scout_rg(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| {
        a == "--pre"
            || a.starts_with("--pre=")
            || a == "--hostname-bin"
            || a.starts_with("--hostname-bin=")
    }) {
        return Err("rg --pre and --hostname-bin are not allowed for scout Sessions".into());
    }
    Ok(())
}

fn validate_scout_fd(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| {
        matches!(a.as_str(), "-x" | "--exec" | "-X" | "--exec-batch")
            || a.starts_with("--exec=")
            || a.starts_with("--exec-batch=")
    }) {
        return Err("fd --exec is not allowed for scout Sessions".into());
    }
    Ok(())
}

fn validate_scout_find(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| {
        matches!(
            a.as_str(),
            "-delete"
                | "-exec"
                | "-execdir"
                | "-ok"
                | "-okdir"
                | "-fprint"
                | "-fprint0"
                | "-fprintf"
                | "-fls"
        )
    }) {
        return Err("mutating or exec find flags are not allowed for scout Sessions".into());
    }
    Ok(())
}

fn validate_scout_rtk(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("rtk requires an allowed subcommand for scout Sessions".into());
    }
    if args.first().map(String::as_str) == Some("rtk") {
        return Err("nested rtk invocations are not allowed for scout Sessions".into());
    }
    validate_scout_argv(args)
}

fn validate_scout_git(args: &[String]) -> Result<(), String> {
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if !arg.starts_with('-') {
            break;
        }
        if matches!(arg, "-C" | "--git-dir" | "--work-tree") {
            i += 2;
            continue;
        }
        if arg.starts_with("--git-dir=")
            || arg.starts_with("--work-tree=")
            || matches!(
                arg,
                "--no-pager" | "--literal-pathspecs" | "--no-optional-locks"
            )
        {
            i += 1;
            continue;
        }
        return Err(format!(
            "git global flag `{arg}` is not allowed for scout Sessions"
        ));
    }
    let Some(subcmd) = args.get(i).map(String::as_str) else {
        return Err("git subcommand is required for scout Sessions".into());
    };
    let rest = &args[i + 1..];
    if rest.iter().any(|a| {
        matches!(
            a.as_str(),
            "--output" | "--ext-diff" | "--textconv" | "--open-files-in-pager"
        ) || a.starts_with("--output=")
            || a.starts_with("--open-files-in-pager=")
    }) {
        return Err("mutating or external git flags are not allowed for scout Sessions".into());
    }
    match subcmd {
        "diff" if rest.iter().any(|a| a == "--no-index") => {
            Err("git diff --no-index is not allowed for scout Sessions".into())
        }
        "grep" if rest.iter().any(|a| has_short_flag(a, 'O')) => {
            Err("git grep -O is not allowed for scout Sessions".into())
        }
        "status" | "diff" | "log" | "show" | "rev-parse" | "ls-files" | "blame" | "grep"
        | "ls-tree" | "cat-file" => Ok(()),
        "branch" => validate_scout_git_branch(rest),
        other => Err(format!(
            "git subcommand `{other}` is not allowed for scout Sessions"
        )),
    }
}

fn validate_scout_git_branch(args: &[String]) -> Result<(), String> {
    let mut list_mode = false;
    for arg in args {
        if matches!(
            arg.as_str(),
            "-d" | "-D"
                | "-m"
                | "-M"
                | "-c"
                | "-C"
                | "-f"
                | "-u"
                | "--delete"
                | "--move"
                | "--copy"
                | "--force"
                | "--edit-description"
                | "--set-upstream-to"
                | "--unset-upstream"
        ) || arg.starts_with("--set-upstream-to=")
        {
            return Err("mutating git branch flags are not allowed for scout Sessions".into());
        }
        if matches!(
            arg.as_str(),
            "-l" | "--list" | "-a" | "--all" | "-r" | "--remotes" | "--show-current"
        ) {
            list_mode = true;
        } else if !arg.starts_with('-') && !list_mode {
            return Err("creating git branches is not allowed for scout Sessions".into());
        }
    }
    Ok(())
}

fn validate_scout_cargo(args: &[String]) -> Result<(), String> {
    let Some(subcmd) = args.first().map(String::as_str) else {
        return Err("cargo subcommand is required for scout Sessions".into());
    };
    if !matches!(subcmd, "check" | "metadata" | "tree") {
        return Err(format!(
            "cargo subcommand `{subcmd}` is not allowed for scout Sessions"
        ));
    }
    if args[1..]
        .iter()
        .any(|a| a == "--config" || a.starts_with("--config="))
    {
        return Err("cargo --config is not allowed for scout Sessions".into());
    }
    Ok(())
}
