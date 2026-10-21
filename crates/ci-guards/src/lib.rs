use std::collections::{BTreeMap, BTreeSet};
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
            jobs: None,
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

struct Gate {
    name: &'static str,
    event: &'static str,
    git_ref: &'static str,
    cancelled: bool,
    changes: &'static str,
    heavy: &'static str,
    reuse: &'static str,
    mode: &'static str,
    proved: &'static str,
    server_check: &'static str,
    packages: &'static str,
    want_publish: bool,
    want_server_check: bool,
}

const PUBLISH_GATES: &[Gate] = &[
    Gate {
        name: "reuse path",
        event: "push",
        git_ref: "refs/tags/v0.4.14",
        cancelled: false,
        changes: "success",
        heavy: "true",
        reuse: "success",
        mode: "reuse",
        proved: "success",
        server_check: "skipped",
        packages: "skipped",
        want_publish: true,
        want_server_check: false,
    },
    Gate {
        name: "reuse path with a failed tag server-check",
        event: "push",
        git_ref: "refs/tags/v0.4.14",
        cancelled: false,
        changes: "success",
        heavy: "true",
        reuse: "success",
        mode: "reuse",
        proved: "success",
        server_check: "failure",
        packages: "skipped",
        want_publish: true,
        want_server_check: false,
    },
    Gate {
        name: "reuse path without server-check proof",
        event: "push",
        git_ref: "refs/tags/v0.4.14",
        cancelled: false,
        changes: "success",
        heavy: "true",
        reuse: "success",
        mode: "reuse",
        proved: "",
        server_check: "skipped",
        packages: "skipped",
        want_publish: false,
        want_server_check: true,
    },
    Gate {
        name: "rebuild path",
        event: "push",
        git_ref: "refs/tags/v0.4.14",
        cancelled: false,
        changes: "success",
        heavy: "true",
        reuse: "success",
        mode: "fallback",
        proved: "",
        server_check: "skipped",
        packages: "success",
        want_publish: false,
        want_server_check: true,
    },
    Gate {
        name: "rebuild path with server-check",
        event: "push",
        git_ref: "refs/tags/v0.4.14",
        cancelled: false,
        changes: "success",
        heavy: "true",
        reuse: "success",
        mode: "fallback",
        proved: "",
        server_check: "success",
        packages: "success",
        want_publish: true,
        want_server_check: true,
    },
    Gate {
        name: "rebuild path with a failed package",
        event: "push",
        git_ref: "refs/tags/v0.4.14",
        cancelled: false,
        changes: "success",
        heavy: "true",
        reuse: "failure",
        mode: "",
        proved: "",
        server_check: "success",
        packages: "failure",
        want_publish: false,
        want_server_check: true,
    },
    Gate {
        name: "cancelled tag",
        event: "push",
        git_ref: "refs/tags/v0.4.14",
        cancelled: true,
        changes: "success",
        heavy: "true",
        reuse: "success",
        mode: "reuse",
        proved: "success",
        server_check: "skipped",
        packages: "skipped",
        want_publish: false,
        want_server_check: false,
    },
    Gate {
        name: "main push",
        event: "push",
        git_ref: "refs/heads/main",
        cancelled: false,
        changes: "success",
        heavy: "true",
        reuse: "skipped",
        mode: "",
        proved: "",
        server_check: "success",
        packages: "success",
        want_publish: false,
        want_server_check: true,
    },
    Gate {
        name: "docs pull request",
        event: "pull_request",
        git_ref: "refs/pull/1/merge",
        cancelled: false,
        changes: "success",
        heavy: "false",
        reuse: "skipped",
        mode: "",
        proved: "",
        server_check: "skipped",
        packages: "skipped",
        want_publish: false,
        want_server_check: false,
    },
    Gate {
        name: "code pull request",
        event: "pull_request",
        git_ref: "refs/pull/2/merge",
        cancelled: false,
        changes: "success",
        heavy: "true",
        reuse: "skipped",
        mode: "",
        proved: "",
        server_check: "success",
        packages: "skipped",
        want_publish: false,
        want_server_check: true,
    },
];

pub fn publish_server_check_violations(workflow: &str) -> Vec<String> {
    let Some(publish_expr) = job_if(workflow, "draft-release") else {
        return vec!["draft-release job has no if".to_string()];
    };
    let Some(server_expr) = job_if(workflow, "server-check") else {
        return vec!["server-check job has no if".to_string()];
    };
    let publish = match parse_expr(&bare_expr(&publish_expr)) {
        Ok(parsed) => parsed,
        Err(error) => return vec![format!("draft-release if cannot be checked: {error}")],
    };
    let server = match parse_expr(&bare_expr(&server_expr)) {
        Ok(parsed) => parsed,
        Err(error) => return vec![format!("server-check if cannot be checked: {error}")],
    };
    let mut violations = Vec::new();
    for case in PUBLISH_GATES {
        let changes_outputs = [("heavy", case.heavy)];
        let reuse_outputs = [("mode", case.mode), ("server_check", case.proved)];
        let jobs = [
            (
                "changes",
                JobState {
                    result: case.changes,
                    outputs: &changes_outputs,
                },
            ),
            (
                "reuse",
                JobState {
                    result: case.reuse,
                    outputs: &reuse_outputs,
                },
            ),
            (
                "version-guard",
                JobState {
                    result: "success",
                    outputs: &[],
                },
            ),
            (
                "server-check",
                JobState {
                    result: case.server_check,
                    outputs: &[],
                },
            ),
            (
                "server-package",
                JobState {
                    result: case.packages,
                    outputs: &[],
                },
            ),
            (
                "client",
                JobState {
                    result: case.packages,
                    outputs: &[],
                },
            ),
            (
                "dashboard",
                JobState {
                    result: case.packages,
                    outputs: &[],
                },
            ),
            (
                "attest",
                JobState {
                    result: case.packages,
                    outputs: &[],
                },
            ),
        ];
        let ctx = Ctx {
            event: case.event,
            git_ref: case.git_ref,
            cancelled: case.cancelled,
            needs_success: false,
            jobs: Some(&jobs),
        };
        match publish.eval(&ctx) {
            Ok(actual) if actual == case.want_publish => {}
            Ok(actual) => violations.push(format!(
                "{}: publish is {actual}, want {}",
                case.name, case.want_publish
            )),
            Err(error) => violations.push(format!("{}: publish if: {error}", case.name)),
        }
        match server.eval(&ctx) {
            Ok(actual) if actual == case.want_server_check => {}
            Ok(actual) => violations.push(format!(
                "{}: server-check is {actual}, want {}",
                case.name, case.want_server_check
            )),
            Err(error) => violations.push(format!("{}: server-check if: {error}", case.name)),
        }
    }
    violations
}

fn bare_expr(expr: &str) -> String {
    let trimmed = expr.trim();
    let trimmed = trimmed.strip_prefix("${{").unwrap_or(trimmed).trim();
    trimmed
        .strip_suffix("}}")
        .unwrap_or(trimmed)
        .trim()
        .to_string()
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

struct JobState<'a> {
    result: &'a str,
    outputs: &'a [(&'a str, &'a str)],
}

struct Ctx<'a> {
    event: &'a str,
    git_ref: &'a str,
    cancelled: bool,
    needs_success: bool,
    jobs: Option<&'a [(&'a str, JobState<'a>)]>,
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
    NeedResult(String),
    NeedOutput(String, String),
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
        (Value::NeedResult(job), Value::Lit(expected)) => match ctx.jobs {
            Some(jobs) => Ok(job_result(jobs, job) == expected),
            None if expected == "success" => Ok(ctx.needs_success),
            None => Err(format!(
                "needs.{job}.result is outside the checked comparison"
            )),
        },
        (Value::NeedOutput(job, name), Value::Lit(expected)) => {
            let Some(jobs) = ctx.jobs else {
                return Err(format!("needs.{job}.outputs.{name} needs a job map"));
            };
            Ok(job_output(jobs, job, name) == expected)
        }
        _ => Err("comparison is outside the attest event check".to_string()),
    }
}

fn job_state<'a>(jobs: &'a [(&'a str, JobState<'a>)], name: &str) -> Option<&'a JobState<'a>> {
    jobs.iter()
        .find(|(job, _)| *job == name)
        .map(|(_, state)| state)
}

fn job_result<'a>(jobs: &'a [(&'a str, JobState<'a>)], name: &str) -> &'a str {
    job_state(jobs, name)
        .map(|state| state.result)
        .unwrap_or("")
}

fn job_output<'a>(jobs: &'a [(&'a str, JobState<'a>)], job: &str, name: &str) -> &'a str {
    job_state(jobs, job)
        .and_then(|state| {
            state
                .outputs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| *value)
        })
        .unwrap_or("")
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
        let negated = if self.eat("!=") {
            true
        } else if self.eat("==") {
            false
        } else {
            return Err(format!("unsupported expression {ident}"));
        };
        let right = self.parse_value()?;
        let eq = Expr::Eq(left, right);
        if negated {
            Ok(Expr::Not(Box::new(eq)))
        } else {
            Ok(eq)
        }
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
    if let Some(rest) = ident.strip_prefix("needs.") {
        if let Some((job, output)) = rest.split_once(".outputs.")
            && !job.is_empty()
            && !output.is_empty()
            && !job.contains('.')
            && !output.contains('.')
        {
            return Ok(Value::NeedOutput(job.to_string(), output.to_string()));
        }
        if let Some(name) = rest.strip_suffix(".result")
            && !name.is_empty()
            && !name.contains('.')
        {
            return Ok(Value::NeedResult(name.to_string()));
        }
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

pub fn reuse_lookup_violations(workflow: &str) -> Vec<String> {
    let jobs = job_blocks(workflow);
    let mut violations = Vec::new();
    let Some((_, reuse)) = jobs.iter().find(|(name, _)| name == "reuse") else {
        return vec!["reuse job is missing".to_string()];
    };
    let decide = step_from(reuse, "id: decide");
    if decide.contains("github.ref_name") {
        violations.push(
            "reuse looks up archives by the git tag; main names them with the committed workspace version"
                .to_string(),
        );
    }
    if !decide.contains("commits/main") {
        violations
            .push("a dry run with an empty reuse_sha does not look up main's commit".to_string());
    }
    let Some((_, draft)) = jobs.iter().find(|(name, _)| name == "draft-release") else {
        violations.push("draft-release job is missing".to_string());
        return violations;
    };
    if draft.contains("needs.reuse.outputs.version") || !draft.contains("github.ref_name") {
        violations.push(
            "draft-release finish checks main's archive names instead of the git tag".to_string(),
        );
    }
    violations
}

/// A v* publish stamps reused packages onto the tag when that tag is not the
/// version main built, then checks the Linux binary before draft-release runs.
pub fn version_guard_violations(workflow: &str) -> Vec<String> {
    let jobs = job_blocks(workflow);
    let mut violations = Vec::new();
    match jobs.iter().find(|(name, _)| name == "version-guard") {
        None => violations.push("version-guard job is missing".to_string()),
        Some((_, guard)) => {
            let guard_if = job_if(workflow, "version-guard").unwrap_or_default();
            if !guard_if.contains("github.event_name == 'push'")
                || !guard_if.contains("startsWith(github.ref, 'refs/tags/v')")
            {
                violations.push("version-guard does not run on v* tag pushes".to_string());
            }
            if guard.contains("scripts/check-tag-version.sh") || guard.contains("Cargo.toml") {
                violations.push(
                    "version-guard still requires the tag to equal the Cargo.toml version"
                        .to_string(),
                );
            }
            if !guard.contains("scripts/stamp-release-dist.sh") {
                violations
                    .push("version-guard does not stamp reused packages onto the tag".to_string());
            }
            if !guard.contains("actions/attest-build-provenance") {
                violations.push("version-guard does not attest packages it stamps".to_string());
            }
            if !guard.contains("check-binary")
                || !guard.contains("x86_64-unknown-linux-gnu")
                || !guard.contains("reused-dist")
            {
                violations.push(
                    "version-guard does not run check-binary on the reused Linux archive"
                        .to_string(),
                );
            }
        }
    }
    let Some((_, draft)) = jobs.iter().find(|(name, _)| name == "draft-release") else {
        violations.push("draft-release job is missing".to_string());
        return violations;
    };
    if !needs_job(draft, "version-guard") {
        violations.push("publish does not depend on version-guard".to_string());
    }
    let draft_if = job_if(workflow, "draft-release").unwrap_or_default();
    if !draft_if.contains("needs.version-guard.result == 'success'") {
        violations.push("publish does not require version-guard to succeed".to_string());
    }
    if draft.contains("${TAG#v}") {
        violations.push("publish renames archives from the Cargo version to the tag".to_string());
    }
    violations
}

fn step_from(job: &str, marker: &str) -> String {
    let lines: Vec<&str> = job.lines().collect();
    let Some(start) = lines.iter().position(|line| line.contains(marker)) else {
        return String::new();
    };
    let mut body = vec![lines[start]];
    for line in lines.iter().skip(start + 1) {
        if line.starts_with("      - ") {
            break;
        }
        body.push(*line);
    }
    body.join("\n")
}

const REUSE_PATH_SKIPPED: &[&str] = &["server-package", "client", "dashboard", "attest"];

/// Jobs that sit downstream of a reuse-path skip must not rely on GitHub's
/// implicit `success()`. That function is false when an ancestor was skipped,
/// so a later job whose direct need succeeded still does not run.
/// `!cancelled()` or `always()` blocks the rewrite. A `!cancelled()` if also
/// names every direct need's result (`success`, or `skipped` where that need
/// is allowed to be skipped). `always()` is that allowance for a gate which
/// still runs when a need is skipped and judges the result in the job.
pub fn reuse_skip_downstream_violations(workflow: &str) -> Vec<String> {
    let jobs = job_blocks(workflow);
    let mut needs_of: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut if_of: BTreeMap<String, String> = BTreeMap::new();
    for (name, body) in &jobs {
        needs_of.insert(name.clone(), needs_items(body));
        if_of.insert(name.clone(), job_if(workflow, name).unwrap_or_default());
    }

    let mut ancestors = BTreeSet::new();
    for name in REUSE_PATH_SKIPPED {
        if needs_of.contains_key(*name) {
            ancestors.insert((*name).to_string());
        }
    }
    for (name, expr) in &if_of {
        if gated_on_reuse_mode(expr) {
            ancestors.insert(name.clone());
        }
    }

    let mut violations = Vec::new();
    for name in jobs_downstream_of(&needs_of, &ancestors) {
        let normalized = normalize_if(if_of.get(&name).map(String::as_str).unwrap_or(""));
        let always = normalized.contains("always()");
        if !normalized.starts_with("!cancelled()") && !always {
            violations.push(format!(
                "{name} is downstream of a reuse-skipped job and its if does not start with !cancelled() or use always()"
            ));
        }
        if always {
            continue;
        }
        for need in needs_of.get(&name).map(Vec::as_slice).unwrap_or(&[]) {
            if !need_result_is_checked(&normalized, need) {
                violations.push(format!(
                    "{name} does not check needs.{need}.result == 'success' or allow needs.{need}.result == 'skipped'"
                ));
            }
        }
    }
    violations
}

fn gated_on_reuse_mode(expr: &str) -> bool {
    normalize_if(expr).contains("needs.reuse.outputs.mode")
}

fn jobs_downstream_of(
    needs_of: &BTreeMap<String, Vec<String>>,
    ancestors: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (name, needs) in needs_of {
        for need in needs {
            dependents
                .entry(need.as_str())
                .or_default()
                .push(name.as_str());
        }
    }
    let mut downstream = BTreeSet::new();
    let mut stack: Vec<&str> = ancestors.iter().map(String::as_str).collect();
    while let Some(job) = stack.pop() {
        let Some(children) = dependents.get(job) else {
            continue;
        };
        for child in children {
            if downstream.insert((*child).to_string()) {
                stack.push(child);
            }
        }
    }
    downstream
}

fn normalize_if(expr: &str) -> String {
    let mut text = expr.trim().to_string();
    if let Some(inner) = strip_quotes(&text) {
        text = inner;
    }
    if let Some(rest) = text.trim().strip_prefix("${{")
        && let Some(inner) = rest.trim().strip_suffix("}}")
    {
        text = inner.trim().to_string();
        if let Some(inner) = strip_quotes(&text) {
            text = inner;
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_quotes(text: &str) -> Option<String> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        return Some(text[1..text.len() - 1].trim().to_string());
    }
    None
}

fn need_result_is_checked(expr: &str, need: &str) -> bool {
    for result in ["success", "skipped"] {
        if expr.contains(&format!("needs.{need}.result == '{result}'"))
            || expr.contains(&format!("needs.{need}.result == \"{result}\""))
        {
            return true;
        }
    }
    false
}

pub fn deploy_caller_violations(workflow: &str) -> Vec<String> {
    let jobs = job_blocks(workflow);
    let mut violations = Vec::new();
    if !jobs
        .iter()
        .any(|(_, body)| calls_deploy(body) && needs_job(body, "draft-release"))
    {
        violations.push(
            "no job calls ./.github/workflows/deploy.yml and needs draft-release".to_string(),
        );
    }
    for (name, body) in &jobs {
        if calls_deploy(body) && !needs_job(body, "draft-release") {
            violations.push(format!(
                "{name} calls ./.github/workflows/deploy.yml without needing draft-release"
            ));
        }
    }
    violations
}

/// `DARK_FORGE_*` values are secrets on the `dark-forge` environment.
/// A job that reads one must set `environment: dark-forge`.
/// A reusable workflow only receives them when the caller passes
/// `secrets: inherit` or names each secret. Otherwise GitHub sets the
/// value to an empty string and the job still runs.
pub fn dark_forge_secret_violations(workflows: &[(&str, &str)]) -> Vec<String> {
    let mut present: BTreeSet<&str> = BTreeSet::new();
    let mut reads: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    let mut violations = Vec::new();
    for (name, text) in workflows {
        present.insert(*name);
        for (job, body) in job_blocks(text) {
            let secrets = dark_forge_names(&strip_job_secrets_block(&body));
            if secrets.is_empty() {
                continue;
            }
            reads
                .entry(*name)
                .or_default()
                .extend(secrets.iter().cloned());
            if job_environment(&body).as_deref() != Some("dark-forge") {
                let listed = secrets.into_iter().collect::<Vec<_>>().join(", ");
                violations.push(format!(
                    "{name} job {job} reads {listed} without environment: dark-forge"
                ));
            }
        }
    }
    for (name, text) in workflows {
        for (job, body) in job_blocks(text) {
            let Some(called) = called_local_workflow(&body) else {
                continue;
            };
            let Some(needed) = reads.get(called.as_str()) else {
                if !present.contains(called.as_str()) {
                    violations.push(format!(
                        "{name} job {job} calls {called}, which was not provided"
                    ));
                }
                continue;
            };
            match passed_secrets(&body) {
                PassedSecrets::All => {}
                PassedSecrets::Named(passed) => {
                    for secret in needed {
                        if !passed.contains(secret) {
                            violations.push(format!(
                                "{name} job {job} calls {called} without passing {secret}"
                            ));
                        }
                    }
                }
                PassedSecrets::None => violations.push(format!(
                    "{name} job {job} calls {called} without passing secrets"
                )),
            }
        }
    }
    violations
}

enum PassedSecrets {
    None,
    All,
    Named(BTreeSet<String>),
}

fn dark_forge_names(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index..].starts_with(b"secrets.DARK_FORGE_") {
            let start = index + "secrets.".len();
            let mut end = start;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            if end > start
                && let Ok(name) = std::str::from_utf8(&bytes[start..end])
            {
                names.insert(name.to_string());
            }
            index = end;
            continue;
        }
        if bytes[index..].starts_with(b"secrets[") {
            let mut cursor = index + "secrets[".len();
            if bytes
                .get(cursor)
                .is_some_and(|byte| *byte == b'\'' || *byte == b'"')
            {
                let quote = bytes[cursor];
                cursor += 1;
                let start = cursor;
                while cursor < bytes.len() && bytes[cursor] != quote {
                    cursor += 1;
                }
                if let Ok(name) = std::str::from_utf8(&bytes[start..cursor])
                    && name.starts_with("DARK_FORGE_")
                    && name.len() > "DARK_FORGE_".len()
                {
                    names.insert(name.to_string());
                }
            }
            index = cursor.max(index + 1);
            continue;
        }
        index += 1;
    }
    names
}

fn job_environment(body: &str) -> Option<String> {
    let lines: Vec<&str> = body.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if line.len() - trimmed.len() != 4 {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("environment:") else {
            continue;
        };
        let rest = rest.trim();
        if rest.is_empty() || is_yaml_fold(rest) {
            for next in &lines[index + 1..] {
                if next.trim().is_empty() || next.trim_start().starts_with('#') {
                    continue;
                }
                if next.len() - next.trim_start().len() <= 4 {
                    break;
                }
                let Some(value) = next.trim().strip_prefix("name:") else {
                    continue;
                };
                let value = value.trim().trim_matches(|ch| ch == '"' || ch == '\'');
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
            return None;
        }
        let name = rest
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches(|ch| ch == '"' || ch == '\'');
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

fn strip_job_secrets_block(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut kept = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim_start();
        if line.len() - trimmed.len() == 4 && trimmed.starts_with("secrets:") {
            let rest = trimmed["secrets:".len()..].trim();
            index += 1;
            if rest.is_empty() || is_yaml_fold(rest) {
                while index < lines.len() {
                    let next = lines[index];
                    if next.trim().is_empty() || next.trim_start().starts_with('#') {
                        index += 1;
                        continue;
                    }
                    if next.len() - next.trim_start().len() <= 4 {
                        break;
                    }
                    index += 1;
                }
            }
            continue;
        }
        kept.push(line);
        index += 1;
    }
    kept.join("\n")
}

fn called_local_workflow(body: &str) -> Option<String> {
    for line in body.lines() {
        let trimmed = line.trim_start();
        if line.len() - trimmed.len() != 4 {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("uses:") else {
            continue;
        };
        let path = rest
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches(|ch| ch == '"' || ch == '\'');
        for prefix in ["./.github/workflows/", "$/.github/workflows/"] {
            if let Some(name) = path.strip_prefix(prefix)
                && !name.is_empty()
                && !name.contains('/')
            {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn passed_secrets(body: &str) -> PassedSecrets {
    let lines: Vec<&str> = body.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if line.len() - trimmed.len() != 4 {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("secrets:") else {
            continue;
        };
        let rest = rest.trim();
        if rest.trim_matches(|ch| ch == '"' || ch == '\'') == "inherit" {
            return PassedSecrets::All;
        }
        if !rest.is_empty() && !is_yaml_fold(rest) {
            return PassedSecrets::None;
        }
        let mut names = BTreeSet::new();
        for next in &lines[index + 1..] {
            if next.trim().is_empty() || next.trim_start().starts_with('#') {
                continue;
            }
            let indent = next.len() - next.trim_start().len();
            if indent <= 4 {
                break;
            }
            if indent == 6 {
                let key = next
                    .trim()
                    .split_once(':')
                    .map(|(key, _)| key.trim())
                    .unwrap_or("")
                    .trim_matches(|ch| ch == '"' || ch == '\'');
                if !key.is_empty() {
                    names.insert(key.to_string());
                }
            }
        }
        return PassedSecrets::Named(names);
    }
    PassedSecrets::None
}

pub fn release_event_violations(workflow: &str) -> Vec<String> {
    let lines: Vec<&str> = workflow
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .collect();
    let Some(start) = lines.iter().position(|line| is_on_header(line)) else {
        return Vec::new();
    };
    let rest = lines[start]
        .split_once(':')
        .map(|(_, rest)| rest.trim())
        .unwrap_or("");
    if inline_on_includes_release(rest) {
        return vec!["workflow triggers on release".to_string()];
    }
    if !rest.is_empty() && !is_yaml_fold(rest) {
        return Vec::new();
    }
    let mut violations = Vec::new();
    for line in &lines[start + 1..] {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 0 {
            break;
        }
        if indent == 2 && is_release_event_line(trimmed) {
            violations.push("workflow triggers on release".to_string());
        }
    }
    violations
}

pub fn main_push_violations(workflow: &str) -> Vec<String> {
    let mut violations = Vec::new();
    for script in run_scripts(workflow) {
        let lines: Vec<&str> = script.lines().collect();
        for command in shell_commands(&lines) {
            let text = strip_unquoted_comment(&command.text);
            if pushes_to_main(&text) {
                violations.push(format!("run script pushes to main: {}", one_line(&text)));
            }
            if contents_api_on_main(&text) {
                violations.push(format!(
                    "run script writes the contents API on main: {}",
                    one_line(&text)
                ));
            }
        }
    }
    violations
}

fn job_blocks(workflow: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = workflow
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .collect();
    let Some(jobs_at) = lines.iter().position(|line| line.trim() == "jobs:") else {
        return Vec::new();
    };
    let mut headers = Vec::new();
    let mut section_end = lines.len();
    for (offset, line) in lines.iter().enumerate().skip(jobs_at + 1) {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 0 {
            section_end = offset;
            break;
        }
        if indent == 2 && line.trim_end().ends_with(':') {
            headers.push((offset, line.trim().trim_end_matches(':').trim().to_string()));
        }
    }
    let mut blocks = Vec::new();
    for (index, (start, name)) in headers.iter().enumerate() {
        let next = headers
            .get(index + 1)
            .map(|(offset, _)| *offset)
            .unwrap_or(section_end);
        blocks.push((name.clone(), lines[start + 1..next].join("\n")));
    }
    blocks
}

fn calls_deploy(body: &str) -> bool {
    body.lines().any(|line| {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        indent == 4 && uses_deploy(trimmed.trim_end())
    })
}

fn uses_deploy(trimmed: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix("uses:") else {
        return false;
    };
    let path = rest.split_whitespace().next().unwrap_or("");
    path == "./.github/workflows/deploy.yml"
}

fn needs_job(body: &str, job: &str) -> bool {
    needs_items(body).iter().any(|item| item == job)
}

fn needs_items(body: &str) -> Vec<String> {
    let lines: Vec<&str> = body.lines().collect();
    let Some(index) = lines.iter().position(|line| {
        let trimmed = line.trim_start();
        line.len() - trimmed.len() == 4 && trimmed.starts_with("needs:")
    }) else {
        return Vec::new();
    };
    let after = lines[index]
        .split_once("needs:")
        .map(|(_, rest)| rest.trim())
        .unwrap_or("");
    let mut items = Vec::new();
    if after.is_empty() || is_yaml_fold(after) {
        for line in &lines[index + 1..] {
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            if line.len() - line.trim_start().len() <= 4 {
                break;
            }
            let item = line
                .trim()
                .trim_start_matches("- ")
                .trim()
                .trim_matches(|ch| ch == '"' || ch == '\'' || ch == ',');
            if !item.is_empty() {
                items.push(item.to_string());
            }
        }
    } else {
        let inner = after.trim().trim_matches(|ch| ch == '[' || ch == ']');
        for part in inner.split(',') {
            let item = part.trim().trim_matches(|ch| ch == '"' || ch == '\'');
            if !item.is_empty() {
                items.push(item.to_string());
            }
        }
    }
    items
}

fn is_on_header(line: &str) -> bool {
    let trimmed = line.trim_end();
    if trimmed.is_empty() || trimmed.starts_with(' ') || trimmed.starts_with('#') {
        return false;
    }
    let key = trimmed
        .split_once(':')
        .map(|(key, _)| key.trim())
        .unwrap_or("");
    key.trim_matches(|ch| ch == '"' || ch == '\'') == "on"
}

fn inline_on_includes_release(rest: &str) -> bool {
    let rest = rest.trim();
    if rest.is_empty() || is_yaml_fold(rest) {
        return false;
    }
    let scalar = rest.trim_matches(|ch| ch == '"' || ch == '\'');
    if scalar == "release" {
        return true;
    }
    let inner = rest.trim().trim_matches(|ch| ch == '[' || ch == ']');
    inner
        .split(',')
        .any(|item| item.trim().trim_matches(|ch| ch == '"' || ch == '\'') == "release")
}

fn is_yaml_fold(value: &str) -> bool {
    matches!(value, ">" | ">-" | "|" | "|-" | "|+" | ">+")
}

fn is_release_event_line(trimmed: &str) -> bool {
    let key = trimmed
        .split_once(':')
        .map(|(key, _)| key.trim())
        .unwrap_or(trimmed);
    key.trim_matches(|ch| ch == '"' || ch == '\'') == "release"
}

fn pushes_to_main(command: &str) -> bool {
    split_chain(command)
        .iter()
        .any(|segment| segment_pushes_to_main(segment))
}

fn segment_pushes_to_main(segment: &str) -> bool {
    let tokens = tokenize(segment);
    if git_push_destinations(&tokens)
        .iter()
        .any(|destination| destination == "main" || destination == "refs/heads/main")
    {
        return true;
    }
    tokens
        .iter()
        .any(|token| token.contains("git ") && segment_pushes_to_main(token))
}

fn git_push_destinations(tokens: &[String]) -> Vec<String> {
    let mut index = 0;
    while index < tokens.len() {
        let word = tokens[index].as_str();
        if word == "sudo" || word == "command" || word == "env" {
            index += 1;
            continue;
        }
        if word.contains('=') && !word.starts_with('-') {
            index += 1;
            continue;
        }
        break;
    }
    if index >= tokens.len() || !is_git(tokens[index].as_str()) {
        return Vec::new();
    }
    index += 1;
    while index < tokens.len() && tokens[index].starts_with('-') {
        if matches!(
            tokens[index].as_str(),
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace"
        ) {
            index += 2;
            continue;
        }
        index += 1;
    }
    if index >= tokens.len() || tokens[index] != "push" {
        return Vec::new();
    }
    index += 1;
    let mut positionals = Vec::new();
    while index < tokens.len() {
        let arg = tokens[index].as_str();
        if arg == "--" {
            positionals.extend(tokens[index + 1..].iter().cloned());
            break;
        }
        if matches!(
            arg,
            "-c" | "--repo" | "--receive-pack" | "--exec" | "-o" | "--push-option"
        ) {
            index += 2;
            continue;
        }
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        positionals.push(tokens[index].clone());
        index += 1;
    }
    let specs = if positionals.len() >= 2 {
        &positionals[1..]
    } else {
        &positionals[..]
    };
    specs.iter().map(|spec| push_destination(spec)).collect()
}

fn is_git(word: &str) -> bool {
    word == "git" || word.ends_with("/git")
}

fn push_destination(spec: &str) -> String {
    let spec = spec.trim_start_matches('+');
    spec.rsplit_once(':')
        .map(|(_, destination)| destination.to_string())
        .unwrap_or_else(|| spec.to_string())
}

fn contents_api_on_main(command: &str) -> bool {
    if !mentions_contents_api(command) {
        return false;
    }
    let branches = branch_values(&normalize_for_branch(command));
    if !branches.is_empty() {
        return branches
            .iter()
            .any(|branch| branch == "main" || branch == "refs/heads/main");
    }
    is_contents_write(command)
}

fn mentions_contents_api(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower.contains("/contents/")
        || lower.contains("/contents?")
        || lower.contains("/contents ")
        || lower.ends_with("/contents")
}

fn normalize_for_branch(command: &str) -> String {
    let mut flat = String::new();
    for ch in command.chars() {
        if matches!(ch, '"' | '\'' | '`' | '{' | '}' | ',') {
            flat.push(' ');
        } else {
            flat.push(ch.to_ascii_lowercase());
        }
    }
    flat.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn branch_values(normalized: &str) -> Vec<String> {
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    let mut values = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let next = tokens.get(index + 1).copied();
        let third = tokens.get(index + 2).copied();
        if let Some(value) = branch_assignment(token, next, third) {
            values.push(value);
        }
    }
    values
}

fn branch_assignment(token: &str, next: Option<&str>, third: Option<&str>) -> Option<String> {
    if let Some(rest) = token.strip_prefix("branch=") {
        return branch_name(rest);
    }
    if token.strip_suffix(':') == Some("branch") {
        return next.and_then(branch_name);
    }
    if token == "branch" {
        if matches!(next, Some("=" | ":")) {
            return third.and_then(branch_name);
        }
        if let Some(next) = next
            && let Some(rest) = next.strip_prefix('=').or_else(|| next.strip_prefix(':'))
        {
            return branch_name(rest);
        }
    }
    None
}

fn branch_name(value: &str) -> Option<String> {
    let value = value.trim_matches(|ch: char| ch == ':' || ch == '=' || ch == ',');
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '_' | '.' | '-'))
    {
        return None;
    }
    Some(value.to_string())
}

fn is_contents_write(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    let tokens: Vec<&str> = lower
        .split_whitespace()
        .map(|token| token.trim_matches(|ch: char| "\"'`".contains(ch)))
        .collect();
    for (index, token) in tokens.iter().enumerate() {
        if let Some(method) = write_method_token(token)
            && is_write_method(method)
        {
            return true;
        }
        if matches!(*token, "-x" | "--request" | "--method")
            && tokens
                .get(index + 1)
                .is_some_and(|next| is_write_method(next))
        {
            return true;
        }
    }
    false
}

fn write_method_token(token: &str) -> Option<&str> {
    if let Some(value) = token.strip_prefix("--method=") {
        return Some(value);
    }
    if let Some(value) = token.strip_prefix("-x")
        && !value.is_empty()
    {
        return Some(value);
    }
    None
}

fn is_write_method(value: &str) -> bool {
    matches!(value, "put" | "patch" | "post" | "delete")
}

fn split_chain(command: &str) -> Vec<String> {
    let chars: Vec<char> = command.chars().collect();
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            }
            current.push(ch);
            index += 1;
            continue;
        }
        if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
            current.push(ch);
            index += 1;
            continue;
        }
        if ch == ';' {
            parts.push(std::mem::take(&mut current));
            index += 1;
            continue;
        }
        if matches!(ch, '&' | '|') && chars.get(index + 1) == Some(&ch) {
            parts.push(std::mem::take(&mut current));
            index += 2;
            continue;
        }
        current.push(ch);
        index += 1;
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect()
}

fn tokenize(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for ch in command.chars() {
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
            continue;
        }
        if ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(ch);
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn strip_unquoted_comment(command: &str) -> String {
    let mut out = String::new();
    let mut quote = None;
    for ch in command.chars() {
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            }
            out.push(ch);
            continue;
        }
        if ch == '#' {
            break;
        }
        if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
        }
        out.push(ch);
    }
    out
}

fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 160 {
        let end = flat
            .char_indices()
            .nth(157)
            .map(|(index, _)| index)
            .unwrap_or(flat.len());
        format!("{}...", &flat[..end])
    } else {
        flat
    }
}
