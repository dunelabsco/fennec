use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

use super::traits::{Tool, ToolResult};

/// A simple calculator tool that evaluates mathematical expressions.
///
/// Supports: `+`, `-`, `*`, `/`, `%`, `^` (power), parentheses, and
/// negative numbers. Uses a recursive descent parser.
pub struct CalculatorTool;

impl CalculatorTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for CalculatorTool {
    fn name(&self) -> &str {
        "calculator"
    }

    fn description(&self) -> &str {
        "Evaluate a mathematical expression. Supports +, -, *, /, %, ^ (power), and parentheses."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "The mathematical expression to evaluate (e.g. '2 + 3 * 4', '(1 + 2) ^ 3')"
                }
            },
            "required": ["expression"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let expression = match args.get("expression").and_then(|v| v.as_str()) {
            Some(e) => e,
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("missing required parameter: expression".to_string()),
                });
            }
        };

        match evaluate(expression) {
            Ok(result) => {
                // Format nicely: show as integer if it's a whole number.
                let output = if result.fract() == 0.0 && result.abs() < 1e15 {
                    format!("{}", result as i64)
                } else {
                    format!("{result}")
                };
                Ok(ToolResult {
                    success: true,
                    output,
                    error: None,
                })
            }
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("evaluation error: {e}")),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Recursive descent expression parser
// ---------------------------------------------------------------------------

/// Tokenize and evaluate a mathematical expression string.
fn evaluate(expr: &str) -> std::result::Result<f64, String> {
    let tokens = tokenize(expr)?;
    let mut pos = 0;
    let result = parse_expression(&tokens, &mut pos)?;
    if pos < tokens.len() {
        return Err(format!("unexpected token: {:?}", tokens[pos]));
    }
    Ok(result)
}

#[derive(Debug, Clone)]
enum Token {
    Number(f64),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    LParen,
    RParen,
}

fn tokenize(expr: &str) -> std::result::Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = expr.chars().peekable();

    while let Some(&ch) = chars.peek() {
        match ch {
            ' ' | '\t' | '\n' => {
                chars.next();
            }
            '0'..='9' | '.' => {
                let mut num_str = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_digit() || c == '.' || c == '_' {
                        if c != '_' {
                            num_str.push(c);
                        }
                        chars.next();
                    } else {
                        break;
                    }
                }
                let num: f64 = num_str
                    .parse()
                    .map_err(|_| format!("invalid number: {num_str}"))?;
                tokens.push(Token::Number(num));
            }
            '+' => {
                tokens.push(Token::Plus);
                chars.next();
            }
            '-' => {
                tokens.push(Token::Minus);
                chars.next();
            }
            '*' => {
                // Handle ** as power.
                chars.next();
                if chars.peek() == Some(&'*') {
                    chars.next();
                    tokens.push(Token::Caret);
                } else {
                    tokens.push(Token::Star);
                }
            }
            '/' => {
                tokens.push(Token::Slash);
                chars.next();
            }
            '%' => {
                tokens.push(Token::Percent);
                chars.next();
            }
            '^' => {
                tokens.push(Token::Caret);
                chars.next();
            }
            '(' => {
                tokens.push(Token::LParen);
                chars.next();
            }
            ')' => {
                tokens.push(Token::RParen);
                chars.next();
            }
            other => {
                return Err(format!("unexpected character: '{other}'"));
            }
        }
    }

    Ok(tokens)
}

/// Parse an expression (lowest precedence: addition/subtraction).
fn parse_expression(tokens: &[Token], pos: &mut usize) -> std::result::Result<f64, String> {
    let mut left = parse_term(tokens, pos)?;

    while *pos < tokens.len() {
        match tokens[*pos] {
            Token::Plus => {
                *pos += 1;
                let right = parse_term(tokens, pos)?;
                left += right;
            }
            Token::Minus => {
                *pos += 1;
                let right = parse_term(tokens, pos)?;
                left -= right;
            }
            _ => break,
        }
    }

    Ok(left)
}

/// Parse a term (multiplication, division, modulo).
fn parse_term(tokens: &[Token], pos: &mut usize) -> std::result::Result<f64, String> {
    let mut left = parse_power(tokens, pos)?;

    while *pos < tokens.len() {
        match tokens[*pos] {
            Token::Star => {
                *pos += 1;
                let right = parse_power(tokens, pos)?;
                left *= right;
            }
            Token::Slash => {
                *pos += 1;
                let right = parse_power(tokens, pos)?;
                if right == 0.0 {
                    return Err("division by zero".to_string());
                }
                left /= right;
            }
            Token::Percent => {
                *pos += 1;
                let right = parse_power(tokens, pos)?;
                if right == 0.0 {
                    return Err("modulo by zero".to_string());
                }
                left %= right;
            }
            _ => break,
        }
    }

    Ok(left)
}

/// Parse a power expression (right-associative).
fn parse_power(tokens: &[Token], pos: &mut usize) -> std::result::Result<f64, String> {
    let base = parse_unary(tokens, pos)?;

    if *pos < tokens.len() {
        if let Token::Caret = tokens[*pos] {
            *pos += 1;
            let exp = parse_power(tokens, pos)?; // right-associative
            return Ok(base.powf(exp));
        }
    }

    Ok(base)
}

/// Parse unary + and -.
fn parse_unary(tokens: &[Token], pos: &mut usize) -> std::result::Result<f64, String> {
    if *pos < tokens.len() {
        match tokens[*pos] {
            Token::Minus => {
                *pos += 1;
                let val = parse_unary(tokens, pos)?;
                return Ok(-val);
            }
            Token::Plus => {
                *pos += 1;
                return parse_unary(tokens, pos);
            }
            _ => {}
        }
    }
    parse_primary(tokens, pos)
}

/// Parse a primary expression (number or parenthesized expression).
fn parse_primary(tokens: &[Token], pos: &mut usize) -> std::result::Result<f64, String> {
    if *pos >= tokens.len() {
        return Err("unexpected end of expression".to_string());
    }

    match tokens[*pos] {
        Token::Number(n) => {
            *pos += 1;
            Ok(n)
        }
        Token::LParen => {
            *pos += 1;
            let result = parse_expression(tokens, pos)?;
            if *pos >= tokens.len() {
                return Err("missing closing parenthesis".to_string());
            }
            match tokens[*pos] {
                Token::RParen => {
                    *pos += 1;
                    Ok(result)
                }
                _ => Err("expected closing parenthesis".to_string()),
            }
        }
        ref tok => Err(format!("unexpected token: {tok:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_arithmetic() {
        assert_eq!(evaluate("2 + 3").unwrap(), 5.0);
        assert_eq!(evaluate("10 - 4").unwrap(), 6.0);
        assert_eq!(evaluate("3 * 4").unwrap(), 12.0);
        assert_eq!(evaluate("15 / 3").unwrap(), 5.0);
        assert_eq!(evaluate("17 % 5").unwrap(), 2.0);
    }

    #[test]
    fn test_operator_precedence() {
        assert_eq!(evaluate("2 + 3 * 4").unwrap(), 14.0);
        assert_eq!(evaluate("(2 + 3) * 4").unwrap(), 20.0);
    }

    #[test]
    fn test_power() {
        assert_eq!(evaluate("2 ^ 3").unwrap(), 8.0);
        assert_eq!(evaluate("2 ** 10").unwrap(), 1024.0);
        // Right-associative: 2^(3^2) = 2^9 = 512
        assert_eq!(evaluate("2 ^ 3 ^ 2").unwrap(), 512.0);
    }

    #[test]
    fn test_negative_numbers() {
        assert_eq!(evaluate("-5 + 3").unwrap(), -2.0);
        assert_eq!(evaluate("-(2 + 3)").unwrap(), -5.0);
    }

    #[test]
    fn test_nested_parentheses() {
        assert_eq!(evaluate("((2 + 3) * (4 - 1))").unwrap(), 15.0);
    }

    #[test]
    fn test_decimals() {
        let result = evaluate("3.14 * 2").unwrap();
        assert!((result - 6.28).abs() < 1e-10);
    }

    #[test]
    fn test_division_by_zero() {
        assert!(evaluate("5 / 0").is_err());
    }

    #[test]
    fn test_invalid_expression() {
        assert!(evaluate("").is_err());
        assert!(evaluate("abc").is_err());
        assert!(evaluate("2 +").is_err());
        assert!(evaluate("(2 + 3").is_err());
    }

    #[test]
    fn test_unary_plus() {
        // Unary plus is valid: 2 + +3 = 5
        assert_eq!(evaluate("2 + +3").unwrap(), 5.0);
    }

    #[test]
    fn test_complex_expression() {
        // (10 + 5) * 2 - 3^2 + 7 % 3 = 15*2 - 9 + 1 = 30 - 9 + 1 = 22
        assert_eq!(evaluate("(10 + 5) * 2 - 3^2 + 7 % 3").unwrap(), 22.0);
    }
}
