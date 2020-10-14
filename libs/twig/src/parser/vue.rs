use super::IResult;
use crate::ast::*;
use nom::branch::alt;
use nom::bytes::complete::{tag, take_until};
use nom::character::complete::multispace0;
use nom::combinator::map;
use nom::sequence::delimited;

pub(crate) fn vue_block(input: &str) -> IResult<HtmlNode> {
    delimited(
        multispace0,
        delimited(
            tag("{{"),
            delimited(
                multispace0,
                map(alt((take_until(" }}"), take_until("}}"))), |content| {
                    HtmlNode::VueBlock(VueBlock { content })
                }),
                multispace0,
            ),
            tag("}}"),
        ),
        multispace0,
    )(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_some_vue_variable_print() {
        let res = vue_block("{{ $tc('swag-migration.index.confirmAbortDialog.hint') }}");

        assert_eq!(
            res,
            Ok((
                "",
                HtmlNode::VueBlock(VueBlock {
                    content: "$tc('swag-migration.index.confirmAbortDialog.hint')"
                })
            ))
        )
    }

    #[test]
    fn test_some_vue_variable_print_with_complex_logic() {
        let res = vue_block(
            "       {{   if a { $tc('swag-migration.index.confirmAbortDialog.hint' ) } else {  $tc('nothing' ); }    }}       ",
        );

        assert_eq!(
            res,
            Ok((
                "",
                HtmlNode::VueBlock(VueBlock {
                    content: "if a { $tc('swag-migration.index.confirmAbortDialog.hint' ) } else {  $tc('nothing' ); }   "
                })
            ))
        )
    }
}
