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
    if !draft.contains("needs.reuse.outputs.version") || draft.contains("finish --version \"$TAG\"")
    {
        violations.push(
            "draft-release finish checks the git tag against main's archive names".to_string(),
        );
    }
    if !draft.contains("${TAG#v}") {
        violations.push(
            "publish keeps the workspace version in the asset name, so deploy cannot download kiln-server-${tag}"
                .to_string(),
        );
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

const REPIN_CASES: &[(&str, &str, bool, bool, bool)] = &[
    ("push", "refs/tags/v0.4.13", false, true, true),
    ("push", "refs/tags/v1.0.0", false, true, true),
    ("push", "refs/heads/main", false, true, false),
    ("push", "refs/heads/feature", false, true, false),
    ("push", "refs/tags/latest", false, true, false),
    ("push", "refs/tags/v0.4.13", true, true, false),
    ("push", "refs/tags/v0.4.13", false, false, false),
    ("pull_request", "refs/pull/1/merge", false, true, false),
    ("pull_request_target", "refs/heads/main", false, true, false),
    ("workflow_dispatch", "refs/heads/main", false, true, false),
    ("workflow_dispatch", "refs/tags/v0.4.13", false, true, false),
    ("merge_group", "refs/heads/main", false, true, false),
];

pub fn repin_violations(workflow: &str, files: &[(&str, &str)]) -> Vec<String> {
    let Some(body) = job_blocks(workflow)
        .into_iter()
        .find(|(name, _)| name == "repin")
        .map(|(_, body)| body)
    else {
        return vec!["repin job is missing".to_string()];
    };
    let mut violations = Vec::new();
    if !job_blocks(workflow)
        .iter()
        .any(|(name, _)| name == "deploy")
    {
        violations.push("repin job needs deploy but deploy is missing".to_string());
    }
    let needs = needs_items(&body);
    if needs != ["deploy".to_string()] {
        violations.push(format!("repin needs {needs:?}, want [\"deploy\"]"));
    }
    match job_if(workflow, "repin") {
        Some(expr) => {
            if !expr.contains("needs.deploy.result == 'success'") {
                violations.push("repin if does not wait for deploy".to_string());
            }
            match parse_expr(&expr) {
                Ok(parsed) => {
                    for (event, git_ref, cancelled, needs_success, allowed) in REPIN_CASES {
                        let ctx = Ctx {
                            event,
                            git_ref,
                            cancelled: *cancelled,
                            needs_success: *needs_success,
                        };
                        match parsed.eval(&ctx) {
                            Ok(actual) if actual == *allowed => {}
                            Ok(actual) => violations.push(format!(
                                "repin if on {event} {git_ref} is {actual}, want {allowed}"
                            )),
                            Err(error) => {
                                violations.push(format!("repin if on {event} {git_ref}: {error}"))
                            }
                        }
                    }
                }
                Err(error) => violations.push(format!("repin if cannot be checked: {error}")),
            }
        }
        None => violations.push("repin job has no if".to_string()),
    }
    if !body
        .lines()
        .any(|line| line.trim() == "REPIN_TOKEN: ${{ secrets.REPIN_TOKEN }}")
    {
        violations.push("repin token is not REPIN_TOKEN from the environment".to_string());
    }
    let shell = repin_shell(&body, files, &mut violations);
    for expr in github_expressions(&shell) {
        if expression_is_secret(&expr) {
            violations.push(format!("run script contains {expr}"));
        }
    }
    if !token_failure_is_loud(&shell) {
        violations.push("repin job skips a missing REPIN_TOKEN".to_string());
    }
    if !shell.contains("gh pr create") {
        violations.push("repin job does not run gh pr create".to_string());
    }
    if shell.contains("--draft") {
        violations.push("repin pull request is a draft".to_string());
    }
    if !shell.contains("--base main") {
        violations.push("repin pull request does not target main".to_string());
    }
    let destinations = push_destinations_in(&shell);
    if !destinations
        .iter()
        .any(|destination| destination.starts_with("repin/"))
    {
        violations.push("repin job does not push repin/<tag>".to_string());
    }
    for destination in &destinations {
        if destination == "main" || destination == "refs/heads/main" {
            violations.push(format!("repin job pushes to main: {destination}"));
        } else if !destination.starts_with("repin/") {
            violations.push(format!("repin job pushes to {destination}"));
        }
    }
    violations
}

fn repin_shell(body: &str, files: &[(&str, &str)], violations: &mut Vec<String>) -> String {
    let mut shell = String::new();
    let mut seen = Vec::new();
    for script in run_scripts(body) {
        shell.push_str(&script);
        shell.push('\n');
        append_repin_scripts(&script, files, &mut seen, &mut shell, violations);
    }
    shell
}

fn append_repin_scripts(
    script: &str,
    files: &[(&str, &str)],
    seen: &mut Vec<String>,
    shell: &mut String,
    violations: &mut Vec<String>,
) {
    for token in tokenize(script) {
        if !(token.starts_with("scripts/") && token.ends_with(".sh")) {
            continue;
        }
        if seen.iter().any(|item| item == &token) {
            continue;
        }
        seen.push(token.clone());
        match files.iter().find(|(name, _)| *name == token) {
            Some((_, text)) => {
                shell.push_str(text);
                shell.push('\n');
                append_repin_scripts(text, files, seen, shell, violations);
            }
            None => violations.push(format!("repin job runs missing {token}")),
        }
    }
}

fn token_failure_is_loud(shell: &str) -> bool {
    let compact = shell
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    compact.contains(
        "if [ -z \"${REPIN_TOKEN:-}\" ]; then\nprintf '%s\\n' 'REPIN_TOKEN is unset' >&2\nexit 1\nfi",
    )
}

fn push_destinations_in(shell: &str) -> Vec<String> {
    let lines: Vec<&str> = shell.lines().collect();
    let mut destinations = Vec::new();
    for command in shell_commands(&lines) {
        let text = strip_unquoted_comment(&command.text);
        for segment in split_chain(&text) {
            destinations.extend(git_push_destinations(&tokenize(&segment)));
        }
    }
    destinations
}
