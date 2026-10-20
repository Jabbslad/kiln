use std::fmt::Write as _;

const ATTEST_CASES: &[(&str, &str, bool)] = &[
    ("push", "refs/heads/main", true),
    ("push", "refs/tags/v0.4.12", true),
    ("workflow_dispatch", "refs/heads/main", true),
    ("push", "refs/heads/feature", false),
    ("workflow_dispatch", "refs/heads/feature", false),
    ("workflow_dispatch", "refs/tags/v0.4.12", false),
    ("pull_request", "refs/pull/1/merge", false),
    ("pull_request_target", "refs/heads/main", false),
    ("merge_group", "refs/heads/main", false),
    ("push", "refs/tags/latest", false),
];

pub fn attest_violations(workflow: &str) -> Vec<String> {
    let Some(expr) = job_if(workflow, "attest") else {
        return vec!["attest job has no if".to_string()];
    };
    let parsed = match parse_expr(&expr) {
        Ok(parsed) => parsed,
        Err(error) => return vec![format!("attest if cannot be checked: {error}")],
    };
    let mut violations = Vec::new();
    for (event, git_ref, allowed) in ATTEST_CASES {
        let ctx = Ctx {
            event,
            git_ref,
            cancelled: false,
            needs_success: true,
        };
        match parsed.eval(&ctx) {
            Ok(actual) if actual == *allowed => {}
            Ok(actual) => violations.push(format!(
                "attest if on {event} {git_ref} is {actual}, want {allowed}"
            )),
            Err(error) => violations.push(format!("attest if on {event} {git_ref}: {error}")),
        }
    }
    violations
}

pub fn secret_violations(workflow: &str) -> Vec<String> {
    let mut violations = Vec::new();
    for script in run_scripts(workflow) {
        for expr in github_expressions(&script) {
            if expression_is_secret(&expr) {
                violations.push(format!("run script contains {expr}"));
            }
        }
    }
    violations
}

pub fn download_violations(script: &str) -> Vec<String> {
    let lines: Vec<&str> = script.lines().collect();
    let commands = shell_commands(&lines);
    let mut violations = Vec::new();
    for command in &commands {
        if !is_download(&command.text) {
            continue;
        }
        for line in lines.iter().skip(command.end_line + 1) {
            if is_checksum(line) {
                break;
            }
            if is_use(line) {
                violations.push(format!(
                    "download on line {} is used before a checksum",
                    command.start_line + 1
                ));
                break;
            }
        }
    }
    violations
}

struct Ctx<'a> {
    event: &'a str,
    git_ref: &'a str,
    cancelled: bool,
    needs_success: bool,
}

enum Expr {
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Eq(Value, Value),
    Cancelled,
    StartsWith(Value, Value),
}

enum Value {
    Event,
    Ref,
    Needs,
    Lit(String),
}

impl Expr {
    fn eval(&self, ctx: &Ctx<'_>) -> Result<bool, String> {
        match self {
            Expr::Not(inner) => Ok(!inner.eval(ctx)?),
            Expr::And(left, right) => Ok(left.eval(ctx)? && right.eval(ctx)?),
            Expr::Or(left, right) => Ok(left.eval(ctx)? || right.eval(ctx)?),
            Expr::Cancelled => Ok(ctx.cancelled),
            Expr::StartsWith(left, right) => match (left, right) {
                (Value::Ref, Value::Lit(prefix)) => Ok(ctx.git_ref.starts_with(prefix)),
                _ => Err("startsWith only checks github.ref".to_string()),
            },
            Expr::Eq(left, right) => eval_eq(left, right, ctx),
        }
    }
}

fn eval_eq(left: &Value, right: &Value, ctx: &Ctx<'_>) -> Result<bool, String> {
    match (left, right) {
        (Value::Event, Value::Lit(expected)) => Ok(ctx.event == expected),
        (Value::Ref, Value::Lit(expected)) => Ok(ctx.git_ref == expected),
        (Value::Needs, Value::Lit(expected)) if expected == "success" => Ok(ctx.needs_success),
        _ => Err("comparison is outside the attest event check".to_string()),
    }
}

fn job_if(workflow: &str, job: &str) -> Option<String> {
    let lines: Vec<&str> = workflow.lines().collect();
    let header = format!("  {job}:");
    let start = lines.iter().position(|line| line.trim_end() == header)?;
    let mut body_end = lines.len();
    for (offset, line) in lines.iter().enumerate().skip(start + 1) {
        if line.len() >= 2
            && line.starts_with("  ")
            && !line.starts_with("   ")
            && line.trim_end().ends_with(':')
            && !line.trim_start().starts_with('#')
        {
            body_end = offset;
            break;
        }
    }
    let body = &lines[start..body_end];
    let index = body.iter().position(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("if:") && line.len() - trimmed.len() == 4
    })?;
    Some(unfold_if(body, index))
}

fn unfold_if(lines: &[&str], index: usize) -> String {
    let line = lines[index];
    let key_indent = line.len() - line.trim_start().len();
    let after = line
        .split_once("if:")
        .map(|(_, rest)| rest.trim())
        .unwrap_or("");
    if !matches!(after, ">" | ">-" | "|" | "|-" | "|+" | ">+") {
        return after.to_string();
    }
    let mut parts = Vec::new();
    for nxt in &lines[index + 1..] {
        if nxt.trim().is_empty() {
            continue;
        }
        let indent = nxt.len() - nxt.trim_start().len();
        if indent <= key_indent {
            break;
        }
        parts.push(nxt.trim());
    }
    parts.join(" ")
}

struct Parser<'a> {
    bytes: &'a [u8],
    index: usize,
}

fn parse_expr(input: &str) -> Result<Expr, String> {
    let mut parser = Parser {
        bytes: input.as_bytes(),
        index: 0,
    };
    let expr = parser.parse_or()?;
    parser.skip();
    if parser.index != parser.bytes.len() {
        return Err(format!(
            "leftover {:?}",
            &input[parser.index..parser.index.saturating_add(24).min(input.len())]
        ));
    }
    Ok(expr)
}

impl<'a> Parser<'a> {
    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        loop {
            self.skip();
            if self.eat("||") {
                let right = self.parse_and()?;
                left = Expr::Or(Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        loop {
            self.skip();
            if self.eat("&&") {
                let right = self.parse_unary()?;
                left = Expr::And(Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        self.skip();
        if self.eat("!") {
            return Ok(Expr::Not(Box::new(self.parse_unary()?)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        self.skip();
        if self.eat("(") {
            let inner = self.parse_or()?;
            self.skip();
            if !self.eat(")") {
                return Err("missing )".to_string());
            }
            return Ok(inner);
        }
        let ident = self.ident()?;
        self.skip();
        if ident == "cancelled" {
            if !self.eat("(") || !self.eat_after_skip(")") {
                return Err("cancelled() is incomplete".to_string());
            }
            return Ok(Expr::Cancelled);
        }
        if ident == "startsWith" {
            if !self.eat("(") {
                return Err("startsWith( is incomplete".to_string());
            }
            let left = self.parse_value()?;
            self.skip();
            if !self.eat(",") {
                return Err("startsWith needs two arguments".to_string());
            }
            let right = self.parse_value()?;
            self.skip();
            if !self.eat(")") {
                return Err("startsWith( is not closed".to_string());
            }
            return Ok(Expr::StartsWith(left, right));
        }
        let left = named_value(&ident)?;
        self.skip();
        if !self.eat("==") {
            return Err(format!("unsupported expression {ident}"));
        }
        let right = self.parse_value()?;
        Ok(Expr::Eq(left, right))
    }

    fn parse_value(&mut self) -> Result<Value, String> {
        self.skip();
        if self
            .bytes
            .get(self.index)
            .is_some_and(|byte| *byte == b'\'' || *byte == b'"')
        {
            return Ok(Value::Lit(self.string()?));
        }
        named_value(&self.ident()?)
    }

    fn ident(&mut self) -> Result<String, String> {
        self.skip();
        let start = self.index;
        while self.index < self.bytes.len() {
            let byte = self.bytes[self.index];
            if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.' || byte == b'-' {
                self.index += 1;
            } else {
                break;
            }
        }
        if start == self.index {
            return Err("missing name".to_string());
        }
        std::str::from_utf8(&self.bytes[start..self.index])
            .map(str::to_string)
            .map_err(|_| "name is not utf-8".to_string())
    }

    fn string(&mut self) -> Result<String, String> {
        let quote = self.bytes[self.index];
        self.index += 1;
        let start = self.index;
        while self.index < self.bytes.len() && self.bytes[self.index] != quote {
            self.index += 1;
        }
        if self.index >= self.bytes.len() {
            return Err("string is not closed".to_string());
        }
        let text = std::str::from_utf8(&self.bytes[start..self.index])
            .map_err(|_| "string is not utf-8".to_string())?
            .to_string();
        self.index += 1;
        Ok(text)
    }

    fn eat(&mut self, token: &str) -> bool {
        if self.bytes[self.index..].starts_with(token.as_bytes()) {
            self.index += token.len();
            true
        } else {
            false
        }
    }

    fn eat_after_skip(&mut self, token: &str) -> bool {
        self.skip();
        self.eat(token)
    }

    fn skip(&mut self) {
        while self.index < self.bytes.len() && self.bytes[self.index].is_ascii_whitespace() {
            self.index += 1;
        }
    }
}

fn named_value(ident: &str) -> Result<Value, String> {
    if ident == "github.event_name" {
        return Ok(Value::Event);
    }
    if ident == "github.ref" {
        return Ok(Value::Ref);
    }
    if let Some(name) = ident
        .strip_prefix("needs.")
        .and_then(|rest| rest.strip_suffix(".result"))
        && !name.is_empty()
    {
        return Ok(Value::Needs);
    }
    Err(format!("unknown name {ident}"))
}

pub fn run_scripts(workflow: &str) -> Vec<String> {
    let lines: Vec<&str> = workflow.lines().collect();
    let mut scripts = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim_end_matches('\r');
        let Some(rest) = run_key_rest(line) else {
            index += 1;
            continue;
        };
        let key_indent = line.len() - line.trim_start().len();
        let indicator = rest.trim();
        if matches!(indicator, "" | ">" | ">-" | "|" | "|-" | "|+" | ">+") {
            let mut script = String::new();
            index += 1;
            while index < lines.len() {
                let next = lines[index].trim_end_matches('\r');
                if next.trim().is_empty() {
                    let _ = writeln!(script);
                    index += 1;
                    continue;
                }
                let indent = next.len() - next.trim_start().len();
                if indent <= key_indent {
                    break;
                }
                let _ = writeln!(script, "{next}");
                index += 1;
            }
            scripts.push(script);
        } else {
            scripts.push(indicator.to_string());
            index += 1;
        }
    }
    scripts
}

fn run_key_rest(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let body = trimmed
        .strip_prefix("- ")
        .map(str::trim_start)
        .unwrap_or(trimmed);
    body.strip_prefix("run:")
}

fn github_expressions(script: &str) -> Vec<String> {
    let mut found = Vec::new();
    let bytes = script.as_bytes();
    let mut index = 0;
    while index + 3 < bytes.len() {
        if &bytes[index..index + 3] == b"${{" {
            let start = index;
            if let Some(end) = script[index + 3..].find("}}") {
                found.push(script[start..index + 3 + end + 2].to_string());
                index += 3 + end + 2;
                continue;
            }
        }
        index += 1;
    }
    found
}

fn expression_is_secret(expr: &str) -> bool {
    let inner = expr
        .trim_start_matches("${{")
        .trim_end_matches("}}")
        .trim()
        .to_ascii_lowercase();
    inner.contains("secrets.")
        || inner.contains("secrets[")
        || inner.contains("github.token")
        || inner.contains("secrets.github_token")
}

struct ShellCommand {
    start_line: usize,
    end_line: usize,
    text: String,
}

fn shell_commands(lines: &[&str]) -> Vec<ShellCommand> {
    let mut commands = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let start = index;
        let mut text = String::new();
        loop {
            let piece = lines[index].trim_end_matches('\r');
            let continued = piece.trim_end().ends_with('\\');
            let visible = if continued {
                piece.trim_end().trim_end_matches('\\')
            } else {
                piece
            };
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(visible.trim());
            if !continued {
                break;
            }
            index += 1;
            if index >= lines.len() {
                break;
            }
        }
        commands.push(ShellCommand {
            start_line: start,
            end_line: index,
            text,
        });
        index += 1;
    }
    commands
}

fn is_download(command: &str) -> bool {
    if !matches!(leading_command(command), Some("curl" | "wget")) {
        return false;
    }
    let targets = output_targets(command);
    !targets.is_empty() && targets.iter().any(|target| *target != "/dev/null")
}

fn leading_command(text: &str) -> Option<&str> {
    for word in text.split_whitespace() {
        let word = word.trim_matches(|ch: char| "\"'`;".contains(ch));
        if word.is_empty() || word == "sudo" || word == "command" || word == "env" {
            continue;
        }
        if let Some((_, value)) = word.split_once('=')
            && !word.starts_with('-')
        {
            let name = value.trim_start_matches("$(").trim_start_matches('(');
            if name == "curl" || name == "wget" {
                return Some(name);
            }
            continue;
        }
        let name = word.trim_start_matches("$(").trim_start_matches('(');
        return Some(name.rsplit('/').next().unwrap_or(name));
    }
    None
}

fn output_targets(command: &str) -> Vec<&str> {
    let tokens: Vec<&str> = command
        .split_whitespace()
        .map(|token| token.trim_matches(|ch: char| ch == '"' || ch == '\'' || ch == '`'))
        .collect();
    let mut targets = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if *token == "-o" || *token == "-O" || *token == "--output" {
            if let Some(next) = tokens.get(index + 1) {
                targets.push(*next);
            }
        } else if let Some(rest) = token.strip_prefix("--output=") {
            targets.push(rest);
        } else if let Some(rest) = token.strip_prefix("-o")
            && !rest.is_empty()
            && !token.starts_with("--")
        {
            targets.push(rest);
        }
    }
    targets
}

fn is_checksum(line: &str) -> bool {
    let text = strip_comment(line);
    if text.contains("command -v") {
        return false;
    }
    text.contains("sha256sum") || text.contains("sha512sum") || text.contains("shasum")
}

fn is_use(line: &str) -> bool {
    let text = strip_comment(line).trim();
    if text.is_empty() {
        return false;
    }
    let mut command = None;
    for word in text.split_whitespace() {
        let word = word.trim_matches(|ch: char| "\"'`;".contains(ch));
        if word.is_empty() {
            continue;
        }
        if command.is_none() && word.contains('=') && !word.starts_with('-') {
            continue;
        }
        if word == "sudo" || word == "command" {
            continue;
        }
        command = Some(word);
        break;
    }
    matches!(command, Some("tar" | "install"))
}

fn strip_comment(line: &str) -> &str {
    line.split_once(" #").map(|(head, _)| head).unwrap_or(line)
}
