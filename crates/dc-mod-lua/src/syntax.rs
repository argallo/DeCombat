//! Lua 5.x lexer and parser used for **static analysis only** (`dc-modkit lint`).
//!
//! Mods are executed by real Lua 5.4 through `mlua` (see `sandbox.rs`); nothing in this module runs code.
//! It survived the removal of the old hand-written interpreter because `dc-modkit` walks this AST to
//! report unintended globals and path traversal with `file:line` diagnostics.
// DOC: docs/MODDING_GUIDE.md section 9

use crate::value::LuaError;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Ident(String),
    Number(f64),
    String(String),
    // Keywords
    And,
    Break,
    Do,
    Else,
    ElseIf,
    End,
    False,
    For,
    Function,
    If,
    In,
    Local,
    Nil,
    Not,
    Or,
    Repeat,
    Return,
    Then,
    True,
    Until,
    While,
    // Symbols & Operators
    Plus,
    Minus,
    Star,
    Slash,
    FloorDiv, // //
    Percent,
    Caret,
    Hash,
    EqEq,
    TildeEq,
    LtEq,
    GtEq,
    Lt,
    Gt,
    Assign,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Semi,
    Colon,
    Comma,
    Dot,
    DotDot,
    DotDotDot,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub line: usize,
}

pub fn tokenize(input: &str) -> Result<Vec<Token>, LuaError> {
    crate::sandbox::check_not_bytecode(input.as_bytes())?;
    let stripped = input.strip_prefix('\u{feff}').unwrap_or(input);
    let chars: Vec<char> = stripped.chars().collect();
    let mut pos = 0usize;
    let mut line = 1usize;
    let mut tokens = Vec::new();

    while pos < chars.len() {
        let ch = chars[pos];
        if ch == '\n' {
            line += 1;
            pos += 1;
            continue;
        }
        if ch == ' ' || ch == '\t' || ch == '\r' {
            pos += 1;
            continue;
        }

        // Comment `--`
        if ch == '-' && pos + 1 < chars.len() && chars[pos + 1] == '-' {
            pos += 2;
            if pos < chars.len() && chars[pos] == '[' {
                if let Some((eq_count, header_len)) = check_long_bracket_open(&chars[pos..]) {
                    pos += header_len;
                    skip_long_bracket(&chars, &mut pos, &mut line, eq_count)?;
                    continue;
                }
            }
            while pos < chars.len() && chars[pos] != '\n' {
                pos += 1;
            }
            continue;
        }

        // Long string `[[ ... ]]` or `[=[ ... ]=]`
        if ch == '[' {
            if let Some((eq_count, header_len)) = check_long_bracket_open(&chars[pos..]) {
                let start_line = line;
                pos += header_len;
                let s = read_long_bracket_string(&chars, &mut pos, &mut line, eq_count)?;
                tokens.push(Token {
                    kind: TokenKind::String(s),
                    line: start_line,
                });
                continue;
            }
        }

        // Quoted string
        if ch == '"' || ch == '\'' {
            let start_line = line;
            let s = read_quoted_string(&chars, &mut pos, &mut line, ch)?;
            tokens.push(Token {
                kind: TokenKind::String(s),
                line: start_line,
            });
            continue;
        }

        // Numbers
        if ch.is_ascii_digit()
            || (ch == '.' && pos + 1 < chars.len() && chars[pos + 1].is_ascii_digit())
        {
            let start = pos;
            if ch == '0'
                && pos + 1 < chars.len()
                && (chars[pos + 1] == 'x' || chars[pos + 1] == 'X')
            {
                pos += 2;
                while pos < chars.len() && chars[pos].is_ascii_hexdigit() {
                    pos += 1;
                }
            } else {
                while pos < chars.len() && chars[pos].is_ascii_digit() {
                    pos += 1;
                }
                if pos < chars.len() && chars[pos] == '.' {
                    // Make sure it's not `..` concatenation
                    if !(pos + 1 < chars.len() && chars[pos + 1] == '.') {
                        pos += 1;
                        while pos < chars.len() && chars[pos].is_ascii_digit() {
                            pos += 1;
                        }
                    }
                }
                if pos < chars.len() && (chars[pos] == 'e' || chars[pos] == 'E') {
                    pos += 1;
                    if pos < chars.len() && (chars[pos] == '+' || chars[pos] == '-') {
                        pos += 1;
                    }
                    while pos < chars.len() && chars[pos].is_ascii_digit() {
                        pos += 1;
                    }
                }
            }
            let num_str: String = chars[start..pos].iter().collect();
            let val = parse_lua_number_str(&num_str).ok_or_else(|| {
                LuaError::Syntax(format!("line {}: malformed number '{}'", line, num_str))
            })?;
            tokens.push(Token {
                kind: TokenKind::Number(val),
                line,
            });
            continue;
        }

        // Identifiers / Keywords
        if ch.is_ascii_alphabetic() || ch == '_' {
            let start = pos;
            pos += 1;
            while pos < chars.len() && (chars[pos].is_ascii_alphanumeric() || chars[pos] == '_') {
                pos += 1;
            }
            let ident: String = chars[start..pos].iter().collect();
            let kind = match ident.as_str() {
                "and" => TokenKind::And,
                "break" => TokenKind::Break,
                "do" => TokenKind::Do,
                "else" => TokenKind::Else,
                "elseif" => TokenKind::ElseIf,
                "end" => TokenKind::End,
                "false" => TokenKind::False,
                "for" => TokenKind::For,
                "function" => TokenKind::Function,
                "if" => TokenKind::If,
                "in" => TokenKind::In,
                "local" => TokenKind::Local,
                "nil" => TokenKind::Nil,
                "not" => TokenKind::Not,
                "or" => TokenKind::Or,
                "repeat" => TokenKind::Repeat,
                "return" => TokenKind::Return,
                "then" => TokenKind::Then,
                "true" => TokenKind::True,
                "until" => TokenKind::Until,
                "while" => TokenKind::While,
                _ => TokenKind::Ident(ident),
            };
            tokens.push(Token { kind, line });
            continue;
        }

        // Operators and punctuation
        let kind = match ch {
            '+' => {
                pos += 1;
                TokenKind::Plus
            }
            '-' => {
                pos += 1;
                TokenKind::Minus
            }
            '*' => {
                pos += 1;
                TokenKind::Star
            }
            '/' => {
                if pos + 1 < chars.len() && chars[pos + 1] == '/' {
                    pos += 2;
                    TokenKind::FloorDiv
                } else {
                    pos += 1;
                    TokenKind::Slash
                }
            }
            '%' => {
                pos += 1;
                TokenKind::Percent
            }
            '^' => {
                pos += 1;
                TokenKind::Caret
            }
            '#' => {
                pos += 1;
                TokenKind::Hash
            }
            '=' => {
                if pos + 1 < chars.len() && chars[pos + 1] == '=' {
                    pos += 2;
                    TokenKind::EqEq
                } else {
                    pos += 1;
                    TokenKind::Assign
                }
            }
            '~' => {
                if pos + 1 < chars.len() && chars[pos + 1] == '=' {
                    pos += 2;
                    TokenKind::TildeEq
                } else {
                    return Err(LuaError::Syntax(format!(
                        "line {}: unexpected character '~'",
                        line
                    )));
                }
            }
            '<' => {
                if pos + 1 < chars.len() && chars[pos + 1] == '=' {
                    pos += 2;
                    TokenKind::LtEq
                } else {
                    pos += 1;
                    TokenKind::Lt
                }
            }
            '>' => {
                if pos + 1 < chars.len() && chars[pos + 1] == '=' {
                    pos += 2;
                    TokenKind::GtEq
                } else {
                    pos += 1;
                    TokenKind::Gt
                }
            }
            '(' => {
                pos += 1;
                TokenKind::LParen
            }
            ')' => {
                pos += 1;
                TokenKind::RParen
            }
            '{' => {
                pos += 1;
                TokenKind::LBrace
            }
            '}' => {
                pos += 1;
                TokenKind::RBrace
            }
            '[' => {
                pos += 1;
                TokenKind::LBracket
            }
            ']' => {
                pos += 1;
                TokenKind::RBracket
            }
            ';' => {
                pos += 1;
                TokenKind::Semi
            }
            ':' => {
                pos += 1;
                TokenKind::Colon
            }
            ',' => {
                pos += 1;
                TokenKind::Comma
            }
            '.' => {
                if pos + 2 < chars.len() && chars[pos + 1] == '.' && chars[pos + 2] == '.' {
                    pos += 3;
                    TokenKind::DotDotDot
                } else if pos + 1 < chars.len() && chars[pos + 1] == '.' {
                    pos += 2;
                    TokenKind::DotDot
                } else {
                    pos += 1;
                    TokenKind::Dot
                }
            }
            other => {
                return Err(LuaError::Syntax(format!(
                    "line {}: unexpected character '{}'",
                    line, other
                )))
            }
        };
        tokens.push(Token { kind, line });
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        line,
    });
    Ok(tokens)
}

pub(crate) fn parse_lua_number_str(s: &str) -> Option<f64> {
    let trimmed = s.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).ok().map(|v| v as f64)
    } else if let Some(hex) = trimmed
        .strip_prefix("-0x")
        .or_else(|| trimmed.strip_prefix("-0X"))
    {
        u64::from_str_radix(hex, 16).ok().map(|v| -(v as f64))
    } else {
        trimmed.parse::<f64>().ok()
    }
}

fn check_long_bracket_open(slice: &[char]) -> Option<(usize, usize)> {
    if slice.first() != Some(&'[') {
        return None;
    }
    let mut i = 1;
    while i < slice.len() && slice[i] == '=' {
        i += 1;
    }
    if i < slice.len() && slice[i] == '[' {
        Some((i - 1, i + 1))
    } else {
        None
    }
}

fn check_long_bracket_close(slice: &[char], eq_count: usize) -> Option<usize> {
    if slice.first() != Some(&']') {
        return None;
    }
    if slice.len() < eq_count + 2 {
        return None;
    }
    for i in 0..eq_count {
        if slice[1 + i] != '=' {
            return None;
        }
    }
    if slice[1 + eq_count] == ']' {
        Some(eq_count + 2)
    } else {
        None
    }
}

fn skip_long_bracket(
    chars: &[char],
    pos: &mut usize,
    line: &mut usize,
    eq_count: usize,
) -> Result<(), LuaError> {
    let start_line = *line;
    while *pos < chars.len() {
        if let Some(close_len) = check_long_bracket_close(&chars[*pos..], eq_count) {
            *pos += close_len;
            return Ok(());
        }
        if chars[*pos] == '\n' {
            *line += 1;
        }
        *pos += 1;
    }
    Err(LuaError::Syntax(format!(
        "line {}: unfinished long comment",
        start_line
    )))
}

fn read_long_bracket_string(
    chars: &[char],
    pos: &mut usize,
    line: &mut usize,
    eq_count: usize,
) -> Result<String, LuaError> {
    let start_line = *line;
    // Lua ignores an immediate newline after opening `[[`
    if *pos < chars.len() && chars[*pos] == '\r' {
        *pos += 1;
    }
    if *pos < chars.len() && chars[*pos] == '\n' {
        *line += 1;
        *pos += 1;
    }
    let mut out = String::new();
    while *pos < chars.len() {
        if let Some(close_len) = check_long_bracket_close(&chars[*pos..], eq_count) {
            *pos += close_len;
            return Ok(out);
        }
        let ch = chars[*pos];
        if ch == '\n' {
            *line += 1;
        }
        if ch != '\r' {
            out.push(ch);
        }
        *pos += 1;
    }
    Err(LuaError::Syntax(format!(
        "line {}: unfinished long string",
        start_line
    )))
}

fn read_quoted_string(
    chars: &[char],
    pos: &mut usize,
    line: &mut usize,
    quote: char,
) -> Result<String, LuaError> {
    let start_line = *line;
    *pos += 1; // skip opening quote
    let mut out = String::new();
    while *pos < chars.len() {
        let ch = chars[*pos];
        *pos += 1;
        if ch == quote {
            return Ok(out);
        }
        if ch == '\n' {
            return Err(LuaError::Syntax(format!(
                "line {}: unfinished string",
                start_line
            )));
        }
        if ch == '\\' {
            if *pos >= chars.len() {
                return Err(LuaError::Syntax(format!(
                    "line {}: unfinished escape sequence",
                    *line
                )));
            }
            let esc = chars[*pos];
            *pos += 1;
            match esc {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'a' => out.push('\x07'),
                'b' => out.push('\x08'),
                'f' => out.push('\x0c'),
                'v' => out.push('\x0b'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                '\'' => out.push('\''),
                '\n' => {
                    *line += 1;
                    out.push('\n');
                }
                '0'..='9' => {
                    let mut code: u32 = (esc as u32) - ('0' as u32);
                    for _ in 0..2 {
                        if *pos < chars.len() && chars[*pos].is_ascii_digit() {
                            code = code * 10 + ((chars[*pos] as u32) - ('0' as u32));
                            *pos += 1;
                        } else {
                            break;
                        }
                    }
                    if let Some(c) = char::from_u32(code) {
                        out.push(c);
                    }
                }
                'x' => {
                    if *pos + 1 >= chars.len() {
                        return Err(LuaError::Syntax(format!(
                            "line {}: incomplete \\xXX escape",
                            *line
                        )));
                    }
                    let hex: String = chars[*pos..*pos + 2].iter().collect();
                    *pos += 2;
                    let code = u32::from_str_radix(&hex, 16).map_err(|_| {
                        LuaError::Syntax(format!("line {}: invalid hex escape \\x{}", *line, hex))
                    })?;
                    if let Some(c) = char::from_u32(code) {
                        out.push(c);
                    }
                }
                other => out.push(other),
            }
        } else {
            out.push(ch);
        }
    }
    Err(LuaError::Syntax(format!(
        "line {}: unfinished string",
        start_line
    )))
}

// ============================================================================
// AST & Recursive-Descent Parser
// ============================================================================

#[derive(Debug, Clone)]
pub struct Stmt {
    pub kind: StmtKind,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    Local {
        names: Vec<String>,
        values: Vec<Expr>,
    },
    Assign {
        targets: Vec<AssignTarget>,
        values: Vec<Expr>,
    },
    FuncDecl {
        name_path: Vec<String>,
        method: Option<String>,
        params: Vec<String>,
        has_varargs: bool,
        body: Vec<Stmt>,
        is_local: bool,
    },
    If {
        branches: Vec<(Expr, Vec<Stmt>)>,
        else_branch: Option<Vec<Stmt>>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
    },
    Repeat {
        body: Vec<Stmt>,
        cond: Expr,
    },
    ForNum {
        var: String,
        start: Expr,
        limit: Expr,
        step: Option<Expr>,
        body: Vec<Stmt>,
    },
    ForIn {
        vars: Vec<String>,
        iter_exprs: Vec<Expr>,
        body: Vec<Stmt>,
    },
    Do {
        body: Vec<Stmt>,
    },
    Return(Vec<Expr>),
    Break,
    ExprStmt(Expr),
}

#[derive(Debug, Clone)]
pub enum AssignTarget {
    Ident(String),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Nil,
    Bool(bool),
    Number(f64),
    String(String),
    Vararg,
    Ident(String),
    Paren(Box<Expr>),
    TableCons(Vec<TableField>),
    Function {
        params: Vec<String>,
        has_varargs: bool,
        body: Vec<Stmt>,
    },
    Unary {
        op: UnOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Field {
        expr: Box<Expr>,
        field: String,
    },
    Index {
        expr: Box<Expr>,
        index: Box<Expr>,
    },
    Call {
        func: Box<Expr>,
        args: Vec<Expr>,
    },
    MethodCall {
        receiver: Box<Expr>,
        method: String,
        args: Vec<Expr>,
    },
}

#[derive(Debug, Clone)]
pub enum TableField {
    Index(Expr, Expr),
    Named(String, Expr),
    Positional(Expr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    Len,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

pub fn parse_chunk(input: &str) -> Result<Vec<Stmt>, LuaError> {
    let tokens = tokenize(input)?;
    let mut parser = Parser { tokens, pos: 0 };
    let stmts = parser.parse_block(&[TokenKind::Eof])?;
    parser.expect(&TokenKind::Eof)?;
    Ok(stmts)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        self.tokens
            .get(self.pos)
            .unwrap_or_else(|| self.tokens.last().unwrap())
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    fn line(&self) -> usize {
        self.peek().line
    }

    fn advance(&mut self) -> Token {
        let tok = self.peek().clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn check(&self, kind: &TokenKind) -> bool {
        self.peek_kind() == kind
    }

    fn match_tok(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: &TokenKind) -> Result<Token, LuaError> {
        if self.check(kind) {
            Ok(self.advance())
        } else {
            Err(LuaError::Syntax(format!(
                "line {}: expected {:?}, found {:?}",
                self.line(),
                kind,
                self.peek_kind()
            )))
        }
    }

    fn expect_ident(&mut self) -> Result<String, LuaError> {
        match self.peek_kind().clone() {
            TokenKind::Ident(s) => {
                self.advance();
                Ok(s)
            }
            other => Err(LuaError::Syntax(format!(
                "line {}: expected identifier, found {:?}",
                self.line(),
                other
            ))),
        }
    }

    fn parse_block(&mut self, terminators: &[TokenKind]) -> Result<Vec<Stmt>, LuaError> {
        let mut stmts = Vec::new();
        while !terminators.contains(self.peek_kind()) && !self.check(&TokenKind::Eof) {
            if self.match_tok(&TokenKind::Semi) {
                continue;
            }
            let stmt = self.parse_stmt()?;
            let is_return_or_break = matches!(stmt.kind, StmtKind::Return(_) | StmtKind::Break);
            stmts.push(stmt);
            self.match_tok(&TokenKind::Semi);
            if is_return_or_break {
                break;
            }
        }
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, LuaError> {
        let line = self.line();
        match self.peek_kind().clone() {
            TokenKind::Local => {
                self.advance();
                if self.match_tok(&TokenKind::Function) {
                    let name = self.expect_ident()?;
                    let (params, has_varargs, body) = self.parse_func_body()?;
                    Ok(Stmt {
                        kind: StmtKind::FuncDecl {
                            name_path: vec![name],
                            method: None,
                            params,
                            has_varargs,
                            body,
                            is_local: true,
                        },
                        line,
                    })
                } else {
                    let mut names = vec![self.expect_ident()?];
                    while self.match_tok(&TokenKind::Comma) {
                        names.push(self.expect_ident()?);
                    }
                    let values = if self.match_tok(&TokenKind::Assign) {
                        self.parse_expr_list()?
                    } else {
                        Vec::new()
                    };
                    Ok(Stmt {
                        kind: StmtKind::Local { names, values },
                        line,
                    })
                }
            }
            TokenKind::Function => {
                self.advance();
                let mut name_path = vec![self.expect_ident()?];
                while self.match_tok(&TokenKind::Dot) {
                    name_path.push(self.expect_ident()?);
                }
                let method = if self.match_tok(&TokenKind::Colon) {
                    Some(self.expect_ident()?)
                } else {
                    None
                };
                let (params, has_varargs, body) = self.parse_func_body()?;
                Ok(Stmt {
                    kind: StmtKind::FuncDecl {
                        name_path,
                        method,
                        params,
                        has_varargs,
                        body,
                        is_local: false,
                    },
                    line,
                })
            }
            TokenKind::If => {
                self.advance();
                let cond = self.parse_expr()?;
                self.expect(&TokenKind::Then)?;
                let then_body =
                    self.parse_block(&[TokenKind::ElseIf, TokenKind::Else, TokenKind::End])?;
                let mut branches = vec![(cond, then_body)];
                while self.match_tok(&TokenKind::ElseIf) {
                    let c = self.parse_expr()?;
                    self.expect(&TokenKind::Then)?;
                    let b =
                        self.parse_block(&[TokenKind::ElseIf, TokenKind::Else, TokenKind::End])?;
                    branches.push((c, b));
                }
                let else_branch = if self.match_tok(&TokenKind::Else) {
                    Some(self.parse_block(&[TokenKind::End])?)
                } else {
                    None
                };
                self.expect(&TokenKind::End)?;
                Ok(Stmt {
                    kind: StmtKind::If {
                        branches,
                        else_branch,
                    },
                    line,
                })
            }
            TokenKind::While => {
                self.advance();
                let cond = self.parse_expr()?;
                self.expect(&TokenKind::Do)?;
                let body = self.parse_block(&[TokenKind::End])?;
                self.expect(&TokenKind::End)?;
                Ok(Stmt {
                    kind: StmtKind::While { cond, body },
                    line,
                })
            }
            TokenKind::Repeat => {
                self.advance();
                let body = self.parse_block(&[TokenKind::Until])?;
                self.expect(&TokenKind::Until)?;
                let cond = self.parse_expr()?;
                Ok(Stmt {
                    kind: StmtKind::Repeat { body, cond },
                    line,
                })
            }
            TokenKind::For => {
                self.advance();
                let first_var = self.expect_ident()?;
                if self.match_tok(&TokenKind::Assign) {
                    let start = self.parse_expr()?;
                    self.expect(&TokenKind::Comma)?;
                    let limit = self.parse_expr()?;
                    let step = if self.match_tok(&TokenKind::Comma) {
                        Some(self.parse_expr()?)
                    } else {
                        None
                    };
                    self.expect(&TokenKind::Do)?;
                    let body = self.parse_block(&[TokenKind::End])?;
                    self.expect(&TokenKind::End)?;
                    Ok(Stmt {
                        kind: StmtKind::ForNum {
                            var: first_var,
                            start,
                            limit,
                            step,
                            body,
                        },
                        line,
                    })
                } else {
                    let mut vars = vec![first_var];
                    while self.match_tok(&TokenKind::Comma) {
                        vars.push(self.expect_ident()?);
                    }
                    self.expect(&TokenKind::In)?;
                    let iter_exprs = self.parse_expr_list()?;
                    self.expect(&TokenKind::Do)?;
                    let body = self.parse_block(&[TokenKind::End])?;
                    self.expect(&TokenKind::End)?;
                    Ok(Stmt {
                        kind: StmtKind::ForIn {
                            vars,
                            iter_exprs,
                            body,
                        },
                        line,
                    })
                }
            }
            TokenKind::Do => {
                self.advance();
                let body = self.parse_block(&[TokenKind::End])?;
                self.expect(&TokenKind::End)?;
                Ok(Stmt {
                    kind: StmtKind::Do { body },
                    line,
                })
            }
            TokenKind::Return => {
                self.advance();
                let values = if matches!(
                    self.peek_kind(),
                    TokenKind::End
                        | TokenKind::Else
                        | TokenKind::ElseIf
                        | TokenKind::Until
                        | TokenKind::Semi
                        | TokenKind::Eof
                ) {
                    Vec::new()
                } else {
                    self.parse_expr_list()?
                };
                Ok(Stmt {
                    kind: StmtKind::Return(values),
                    line,
                })
            }
            TokenKind::Break => {
                self.advance();
                Ok(Stmt {
                    kind: StmtKind::Break,
                    line,
                })
            }
            _ => {
                let expr = self.parse_suffixed_expr()?;
                if self.check(&TokenKind::Comma) || self.check(&TokenKind::Assign) {
                    let mut targets = vec![expr_to_assign_target(expr)?];
                    while self.match_tok(&TokenKind::Comma) {
                        let next_expr = self.parse_suffixed_expr()?;
                        targets.push(expr_to_assign_target(next_expr)?);
                    }
                    self.expect(&TokenKind::Assign)?;
                    let values = self.parse_expr_list()?;
                    Ok(Stmt {
                        kind: StmtKind::Assign { targets, values },
                        line,
                    })
                } else {
                    match expr.kind {
                        ExprKind::Call { .. } | ExprKind::MethodCall { .. } => Ok(Stmt {
                            kind: StmtKind::ExprStmt(expr),
                            line,
                        }),
                        _ => Err(LuaError::Syntax(format!(
                            "line {}: expected assignment or function call statement",
                            line
                        ))),
                    }
                }
            }
        }
    }

    fn parse_func_body(&mut self) -> Result<(Vec<String>, bool, Vec<Stmt>), LuaError> {
        self.expect(&TokenKind::LParen)?;
        let mut params = Vec::new();
        let mut has_varargs = false;
        if !self.check(&TokenKind::RParen) {
            loop {
                if self.match_tok(&TokenKind::DotDotDot) {
                    has_varargs = true;
                    break;
                }
                params.push(self.expect_ident()?);
                if !self.match_tok(&TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(&TokenKind::RParen)?;
        let body = self.parse_block(&[TokenKind::End])?;
        self.expect(&TokenKind::End)?;
        Ok((params, has_varargs, body))
    }

    fn parse_expr_list(&mut self) -> Result<Vec<Expr>, LuaError> {
        let mut list = vec![self.parse_expr()?];
        while self.match_tok(&TokenKind::Comma) {
            list.push(self.parse_expr()?);
        }
        Ok(list)
    }

    fn parse_expr(&mut self) -> Result<Expr, LuaError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, LuaError> {
        let mut lhs = self.parse_and()?;
        while self.match_tok(&TokenKind::Or) {
            let line = lhs.line;
            let rhs = self.parse_and()?;
            lhs = Expr {
                kind: ExprKind::Binary {
                    op: BinOp::Or,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                line,
            };
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, LuaError> {
        let mut lhs = self.parse_cmp()?;
        while self.match_tok(&TokenKind::And) {
            let line = lhs.line;
            let rhs = self.parse_cmp()?;
            lhs = Expr {
                kind: ExprKind::Binary {
                    op: BinOp::And,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                line,
            };
        }
        Ok(lhs)
    }

    fn parse_cmp(&mut self) -> Result<Expr, LuaError> {
        let mut lhs = self.parse_concat()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::EqEq => BinOp::Eq,
                TokenKind::TildeEq => BinOp::Ne,
                TokenKind::Lt => BinOp::Lt,
                TokenKind::LtEq => BinOp::Le,
                TokenKind::Gt => BinOp::Gt,
                TokenKind::GtEq => BinOp::Ge,
                _ => break,
            };
            self.advance();
            let line = lhs.line;
            let rhs = self.parse_concat()?;
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                line,
            };
        }
        Ok(lhs)
    }

    fn parse_concat(&mut self) -> Result<Expr, LuaError> {
        let lhs = self.parse_add()?;
        if self.match_tok(&TokenKind::DotDot) {
            let line = lhs.line;
            let rhs = self.parse_concat()?; // right-associative
            Ok(Expr {
                kind: ExprKind::Binary {
                    op: BinOp::Concat,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                line,
            })
        } else {
            Ok(lhs)
        }
    }

    fn parse_add(&mut self) -> Result<Expr, LuaError> {
        let mut lhs = self.parse_mul()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let line = lhs.line;
            let rhs = self.parse_mul()?;
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                line,
            };
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> Result<Expr, LuaError> {
        let mut lhs = self.parse_unary()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                TokenKind::FloorDiv => BinOp::FloorDiv,
                TokenKind::Percent => BinOp::Mod,
                _ => break,
            };
            self.advance();
            let line = lhs.line;
            let rhs = self.parse_unary()?;
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                line,
            };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, LuaError> {
        let line = self.line();
        match self.peek_kind() {
            TokenKind::Minus => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr {
                    kind: ExprKind::Unary {
                        op: UnOp::Neg,
                        expr: Box::new(expr),
                    },
                    line,
                })
            }
            TokenKind::Not => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr {
                    kind: ExprKind::Unary {
                        op: UnOp::Not,
                        expr: Box::new(expr),
                    },
                    line,
                })
            }
            TokenKind::Hash => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr {
                    kind: ExprKind::Unary {
                        op: UnOp::Len,
                        expr: Box::new(expr),
                    },
                    line,
                })
            }
            _ => self.parse_pow(),
        }
    }

    fn parse_pow(&mut self) -> Result<Expr, LuaError> {
        let lhs = self.parse_suffixed_expr()?;
        if self.match_tok(&TokenKind::Caret) {
            let line = lhs.line;
            let rhs = self.parse_unary()?; // right-associative
            Ok(Expr {
                kind: ExprKind::Binary {
                    op: BinOp::Pow,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                line,
            })
        } else {
            Ok(lhs)
        }
    }

    fn parse_suffixed_expr(&mut self) -> Result<Expr, LuaError> {
        let mut expr = self.parse_primary_expr()?;
        loop {
            let line = self.line();
            match self.peek_kind().clone() {
                TokenKind::Dot => {
                    self.advance();
                    let field = self.expect_ident()?;
                    expr = Expr {
                        kind: ExprKind::Field {
                            expr: Box::new(expr),
                            field,
                        },
                        line,
                    };
                }
                TokenKind::LBracket => {
                    self.advance();
                    let index = self.parse_expr()?;
                    self.expect(&TokenKind::RBracket)?;
                    expr = Expr {
                        kind: ExprKind::Index {
                            expr: Box::new(expr),
                            index: Box::new(index),
                        },
                        line,
                    };
                }
                TokenKind::Colon => {
                    self.advance();
                    let method = self.expect_ident()?;
                    let args = self.parse_call_args()?;
                    expr = Expr {
                        kind: ExprKind::MethodCall {
                            receiver: Box::new(expr),
                            method,
                            args,
                        },
                        line,
                    };
                }
                TokenKind::LParen | TokenKind::LBrace | TokenKind::String(_) => {
                    let args = self.parse_call_args()?;
                    expr = Expr {
                        kind: ExprKind::Call {
                            func: Box::new(expr),
                            args,
                        },
                        line,
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_call_args(&mut self) -> Result<Vec<Expr>, LuaError> {
        let line = self.line();
        match self.peek_kind().clone() {
            TokenKind::LParen => {
                self.advance();
                if self.match_tok(&TokenKind::RParen) {
                    Ok(Vec::new())
                } else {
                    let args = self.parse_expr_list()?;
                    self.expect(&TokenKind::RParen)?;
                    Ok(args)
                }
            }
            TokenKind::LBrace => {
                let tbl = self.parse_table_constructor()?;
                Ok(vec![tbl])
            }
            TokenKind::String(s) => {
                self.advance();
                Ok(vec![Expr {
                    kind: ExprKind::String(s),
                    line,
                }])
            }
            other => Err(LuaError::Syntax(format!(
                "line {}: expected function arguments, found {:?}",
                line, other
            ))),
        }
    }

    fn parse_primary_expr(&mut self) -> Result<Expr, LuaError> {
        let line = self.line();
        match self.peek_kind().clone() {
            TokenKind::Nil => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Nil,
                    line,
                })
            }
            TokenKind::True => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Bool(true),
                    line,
                })
            }
            TokenKind::False => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Bool(false),
                    line,
                })
            }
            TokenKind::Number(n) => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Number(n),
                    line,
                })
            }
            TokenKind::String(s) => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::String(s),
                    line,
                })
            }
            TokenKind::DotDotDot => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Vararg,
                    line,
                })
            }
            TokenKind::Ident(name) => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Ident(name),
                    line,
                })
            }
            TokenKind::LParen => {
                self.advance();
                let inner = self.parse_expr()?;
                self.expect(&TokenKind::RParen)?;
                Ok(Expr {
                    kind: ExprKind::Paren(Box::new(inner)),
                    line,
                })
            }
            TokenKind::LBrace => self.parse_table_constructor(),
            TokenKind::Function => {
                self.advance();
                let (params, has_varargs, body) = self.parse_func_body()?;
                Ok(Expr {
                    kind: ExprKind::Function {
                        params,
                        has_varargs,
                        body,
                    },
                    line,
                })
            }
            other => Err(LuaError::Syntax(format!(
                "line {}: unexpected token {:?} in expression",
                line, other
            ))),
        }
    }

    fn parse_table_constructor(&mut self) -> Result<Expr, LuaError> {
        let line = self.line();
        self.expect(&TokenKind::LBrace)?;
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            if self.match_tok(&TokenKind::LBracket) {
                let key_expr = self.parse_expr()?;
                self.expect(&TokenKind::RBracket)?;
                self.expect(&TokenKind::Assign)?;
                let val_expr = self.parse_expr()?;
                fields.push(TableField::Index(key_expr, val_expr));
            } else if let TokenKind::Ident(name) = self.peek_kind().clone() {
                if self
                    .tokens
                    .get(self.pos + 1)
                    .map(|t| t.kind == TokenKind::Assign)
                    .unwrap_or(false)
                {
                    self.advance(); // ident
                    self.advance(); // '='
                    let val_expr = self.parse_expr()?;
                    fields.push(TableField::Named(name, val_expr));
                } else {
                    let val_expr = self.parse_expr()?;
                    fields.push(TableField::Positional(val_expr));
                }
            } else {
                let val_expr = self.parse_expr()?;
                fields.push(TableField::Positional(val_expr));
            }

            if !self.match_tok(&TokenKind::Comma) && !self.match_tok(&TokenKind::Semi) {
                break;
            }
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Expr {
            kind: ExprKind::TableCons(fields),
            line,
        })
    }
}

fn expr_to_assign_target(expr: Expr) -> Result<AssignTarget, LuaError> {
    match expr.kind {
        ExprKind::Ident(name) => Ok(AssignTarget::Ident(name)),
        ExprKind::Field { expr, field } => Ok(AssignTarget::Field(expr, field)),
        ExprKind::Index { expr, index } => Ok(AssignTarget::Index(expr, index)),
        _ => Err(LuaError::Syntax(format!(
            "line {}: invalid assignment target",
            expr.line
        ))),
    }
}

// ============================================================================
// Sandboxed Lua VM & Evaluator
// ============================================================================
