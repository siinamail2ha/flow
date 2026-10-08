//! Sandboxed, bounded Pine indicator bridge.
//!
//! This is intentionally a chart-host runtime rather than a second scripting
//! language. It accepts the common Pine v5/v6 indicator surface used by
//! overlays (series assignments, arithmetic, `ta.*`/`math.*` helpers, `plot`,
//! `hline`, and the input defaults) and rejects unsupported control flow. The
//! limits keep a user-provided file from allocating unbounded state or
//! stalling the UI thread.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use data::chart::kline::{PineScriptConfig, drawing::DrawingColor};

pub const MAX_SOURCE_BYTES: usize = 512 * 1024;
pub const MAX_SOURCE_LINES: usize = 2_048;
pub const MAX_STATEMENTS: usize = 512;
// Pine overlays are a visual aid. Keeping a bounded working set prevents a
// collection of open charts from repeatedly evaluating very large histories
// on the UI thread.
pub const MAX_BARS: usize = 12_000;

#[derive(Debug, Clone, Copy)]
pub struct PineBar {
    pub time: u64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

#[derive(Debug, Clone)]
pub struct PinePlot {
    pub title: String,
    pub values: Vec<Option<f64>>,
    pub color: DrawingColor,
    pub line_width: f32,
}

#[derive(Debug, Clone)]
pub struct PineScriptResult {
    pub name: String,
    pub source_path: String,
    pub version: u8,
    pub plots: Vec<PinePlot>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExternalPineIndicator {
    pub name: String,
    pub path: PathBuf,
    pub version: u8,
    pub enabled: bool,
    pub error: Option<String>,
}

impl PineScriptResult {
    fn error(config: &PineScriptConfig, version: u8, error: impl Into<String>) -> Self {
        Self {
            name: config.name.clone(),
            source_path: config.source_path.clone(),
            version,
            plots: Vec::new(),
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PineRuntime {
    pub bars: Vec<PineBar>,
    configs: Vec<PineScriptConfig>,
    pub scripts: Vec<PineScriptResult>,
}

impl PineRuntime {
    pub fn empty() -> Self {
        Self {
            bars: Vec::new(),
            configs: Vec::new(),
            scripts: Vec::new(),
        }
    }

    pub fn evaluate(configs: &[PineScriptConfig], bars: &[PineBar]) -> Self {
        let scripts: Vec<PineScriptResult> = configs
            .iter()
            .filter(|config| config.enabled && !config.source.trim().is_empty())
            .map(|config| evaluate_script(config, bars))
            .collect();
        for script in &scripts {
            log::debug!(
                "PINE Loaded | name={} path={} version={} plots={} first_plot={} error={}",
                script.name,
                script.source_path,
                script.version,
                script.plots.len(),
                script.plots.first().map_or("-", |plot| plot.title.as_str()),
                script.error.as_deref().unwrap_or("-")
            );
        }
        Self {
            bars: bars.to_vec(),
            configs: configs.to_vec(),
            scripts,
        }
    }

    /// Load user indicators from the external indicator directory. This keeps
    /// post-build customization independent from the application source tree.
    pub fn load_external(bars: &[PineBar]) -> Self {
        let configs = load_external_configs();
        Self::evaluate(&configs, bars)
    }

    pub fn refresh(&mut self, bars: &[PineBar]) {
        *self = Self::evaluate(&self.configs, bars);
    }

    pub fn has_visible_plots(&self) -> bool {
        self.scripts.iter().any(|script| !script.plots.is_empty())
    }

    pub fn has_scripts(&self) -> bool {
        !self.configs.is_empty()
    }
}

pub fn external_indicator_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Ok(executable) = std::env::current_exe()
        && let Some(parent) = executable.parent()
    {
        // The singular name is the documented release layout. Keep the
        // plural alias for existing installations and developer builds.
        directories.push(parent.join("indicator"));
        directories.push(parent.join("indicators"));
    }
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        let root = PathBuf::from(data_home).join("flowsurface");
        directories.push(root.join("indicator"));
        directories.push(root.join("indicators"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let root = PathBuf::from(home).join(".local/share/flowsurface");
        directories.push(root.join("indicator"));
        directories.push(root.join("indicators"));
    }
    directories
}

fn load_external_configs() -> Vec<PineScriptConfig> {
    const MAX_FILES: usize = 64;
    let mut paths = Vec::new();
    for directory in external_indicator_directories() {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        paths.extend(
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.is_file()
                        && matches!(
                            path.extension().and_then(|extension| extension.to_str()),
                            Some("pine" | "pine5" | "pine6")
                        )
                }),
        );
    }
    paths.sort();
    paths.dedup();
    // Executable-adjacent files are preferred when the same filename exists
    // in a fallback directory.
    paths.sort_by_key(|path| {
        let executable_parent = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf));
        (
            !executable_parent.is_some_and(|parent| path.starts_with(parent)),
            path.clone(),
        )
    });
    paths.truncate(MAX_FILES);

    paths
        .into_iter()
        .filter_map(|path| load_external_config(&path))
        .collect()
}

pub fn external_indicator_summaries() -> Vec<ExternalPineIndicator> {
    load_external_configs()
        .into_iter()
        .map(|config| ExternalPineIndicator {
            name: config.name,
            path: PathBuf::from(config.source_path),
            version: detect_version(&config.source).unwrap_or(6),
            enabled: config.enabled,
            error: parse_source(&config.source, detect_version(&config.source).unwrap_or(6)).err(),
        })
        .collect()
}

fn load_external_config(path: &Path) -> Option<PineScriptConfig> {
    let source = fs::read_to_string(path).ok()?;
    if source.len() > MAX_SOURCE_BYTES {
        log::warn!(
            "PINE SourceSkipped | path={} reason=source_too_large",
            path.display()
        );
        return None;
    }
    let mut config = PineScriptConfig {
        name: path.file_stem()?.to_string_lossy().into_owned(),
        source_path: path.to_string_lossy().into_owned(),
        source,
        ..PineScriptConfig::default()
    };
    for line in config.source.lines().take(32) {
        let line = line.trim();
        if let Some(name) = line.strip_prefix("//@name=") {
            if !name.trim().is_empty() {
                config.name = name.trim().to_string();
            }
        } else if let Some(enabled) = line.strip_prefix("//@enabled=") {
            config.enabled = !matches!(enabled.trim(), "false" | "0" | "off");
        } else if let Some(color) = line.strip_prefix("//@color=") {
            if let Some(color) = parse_hex_color(color.trim()) {
                config.color = color;
            }
        } else if let Some(width) = line.strip_prefix("//@line_width=")
            && let Ok(width) = width.trim().parse::<f32>()
        {
            config.line_width = width.clamp(0.5, 8.0);
        }
    }
    Some(config)
}

#[derive(Debug, Clone)]
enum Statement {
    Assign(String, Expr),
    Plot {
        expr: Expr,
        title: Option<String>,
        color: Option<DrawingColor>,
        line_width: Option<f32>,
    },
    HLine {
        expr: Expr,
        title: Option<String>,
        color: Option<DrawingColor>,
        line_width: Option<f32>,
    },
    Shape {
        expr: Expr,
        title: Option<String>,
        color: Option<DrawingColor>,
    },
}

#[derive(Debug, Clone)]
enum Expr {
    Number(f64),
    String(String),
    Ident(String),
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
    Ternary {
        condition: Box<Expr>,
        yes: Box<Expr>,
        no: Box<Expr>,
    },
    Index {
        expr: Box<Expr>,
        offset: Box<Expr>,
    },
    Call {
        name: String,
        args: Vec<CallArg>,
    },
}

#[derive(Debug, Clone)]
struct CallArg {
    name: Option<String>,
    expr: Expr,
}

#[derive(Debug, Clone, Copy)]
enum UnaryOp {
    Plus,
    Minus,
    Not,
}

#[derive(Debug, Clone, Copy)]
enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Greater,
    GreaterEqual,
    Less,
    LessEqual,
    Equal,
    NotEqual,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Ident(String),
    String(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
    Question,
    Colon,
    Comma,
    Dot,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Equal,
    Declare,
    EqualEqual,
    NotEqual,
    Greater,
    GreaterEqual,
    Less,
    LessEqual,
    And,
    Or,
}

fn evaluate_script(config: &PineScriptConfig, bars: &[PineBar]) -> PineScriptResult {
    let version = detect_version(&config.source).unwrap_or(6);
    if config.source.len() > MAX_SOURCE_BYTES {
        return PineScriptResult::error(config, version, "Pine source exceeds 512 KiB");
    }
    if bars.len() > MAX_BARS {
        return PineScriptResult::error(
            config,
            version,
            "Chart history exceeds the Pine bar limit",
        );
    }

    let mut plots: Vec<(String, Expr, DrawingColor, f32)> = Vec::new();

    // Keeping statement order is important because Pine assignments can
    // depend on earlier assignments.
    let program = match parse_source(&config.source, version) {
        Ok(program) => program,
        Err(error) => return PineScriptResult::error(config, version, error),
    };
    for statement in &program {
        match statement {
            Statement::Plot {
                expr,
                title,
                color,
                line_width,
            }
            | Statement::HLine {
                expr,
                title,
                color,
                line_width,
            } => plots.push((
                title
                    .clone()
                    .unwrap_or_else(|| format!("Plot {}", plots.len() + 1)),
                expr.clone(),
                color.unwrap_or(config.color),
                line_width.unwrap_or(config.line_width.clamp(0.5, 8.0)),
            )),
            Statement::Shape { expr, title, color } => plots.push((
                title
                    .clone()
                    .unwrap_or_else(|| format!("Shape {}", plots.len() + 1)),
                expr.clone(),
                color.unwrap_or(config.color),
                config.line_width.clamp(0.5, 8.0),
            )),
            Statement::Assign(_, _) => {}
        }
    }

    let mut output = plots
        .into_iter()
        .map(|(title, _, color, line_width)| PinePlot {
            title,
            values: Vec::with_capacity(bars.len()),
            color,
            line_width,
        })
        .collect::<Vec<_>>();
    let mut plot_exprs = Vec::new();
    for statement in &program {
        match statement {
            Statement::Plot { expr, .. }
            | Statement::HLine { expr, .. }
            | Statement::Shape { expr, .. } => plot_exprs.push(expr.clone()),
            Statement::Assign(_, _) => {}
        }
    }

    // Assignment buffers are deliberately bounded by the chart bar count.
    let mut assignments: HashMap<String, Vec<Option<f64>>> = HashMap::new();
    assignments.clear();
    for statement in &program {
        if let Statement::Assign(name, _) = statement {
            assignments.insert(name.clone(), Vec::with_capacity(bars.len()));
        }
    }
    for index in 0..bars.len() {
        for statement in &program {
            if let Statement::Assign(name, expr) = statement {
                let value = eval_expr(expr, index, bars, &assignments);
                assignments
                    .get_mut(name)
                    .expect("assignment buffer")
                    .push(value);
            }
        }
        for (plot, expr) in output.iter_mut().zip(&plot_exprs) {
            plot.values.push(eval_expr(expr, index, bars, &assignments));
        }
    }

    PineScriptResult {
        name: config.name.clone(),
        source_path: config.source_path.clone(),
        version,
        plots: output,
        error: None,
    }
}

fn detect_version(source: &str) -> Option<u8> {
    source.lines().take(32).find_map(|line| {
        let normalized = line.trim().replace(' ', "");
        normalized
            .strip_prefix("//@version=")
            .and_then(|value| value.parse::<u8>().ok())
    })
}

fn parse_source(source: &str, version: u8) -> Result<Vec<Statement>, String> {
    if !matches!(version, 5 | 6) {
        return Err(format!(
            "Only Pine Script v5 and v6 are supported (found v{version})"
        ));
    }
    let lines = source.lines().collect::<Vec<_>>();
    if lines.len() > MAX_SOURCE_LINES {
        return Err("Pine source has too many lines".to_string());
    }
    let mut statements = Vec::new();
    for raw_line in lines {
        let line = strip_comment(raw_line).trim().to_string();
        if line.is_empty() || line.starts_with("//@") || line.starts_with("//") {
            continue;
        }
        let normalized = line.trim_start_matches("var ").trim_start_matches("varip ");
        if normalized.starts_with("indicator(")
            || normalized.starts_with("strategy(")
            || normalized.starts_with("study(")
            || normalized.starts_with("library(")
            || normalized.starts_with("import ")
        {
            continue;
        }
        if normalized.starts_with("if ")
            || normalized.starts_with("for ")
            || normalized.starts_with("while ")
            || normalized.starts_with("switch ")
            || normalized.starts_with("method ")
            || normalized.starts_with("type ")
            || normalized.contains("=>")
        {
            return Err(format!(
                "Unsupported Pine control-flow or declaration: {}",
                normalized.chars().take(96).collect::<String>()
            ));
        }

        let tokens = tokenize(normalized)?;
        if tokens.is_empty() {
            continue;
        }
        if let Some(statement) = parse_plot_statement(&tokens, normalized)? {
            statements.push(statement);
            continue;
        }
        if let Some((name, expression_tokens)) = assignment_tokens(&tokens) {
            let expression = Parser::new(expression_tokens).parse_expression()?;
            statements.push(Statement::Assign(name, expression));
            continue;
        }
        return Err(format!("Unsupported Pine statement: {}", normalized));
    }
    if statements.len() > MAX_STATEMENTS {
        return Err("Pine source has too many statements".to_string());
    }
    if !statements.iter().any(|statement| {
        matches!(
            statement,
            Statement::Plot { .. } | Statement::HLine { .. } | Statement::Shape { .. }
        )
    }) {
        return Err("Pine script does not contain a plot, hline, or plotshape output".to_string());
    }
    Ok(statements)
}

fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut quote = b'"';
    let bytes = line.as_bytes();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if (bytes[index] == b'"' || bytes[index] == b'\'')
            && (index == 0 || bytes[index - 1] != b'\\')
        {
            if !in_string {
                quote = bytes[index];
            }
            if bytes[index] != quote && in_string {
                index += 1;
                continue;
            }
            in_string = !in_string;
        }
        if !in_string && bytes[index] == b'/' && bytes[index + 1] == b'/' {
            return &line[..index];
        }
        index += 1;
    }
    line
}

fn assignment_tokens(tokens: &[Token]) -> Option<(String, Vec<Token>)> {
    let mut offset = 0;
    if matches!(tokens.first(), Some(Token::Ident(name)) if name == "var" || name == "varip") {
        offset += 1;
        if matches!(tokens.get(offset), Some(Token::Ident(name)) if name == "float" || name == "int" || name == "bool")
        {
            offset += 1;
        }
    }
    let name = match tokens.get(offset) {
        Some(Token::Ident(name)) => name.clone(),
        _ => return None,
    };
    if !matches!(tokens.get(offset + 1), Some(Token::Equal | Token::Declare)) {
        return None;
    }
    Some((name, tokens[offset + 2..].to_vec()))
}

fn parse_plot_statement(tokens: &[Token], source: &str) -> Result<Option<Statement>, String> {
    let (name, shape) = match tokens.first() {
        Some(Token::Ident(name)) if name == "plot" => ("plot", false),
        Some(Token::Ident(name)) if name == "hline" => ("hline", false),
        Some(Token::Ident(name)) if name == "plotshape" || name == "plotchar" => {
            (name.as_str(), true)
        }
        _ => return Ok(None),
    };
    let args = Parser::new(tokens.to_vec()).parse_call_only(name)?;
    let expression = args
        .first()
        .ok_or_else(|| format!("{name} requires a series expression in `{source}`"))?
        .expr
        .clone();
    let title = named_string(&args, "title");
    let color = named_color(&args, "color");
    let line_width = named_number(&args, "linewidth").map(|value| value as f32);
    Ok(Some(if name == "hline" {
        Statement::HLine {
            expr: expression,
            title,
            color,
            line_width,
        }
    } else if shape {
        Statement::Shape {
            expr: expression,
            title,
            color,
        }
    } else {
        Statement::Plot {
            expr: expression,
            title,
            color,
            line_width,
        }
    }))
}

fn named_string(args: &[CallArg], key: &str) -> Option<String> {
    args.iter().find_map(|arg| {
        (arg.name.as_deref() == Some(key)).then(|| match &arg.expr {
            Expr::String(value) => Some(value.clone()),
            _ => None,
        })?
    })
}

fn named_number(args: &[CallArg], key: &str) -> Option<f64> {
    args.iter().find_map(|arg| {
        (arg.name.as_deref() == Some(key)).then_some(match arg.expr {
            Expr::Number(value) => Some(value),
            _ => None,
        })?
    })
}

fn named_color(args: &[CallArg], key: &str) -> Option<DrawingColor> {
    args.iter()
        .find_map(|arg| (arg.name.as_deref() == Some(key)).then(|| color_from_expr(&arg.expr))?)
}

fn color_from_expr(expr: &Expr) -> Option<DrawingColor> {
    let Expr::Ident(name) = expr else {
        if let Expr::String(value) = expr {
            return parse_hex_color(value);
        }
        return None;
    };
    Some(match name.as_str() {
        "color.red" => DrawingColor::RED,
        "color.green" => DrawingColor::GREEN,
        "color.blue" => DrawingColor::BLUE,
        "color.orange" => DrawingColor::ORANGE,
        "color.yellow" => DrawingColor::rgb(0.95, 0.82, 0.16),
        "color.purple" => DrawingColor::rgb(0.65, 0.35, 0.9),
        "color.white" => DrawingColor::rgb(1.0, 1.0, 1.0),
        "color.black" => DrawingColor::rgb(0.0, 0.0, 0.0),
        _ => return None,
    })
}

fn parse_hex_color(value: &str) -> Option<DrawingColor> {
    let value = value.strip_prefix('#')?;
    if value.len() != 6 && value.len() != 8 {
        return None;
    }
    let parse = |part: &str| u8::from_str_radix(part, 16).ok();
    let r = parse(&value[0..2])? as f32 / 255.0;
    let g = parse(&value[2..4])? as f32 / 255.0;
    let b = parse(&value[4..6])? as f32 / 255.0;
    let a = if value.len() == 8 {
        parse(&value[6..8])? as f32 / 255.0
    } else {
        1.0
    };
    Some(DrawingColor { r, g, b, a })
}

fn tokenize(source: &str) -> Result<Vec<Token>, String> {
    let chars = source.chars().collect::<Vec<_>>();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if ch.is_whitespace() {
            index += 1;
            continue;
        }
        if ch == '"' || ch == '\'' {
            let quote = ch;
            let start = index + 1;
            index += 1;
            let mut value = String::new();
            while index < chars.len() && chars[index] != quote {
                if chars[index] == '\\' && index + 1 < chars.len() {
                    index += 1;
                    value.push(chars[index]);
                } else {
                    value.push(chars[index]);
                }
                index += 1;
            }
            if index >= chars.len() {
                return Err(format!("Unterminated string starting at character {start}"));
            }
            index += 1;
            tokens.push(Token::String(value));
            continue;
        }
        if ch.is_ascii_digit()
            || (ch == '.' && chars.get(index + 1).is_some_and(char::is_ascii_digit))
        {
            let start = index;
            index += 1;
            while index < chars.len()
                && (chars[index].is_ascii_digit()
                    || matches!(chars[index], '.' | 'e' | 'E' | '+' | '-'))
            {
                if matches!(chars[index], '+' | '-')
                    && !matches!(chars[index.saturating_sub(1)], 'e' | 'E')
                {
                    break;
                }
                index += 1;
            }
            let value = source
                .chars()
                .skip(start)
                .take(index - start)
                .collect::<String>()
                .parse::<f64>()
                .map_err(|_| "Invalid numeric literal".to_string())?;
            tokens.push(Token::Number(value));
            continue;
        }
        if ch.is_ascii_alphabetic() || ch == '_' {
            let start = index;
            index += 1;
            while index < chars.len()
                && (chars[index].is_ascii_alphanumeric() || chars[index] == '_')
            {
                index += 1;
            }
            let value = chars[start..index].iter().collect::<String>();
            tokens.push(match value.as_str() {
                "and" => Token::And,
                "or" => Token::Or,
                _ => Token::Ident(value),
            });
            continue;
        }
        let next = chars.get(index + 1).copied();
        let token = match (ch, next) {
            ('=', Some('=')) => {
                index += 2;
                Token::EqualEqual
            }
            ('!', Some('=')) => {
                index += 2;
                Token::NotEqual
            }
            ('>', Some('=')) => {
                index += 2;
                Token::GreaterEqual
            }
            ('<', Some('=')) => {
                index += 2;
                Token::LessEqual
            }
            (':', Some('=')) => {
                index += 2;
                Token::Declare
            }
            _ => {
                index += 1;
                match ch {
                    '+' => Token::Plus,
                    '-' => Token::Minus,
                    '*' => Token::Star,
                    '/' => Token::Slash,
                    '%' => Token::Percent,
                    '!' => Token::Bang,
                    '?' => Token::Question,
                    ':' => Token::Colon,
                    ',' => Token::Comma,
                    '.' => Token::Dot,
                    '(' => Token::LParen,
                    ')' => Token::RParen,
                    '[' => Token::LBracket,
                    ']' => Token::RBracket,
                    '=' => Token::Equal,
                    '>' => Token::Greater,
                    '<' => Token::Less,
                    _ => return Err(format!("Unsupported character `{ch}` in expression")),
                }
            }
        };
        tokens.push(token);
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            position: 0,
        }
    }

    fn parse_expression(mut self) -> Result<Expr, String> {
        let expression = self.parse_precedence(0)?;
        if self.position != self.tokens.len() {
            return Err(format!(
                "Unexpected token in expression: {:?}",
                self.tokens[self.position]
            ));
        }
        Ok(expression)
    }

    fn parse_call_only(mut self, expected: &str) -> Result<Vec<CallArg>, String> {
        let name = self.parse_name()?;
        if name != expected {
            return Err(format!("Expected {expected} call"));
        }
        if !matches!(self.next(), Some(Token::LParen)) {
            return Err(format!("{expected} requires parentheses"));
        }
        let args = self.parse_args()?;
        if self.position != self.tokens.len() {
            return Err("Unexpected tokens after call".to_string());
        }
        Ok(args)
    }

    fn parse_precedence(&mut self, minimum: u8) -> Result<Expr, String> {
        let mut left = self.parse_prefix()?;
        while let Some((operator, precedence)) = self.peek_binary_operator() {
            if precedence < minimum {
                break;
            }
            self.position += 1;
            let right = self.parse_precedence(precedence + 1)?;
            left = Expr::Binary {
                left: Box::new(left),
                op: operator,
                right: Box::new(right),
            };
        }
        if minimum == 0 && matches!(self.peek(), Some(Token::Question)) {
            self.position += 1;
            let yes = self.parse_precedence(0)?;
            if !matches!(self.next(), Some(Token::Colon)) {
                return Err("Ternary expression requires `:`".to_string());
            }
            let no = self.parse_precedence(0)?;
            left = Expr::Ternary {
                condition: Box::new(left),
                yes: Box::new(yes),
                no: Box::new(no),
            };
        }
        Ok(left)
    }

    fn parse_prefix(&mut self) -> Result<Expr, String> {
        let mut expression = match self.next() {
            Some(Token::Number(value)) => Expr::Number(value),
            Some(Token::String(value)) => Expr::String(value),
            Some(Token::Plus) => Expr::Unary {
                op: UnaryOp::Plus,
                expr: Box::new(self.parse_precedence(8)?),
            },
            Some(Token::Minus) => Expr::Unary {
                op: UnaryOp::Minus,
                expr: Box::new(self.parse_precedence(8)?),
            },
            Some(Token::Bang) => Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(self.parse_precedence(8)?),
            },
            Some(Token::Ident(_)) => {
                self.position -= 1;
                let name = self.parse_name()?;
                if matches!(self.peek(), Some(Token::LParen)) {
                    self.position += 1;
                    Expr::Call {
                        name,
                        args: self.parse_args()?,
                    }
                } else {
                    Expr::Ident(name)
                }
            }
            Some(Token::LParen) => {
                let value = self.parse_precedence(0)?;
                if !matches!(self.next(), Some(Token::RParen)) {
                    return Err("Missing closing parenthesis".to_string());
                }
                value
            }
            Some(token) => return Err(format!("Unexpected token: {token:?}")),
            None => return Err("Expected an expression".to_string()),
        };

        while matches!(self.peek(), Some(Token::LBracket)) {
            self.position += 1;
            let offset = self.parse_precedence(0)?;
            if !matches!(self.next(), Some(Token::RBracket)) {
                return Err("Missing closing history bracket".to_string());
            }
            expression = Expr::Index {
                expr: Box::new(expression),
                offset: Box::new(offset),
            };
        }
        Ok(expression)
    }

    fn parse_name(&mut self) -> Result<String, String> {
        let Some(Token::Ident(first)) = self.next() else {
            return Err("Expected identifier".to_string());
        };
        let mut name = first;
        while matches!(self.peek(), Some(Token::Dot)) {
            self.position += 1;
            let Some(Token::Ident(part)) = self.next() else {
                return Err("Expected identifier after `.`".to_string());
            };
            name.push('.');
            name.push_str(&part);
        }
        Ok(name)
    }

    fn parse_args(&mut self) -> Result<Vec<CallArg>, String> {
        let mut args = Vec::new();
        if matches!(self.peek(), Some(Token::RParen)) {
            self.position += 1;
            return Ok(args);
        }
        loop {
            let name = if let (Some(Token::Ident(name)), Some(Token::Equal)) =
                (self.peek(), self.tokens.get(self.position + 1))
            {
                let name = name.clone();
                self.position += 2;
                Some(name)
            } else {
                None
            };
            let expr = self.parse_precedence(0)?;
            args.push(CallArg { name, expr });
            match self.next() {
                Some(Token::Comma) => continue,
                Some(Token::RParen) => break,
                Some(token) => return Err(format!("Unexpected call token: {token:?}")),
                None => return Err("Missing closing call parenthesis".to_string()),
            }
        }
        Ok(args)
    }

    fn peek_binary_operator(&self) -> Option<(BinaryOp, u8)> {
        Some(match self.peek()? {
            Token::Or => (BinaryOp::Or, 1),
            Token::And => (BinaryOp::And, 2),
            Token::EqualEqual => (BinaryOp::Equal, 3),
            Token::NotEqual => (BinaryOp::NotEqual, 3),
            Token::Greater => (BinaryOp::Greater, 4),
            Token::GreaterEqual => (BinaryOp::GreaterEqual, 4),
            Token::Less => (BinaryOp::Less, 4),
            Token::LessEqual => (BinaryOp::LessEqual, 4),
            Token::Plus => (BinaryOp::Add, 5),
            Token::Minus => (BinaryOp::Sub, 5),
            Token::Star => (BinaryOp::Mul, 6),
            Token::Slash => (BinaryOp::Div, 6),
            Token::Percent => (BinaryOp::Rem, 6),
            _ => return None,
        })
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.position).cloned();
        self.position += usize::from(token.is_some());
        token
    }
}

fn eval_expr(
    expression: &Expr,
    index: usize,
    bars: &[PineBar],
    assignments: &HashMap<String, Vec<Option<f64>>>,
) -> Option<f64> {
    match expression {
        Expr::Number(value) => Some(*value),
        Expr::String(_) => None,
        Expr::Ident(name) => builtin_series(name, index, bars).or_else(|| {
            assignments
                .get(name)
                .and_then(|values| values.get(index).copied().flatten())
        }),
        Expr::Unary { op, expr } => {
            let value = eval_expr(expr, index, bars, assignments)?;
            Some(match op {
                UnaryOp::Plus => value,
                UnaryOp::Minus => -value,
                UnaryOp::Not => bool_value(!truthy(value)),
            })
        }
        Expr::Binary { left, op, right } => {
            let left = eval_expr(left, index, bars, assignments);
            let right = eval_expr(right, index, bars, assignments);
            match op {
                BinaryOp::And => Some(bool_value(
                    truthy(left.unwrap_or(0.0)) && truthy(right.unwrap_or(0.0)),
                )),
                BinaryOp::Or => Some(bool_value(
                    truthy(left.unwrap_or(0.0)) || truthy(right.unwrap_or(0.0)),
                )),
                _ => {
                    let (left, right) = (left?, right?);
                    Some(match op {
                        BinaryOp::Add => left + right,
                        BinaryOp::Sub => left - right,
                        BinaryOp::Mul => left * right,
                        BinaryOp::Div => {
                            if right == 0.0 {
                                f64::NAN
                            } else {
                                left / right
                            }
                        }
                        BinaryOp::Rem => {
                            if right == 0.0 {
                                f64::NAN
                            } else {
                                left % right
                            }
                        }
                        BinaryOp::Greater => bool_value(left > right),
                        BinaryOp::GreaterEqual => bool_value(left >= right),
                        BinaryOp::Less => bool_value(left < right),
                        BinaryOp::LessEqual => bool_value(left <= right),
                        BinaryOp::Equal => bool_value((left - right).abs() <= f64::EPSILON),
                        BinaryOp::NotEqual => bool_value((left - right).abs() > f64::EPSILON),
                        BinaryOp::And | BinaryOp::Or => unreachable!(),
                    })
                }
            }
        }
        Expr::Ternary { condition, yes, no } => {
            if truthy(eval_expr(condition, index, bars, assignments).unwrap_or(0.0)) {
                eval_expr(yes, index, bars, assignments)
            } else {
                eval_expr(no, index, bars, assignments)
            }
        }
        Expr::Index { expr, offset } => {
            let offset = eval_expr(offset, index, bars, assignments)?.max(0.0) as usize;
            let target = index.checked_sub(offset)?;
            match expr.as_ref() {
                Expr::Ident(name) => builtin_series(name, target, bars).or_else(|| {
                    assignments
                        .get(name)
                        .and_then(|values| values.get(target).copied().flatten())
                }),
                _ => eval_expr(expr, target, bars, assignments),
            }
        }
        Expr::Call { name, args } => eval_call(name, args, index, bars, assignments),
    }
}

fn eval_call(
    name: &str,
    args: &[CallArg],
    index: usize,
    bars: &[PineBar],
    assignments: &HashMap<String, Vec<Option<f64>>>,
) -> Option<f64> {
    let positional = args
        .iter()
        .filter(|arg| arg.name.is_none())
        .collect::<Vec<_>>();
    let value = |position: usize| {
        positional
            .get(position)
            .and_then(|arg| eval_expr(&arg.expr, index, bars, assignments))
    };
    let length = |position: usize, default: usize| {
        value(position).map_or(default, |value| (value.max(1.0) as usize).min(10_000))
    };
    match name {
        "ta.sma" => moving_average(&positional, index, bars, assignments, length(1, 14), false),
        "ta.ema" => moving_average(&positional, index, bars, assignments, length(1, 14), true),
        "ta.rma" => moving_average(&positional, index, bars, assignments, length(1, 14), true),
        "ta.wma" => weighted_average(&positional, index, bars, assignments, length(1, 14)),
        "ta.highest" => window_extreme(&positional, index, bars, assignments, length(1, 14), true),
        "ta.lowest" => window_extreme(&positional, index, bars, assignments, length(1, 14), false),
        "ta.stdev" => window_stdev(&positional, index, bars, assignments, length(1, 14)),
        "ta.rsi" => rsi(&positional, index, bars, assignments, length(1, 14)),
        "ta.change" | "change" => {
            let current = value(0)?;
            let previous = index.checked_sub(1).and_then(|previous| {
                eval_expr(&positional.first()?.expr, previous, bars, assignments)
            })?;
            Some(current - previous)
        }
        "ta.crossover" | "crossover" => crossover(&positional, index, bars, assignments, true),
        "ta.crossunder" | "crossunder" => crossover(&positional, index, bars, assignments, false),
        "math.abs" | "abs" => Some(value(0)?.abs()),
        "math.max" | "max" => Some(value(0)?.max(value(1)?)),
        "math.min" | "min" => Some(value(0)?.min(value(1)?)),
        "math.pow" => Some(value(0)?.powf(value(1)?)),
        "math.sqrt" => Some(value(0)?.max(0.0).sqrt()),
        "math.log" => Some(value(0)?.ln()),
        "math.exp" => Some(value(0)?.exp()),
        "nz" => value(0).or_else(|| value(1)).or(Some(0.0)),
        "na" => Some(bool_value(value(0).is_none())),
        name if name.starts_with("input.") => value(0),
        // `request.security` is deliberately deterministic and side-effect
        // free here: the host exposes the chart's own source, so a foreign
        // symbol cannot silently trigger network requests from a script.
        "request.security" | "request.security_lower_tf" => value(2),
        _ => None,
    }
}

fn builtin_series(name: &str, index: usize, bars: &[PineBar]) -> Option<f64> {
    let bar = bars.get(index)?;
    match name {
        "open" => Some(bar.open),
        "high" => Some(bar.high),
        "low" => Some(bar.low),
        "close" => Some(bar.close),
        "volume" => Some(bar.volume),
        "time" => Some(bar.time as f64),
        "bar_index" => Some(index as f64),
        "hl2" => Some((bar.high + bar.low) / 2.0),
        "hlc3" => Some((bar.high + bar.low + bar.close) / 3.0),
        "ohlc4" => Some((bar.open + bar.high + bar.low + bar.close) / 4.0),
        "true" => Some(1.0),
        "false" => Some(0.0),
        "na" => None,
        _ => None,
    }
}

fn moving_average(
    args: &[&CallArg],
    index: usize,
    bars: &[PineBar],
    assignments: &HashMap<String, Vec<Option<f64>>>,
    length: usize,
    exponential: bool,
) -> Option<f64> {
    let source = args.first()?.expr.clone();
    let start = index.saturating_sub(length.saturating_sub(1));
    let mut values = Vec::with_capacity(index - start + 1);
    for position in start..=index {
        values.push(eval_expr(&source, position, bars, assignments)?);
    }
    if values.len() < length {
        return None;
    }
    if !exponential {
        return Some(values.iter().sum::<f64>() / values.len() as f64);
    }
    let alpha = 2.0 / (length as f64 + 1.0);
    let mut ema = values[0];
    for value in values.iter().skip(1) {
        ema = alpha * *value + (1.0 - alpha) * ema;
    }
    Some(ema)
}

fn weighted_average(
    args: &[&CallArg],
    index: usize,
    bars: &[PineBar],
    assignments: &HashMap<String, Vec<Option<f64>>>,
    length: usize,
) -> Option<f64> {
    let source = args.first()?.expr.clone();
    let start = index.checked_sub(length.saturating_sub(1))?;
    let mut total = 0.0;
    let mut weight_total = 0.0;
    for (offset, position) in (start..=index).enumerate() {
        let value = eval_expr(&source, position, bars, assignments)?;
        let weight = (offset + 1) as f64;
        total += value * weight;
        weight_total += weight;
    }
    Some(total / weight_total)
}

fn window_extreme(
    args: &[&CallArg],
    index: usize,
    bars: &[PineBar],
    assignments: &HashMap<String, Vec<Option<f64>>>,
    length: usize,
    maximum: bool,
) -> Option<f64> {
    let source = args.first()?.expr.clone();
    let start = index.saturating_sub(length.saturating_sub(1));
    (start..=index)
        .filter_map(|position| eval_expr(&source, position, bars, assignments))
        .reduce(|a, b| if maximum { a.max(b) } else { a.min(b) })
}

fn window_stdev(
    args: &[&CallArg],
    index: usize,
    bars: &[PineBar],
    assignments: &HashMap<String, Vec<Option<f64>>>,
    length: usize,
) -> Option<f64> {
    let source = args.first()?.expr.clone();
    let start = index.saturating_sub(length.saturating_sub(1));
    let values = (start..=index)
        .map(|position| eval_expr(&source, position, bars, assignments))
        .collect::<Option<Vec<_>>>()?;
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    Some(
        (values
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f64>()
            / values.len() as f64)
            .sqrt(),
    )
}

fn rsi(
    args: &[&CallArg],
    index: usize,
    bars: &[PineBar],
    assignments: &HashMap<String, Vec<Option<f64>>>,
    length: usize,
) -> Option<f64> {
    let source = args.first()?.expr.clone();
    let start = index.checked_sub(length)?;
    let mut gains = 0.0;
    let mut losses = 0.0;
    for position in (start + 1)..=index {
        let current = eval_expr(&source, position, bars, assignments)?;
        let previous = eval_expr(&source, position - 1, bars, assignments)?;
        let change = current - previous;
        if change >= 0.0 {
            gains += change;
        } else {
            losses -= change;
        }
    }
    if losses == 0.0 {
        return Some(100.0);
    }
    let relative = gains / losses;
    Some(100.0 - 100.0 / (1.0 + relative))
}

fn crossover(
    args: &[&CallArg],
    index: usize,
    bars: &[PineBar],
    assignments: &HashMap<String, Vec<Option<f64>>>,
    over: bool,
) -> Option<f64> {
    let left = args.first()?.expr.clone();
    let right = args.get(1)?.expr.clone();
    let current_left = eval_expr(&left, index, bars, assignments)?;
    let current_right = eval_expr(&right, index, bars, assignments)?;
    let previous = index.checked_sub(1)?;
    let previous_left = eval_expr(&left, previous, bars, assignments)?;
    let previous_right = eval_expr(&right, previous, bars, assignments)?;
    Some(bool_value(if over {
        current_left > current_right && previous_left <= previous_right
    } else {
        current_left < current_right && previous_left >= previous_right
    }))
}

fn truthy(value: f64) -> bool {
    value.is_finite() && value != 0.0
}

fn bool_value(value: bool) -> f64 {
    value as u8 as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bars() -> Vec<PineBar> {
        (0..32)
            .map(|index| PineBar {
                time: index * 60_000,
                open: index as f64,
                high: index as f64 + 2.0,
                low: index as f64 - 1.0,
                close: index as f64 + 1.0,
                volume: 10.0 + index as f64,
            })
            .collect()
    }

    #[test]
    fn detects_both_supported_pine_versions() {
        assert_eq!(detect_version("//@version=5\nindicator(\"x\")"), Some(5));
        assert_eq!(detect_version("//@version = 6\nindicator(\"x\")"), Some(6));
    }

    #[test]
    fn evaluates_close_plot_and_sma() {
        let config = PineScriptConfig {
            source: r#"//@version=6
indicator("Test")
smooth = ta.sma(close, 3)
plot(smooth, title="SMA", color=color.orange)
"#
            .to_string(),
            ..PineScriptConfig::default()
        };
        let result = evaluate_script(&config, &bars());
        assert_eq!(result.error, None);
        assert_eq!(result.version, 6);
        assert_eq!(result.plots.len(), 1);
        assert_eq!(result.plots[0].title, "SMA");
        assert!(result.plots[0].values[1].is_none());
        assert_eq!(result.plots[0].values[2], Some(2.0));
    }

    #[test]
    fn accepts_single_quoted_metadata_and_plot_titles() {
        let config = PineScriptConfig {
            source: "//@version=5\nindicator('Test')\nplot(close, title='Close')".to_string(),
            ..PineScriptConfig::default()
        };
        let result = evaluate_script(&config, &bars());
        assert_eq!(result.error, None);
        assert_eq!(result.plots[0].title, "Close");
    }

    #[test]
    fn rejects_unsupported_versions_and_unbounded_control_flow() {
        let config = PineScriptConfig {
            source: "//@version=4\nplot(close)".to_string(),
            ..PineScriptConfig::default()
        };
        assert!(evaluate_script(&config, &bars()).error.is_some());

        let config = PineScriptConfig {
            source: "//@version=6\nif close > open\n    plot(close)".to_string(),
            ..PineScriptConfig::default()
        };
        assert!(evaluate_script(&config, &bars()).error.is_some());
    }
}
