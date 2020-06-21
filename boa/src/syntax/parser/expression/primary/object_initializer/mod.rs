//! Object initializer parsing.
//!
//! More information:
//!  - [MDN documentation][mdn]
//!  - [ECMAScript specification][spec]
//!
//! [mdn]: https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Operators/Object_initializer
//! [spec]: https://tc39.es/ecma262/#sec-object-initializer

#[cfg(test)]
mod tests;
use crate::syntax::lexer::{Token, TokenKind};
use crate::{
    syntax::{
        ast::{
            node::{self, FunctionExpr, MethodDefinitionKind, Node, Object},
            Punctuator,
        },
        parser::{
            expression::AssignmentExpression,
            function::{FormalParameters, FunctionBody},
            AllowAwait, AllowIn, AllowYield, ParseError, ParseResult, Parser, TokenParser,
        },
    },
    BoaProfiler,
};
use std::io::Read;

/// Parses an object literal.
///
/// More information:
///  - [MDN documentation][mdn]
///  - [ECMAScript specification][spec]
///
/// [mdn]: https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Operators/Object_initializer
/// [spec]: https://tc39.es/ecma262/#prod-ObjectLiteral
#[derive(Debug, Clone, Copy)]
pub(super) struct ObjectLiteral {
    allow_yield: AllowYield,
    allow_await: AllowAwait,
}

impl ObjectLiteral {
    /// Creates a new `ObjectLiteral` parser.
    pub(super) fn new<Y, A>(allow_yield: Y, allow_await: A) -> Self
    where
        Y: Into<AllowYield>,
        A: Into<AllowAwait>,
    {
        Self {
            allow_yield: allow_yield.into(),
            allow_await: allow_await.into(),
        }
    }
}

impl<R> TokenParser<R> for ObjectLiteral
where
    R: Read,
{
    type Output = Object;

    fn parse(self, parser: &mut Parser<R>) -> Result<Self::Output, ParseError> {
        let _timer = BoaProfiler::global().start_event("ObjectLiteral", "Parsing");
        let mut elements = Vec::new();

        loop {
            if parser.next_if(Punctuator::CloseBlock).is_some() {
                break;
            }

            elements
                .push(PropertyDefinition::new(self.allow_yield, self.allow_await).parse(parser)?);

            if parser.next_if(Punctuator::CloseBlock).is_some() {
                break;
            }

            if parser.next_if(Punctuator::Comma).is_none() {
                let next_token = parser.next().ok_or(ParseError::AbruptEnd)?;
                return Err(ParseError::expected(
                    vec![
                        TokenKind::Punctuator(Punctuator::Comma),
                        TokenKind::Punctuator(Punctuator::CloseBlock),
                    ],
                    next_token.clone(),
                    "object literal",
                ));
            }
        }

        Ok(Object::from(elements))
    }
}

/// Parses a property definition.
///
/// More information:
///  - [ECMAScript specification][spec]
///
/// [spec]: https://tc39.es/ecma262/#prod-PropertyDefinition
#[derive(Debug, Clone, Copy)]
struct PropertyDefinition {
    allow_yield: AllowYield,
    allow_await: AllowAwait,
}

impl PropertyDefinition {
    /// Creates a new `PropertyDefinition` parser.
    fn new<Y, A>(allow_yield: Y, allow_await: A) -> Self
    where
        Y: Into<AllowYield>,
        A: Into<AllowAwait>,
    {
        Self {
            allow_yield: allow_yield.into(),
            allow_await: allow_await.into(),
        }
    }
}

impl<R> TokenParser<R> for PropertyDefinition
where
    R: Read,
{
    type Output = node::PropertyDefinition;

    fn parse(self, parser: &mut Parser<R>) -> Result<Self::Output, ParseError> {
        if parser.next_if(Punctuator::Spread).is_some() {
            let node = AssignmentExpression::new(true, self.allow_yield, self.allow_await)
                .parse(parser)?;
            return Ok(node::PropertyDefinition::SpreadObject(node));
        }

        let prop_name = parser
            .next()
            .map(Token::to_string)
            .ok_or(ParseError::AbruptEnd)?;
        if parser.next_if(Punctuator::Colon).is_some() {
            let val = AssignmentExpression::new(true, self.allow_yield, self.allow_await)
                .parse(parser)?;
            return Ok(node::PropertyDefinition::property(prop_name, val));
        }

        if parser
            .next_if(TokenKind::Punctuator(Punctuator::OpenParen))
            .is_some()
            || ["get", "set"].contains(&prop_name.as_str())
        {
            return MethodDefinition::new(self.allow_yield, self.allow_await, prop_name)
                .parse(parser);
        }

        let pos = parser
            .peek(0)
            .map(|tok| tok.span().start())
            .ok_or(ParseError::AbruptEnd)?;
        Err(ParseError::general("expected property definition", pos))
    }
}

/// Parses a method definition.
///
/// More information:
///  - [ECMAScript specification][spec]
///
/// [spec]: https://tc39.es/ecma262/#prod-MethodDefinition
#[derive(Debug, Clone)]
struct MethodDefinition {
    allow_yield: AllowYield,
    allow_await: AllowAwait,
    identifier: String,
}

impl MethodDefinition {
    /// Creates a new `MethodDefinition` parser.
    fn new<Y, A, I>(allow_yield: Y, allow_await: A, identifier: I) -> Self
    where
        Y: Into<AllowYield>,
        A: Into<AllowAwait>,
        I: Into<String>,
    {
        Self {
            allow_yield: allow_yield.into(),
            allow_await: allow_await.into(),
            identifier: identifier.into(),
        }
    }
}

impl<R> TokenParser<R> for MethodDefinition
where
    R: Read,
{
    type Output = node::PropertyDefinition;

    fn parse(self, parser: &mut Parser<R>) -> Result<Self::Output, ParseError> {
        let (methodkind, prop_name, params) = match self.identifier.as_str() {
            idn @ "get" | idn @ "set" => {
                let prop_name = parser
                    .next()
                    .map(Token::to_string)
                    .ok_or(ParseError::AbruptEnd)?;
                parser.expect(
                    TokenKind::Punctuator(Punctuator::OpenParen),
                    "property method definition",
                )?;
                let first_param = parser.peek(0).expect("current token disappeared").clone();
                let params = FormalParameters::new(false, false).parse(parser)?;
                parser.expect(Punctuator::CloseParen, "method definition")?;
                if idn == "get" {
                    if !params.is_empty() {
                        return Err(ParseError::unexpected(
                            first_param,
                            "getter functions must have no arguments",
                        ));
                    }
                    (MethodDefinitionKind::Get, prop_name, params)
                } else {
                    if params.len() != 1 {
                        return Err(ParseError::unexpected(
                            first_param,
                            "setter functions must have one argument",
                        ));
                    }
                    (MethodDefinitionKind::Set, prop_name, params)
                }
            }
            prop_name => {
                let params = FormalParameters::new(false, false).parse(parser)?;
                parser.expect(Punctuator::CloseParen, "method definition")?;
                (
                    MethodDefinitionKind::Ordinary,
                    prop_name.to_string(),
                    params,
                )
            }
        };

        parser.expect(
            TokenKind::Punctuator(Punctuator::OpenBlock),
            "property method definition",
        )?;
        let body = FunctionBody::new(false, false).parse(parser)?;
        parser.expect(
            TokenKind::Punctuator(Punctuator::CloseBlock),
            "property method definition",
        )?;

        Ok(node::PropertyDefinition::method_definition(
            methodkind,
            prop_name,
            FunctionExpr::new(None, params, body),
        ))
    }
}

/// Initializer parsing.
///
/// More information:
///  - [ECMAScript specification][spec]
///
/// [spec]: https://tc39.es/ecma262/#prod-Initializer
#[derive(Debug, Clone, Copy)]
pub(in crate::syntax::parser) struct Initializer {
    allow_in: AllowIn,
    allow_yield: AllowYield,
    allow_await: AllowAwait,
}

impl Initializer {
    /// Creates a new `Initializer` parser.
    pub(in crate::syntax::parser) fn new<I, Y, A>(
        allow_in: I,
        allow_yield: Y,
        allow_await: A,
    ) -> Self
    where
        I: Into<AllowIn>,
        Y: Into<AllowYield>,
        A: Into<AllowAwait>,
    {
        Self {
            allow_in: allow_in.into(),
            allow_yield: allow_yield.into(),
            allow_await: allow_await.into(),
        }
    }
}

impl<R> TokenParser<R> for Initializer
where
    R: Read,
{
    type Output = Node;

    fn parse(self, parser: &mut Parser<R>) -> ParseResult {
        parser.expect(TokenKind::Punctuator(Punctuator::Assign), "initializer")?;
        AssignmentExpression::new(self.allow_in, self.allow_yield, self.allow_await).parse(parser)
    }
}
