use crate::{CliContext, Config};
use ludtwig_parser::syntax::untyped::{SyntaxNode, SyntaxToken, TextRange};
use std::fmt::{Debug, Formatter};

pub trait Rule: Sync {
    /// A unique, kebab-case name for the rule.
    fn name(&self) -> &'static str;

    /// Check an individual untyped node in the syntax tree.
    /// The conversion to a typed AST node can be made at any time with a simple call to cast.
    /// Defaults to doing nothing.
    ///
    /// The return type is `Option<()>` to allow usage of the `?` (early return if not found) on the properties of the AST nodes
    /// which are all optional.
    #[allow(unused_variables)]
    #[inline]
    fn check_node(&self, node: SyntaxNode, ctx: &mut RuleContext) -> Option<()> {
        None
    }

    /// Check an individual untyped token (which doesn't have children) in the syntax tree.
    /// The conversion to a typed AST node can be made at any time with a simple call to cast.
    /// Defaults to doing nothing.
    ///
    /// The return type is `Option<()>` to allow usage of the `?` (early return if not found) on the properties of the AST nodes
    /// which are all optional.
    #[allow(unused_variables)]
    #[inline]
    fn check_token(&self, token: SyntaxToken, ctx: &mut RuleContext) -> Option<()> {
        None
    }

    /// Called once with the root untyped node in the syntax tree.
    /// Be Careful, rules that use this must follow this by themselves:
    /// - when iterating you should most likely skip SyntaxKind::Error Nodes!
    /// - you need to check for prev_siblings of LudtwigDirectiveIgnore and respect the ignored rules!
    ///
    /// The conversion to a typed AST node can be made at any time with a simple call to cast.
    /// Defaults to doing nothing.
    ///
    /// The return type is `Option<()>` to allow usage of the `?` (early return if not found) on the properties of the AST nodes
    /// which are all optional.
    #[allow(unused_variables)]
    #[inline]
    fn check_root(&self, node: SyntaxNode, ctx: &mut RuleContext) -> Option<()> {
        None
    }
}

impl Debug for dyn Rule {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rule<{}>", self.name())
    }
}

#[derive(Debug)]
pub struct TreeTraversalContext {
    pub inside_trivia_sensitive_node: bool,
}

#[derive(Debug)]
pub struct RuleContext {
    // file_id
    // source_text
    pub(super) check_results: Vec<CheckResult>,
    pub(super) cli_context: CliContext,
    pub(super) traversal_ctx: TreeTraversalContext,
}

impl RuleContext {
    pub fn create_result<S: Into<String>>(
        &self,
        rule_name: S,
        severity: Severity,
        message: S,
    ) -> CheckResult {
        CheckResult {
            rule_name: rule_name.into(),
            severity,
            message: message.into(),
            primary: None,
            suggestions: vec![],
        }
    }

    pub fn add_result(&mut self, result: CheckResult) {
        self.check_results.push(result);
    }

    pub fn config(&self) -> &Config {
        &self.cli_context.data.config
    }

    pub fn traversal_ctx(&self) -> &TreeTraversalContext {
        &self.traversal_ctx
    }
}

#[derive(Debug)]
pub struct CheckResult {
    // file_id
    pub(super) rule_name: String,
    pub(super) severity: Severity,
    pub(super) message: String,
    pub(super) primary: Option<CheckNote>,
    pub(super) suggestions: Vec<CheckSuggestion>,
}

impl CheckResult {
    // TODO: enforce only one primary_note call via type builder pattern
    /// The primary (red) label and location of the error, there should be only one of these per check result.
    /// Further context can be provided with multiple secondary notes.
    pub fn primary_note<S: Into<String>>(mut self, syntax_range: TextRange, message: S) -> Self {
        self.primary = Some(CheckNote {
            syntax_range,
            message: message.into(),
        });
        self
    }

    // The secondary (blue) label which can provide more context and explain the error to a user.
    // pub fn secondary_note<S: Into<String>>(mut self, syntax_range: TextRange, message: S) -> Self {
    //     self.secondary.push(CheckNote {
    //         syntax_range,
    //         message: message.into(),
    //     });
    //     self
    // }

    /// Add a code suggestion which the user can follow or is replaced automatically
    pub fn suggestion<R: Into<String>, S: Into<String>>(
        mut self,
        syntax_range: TextRange,
        replace_with: R,
        message: S,
    ) -> Self {
        self.suggestions.push(CheckSuggestion {
            syntax_range,
            replace_with: replace_with.into(),
            message: message.into(),
        });
        self
    }
}

#[derive(Debug)]
pub struct CheckNote {
    pub syntax_range: TextRange,
    pub message: String,
}

#[derive(Debug)]
pub struct CheckSuggestion {
    pub syntax_range: TextRange,
    pub replace_with: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub enum Severity {
    /// Errors which must be fixed for the template to work correctly
    Error,
    /// Potential errors which should be fixed before using the template in production
    Warning,
    /// Stylistic errors which should be fixed for readability
    Help,
    /// Just information
    Info,
}
