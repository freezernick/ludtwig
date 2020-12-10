use crate::process::FileContext;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncWrite, AsyncWriteExt, BufWriter};
use twig::ast::{HtmlComment, HtmlNode, HtmlPlain, HtmlTag, TwigBlock, TwigComment, VueBlock};

#[derive(Clone, PartialEq)]
struct PrintingContext<'a> {
    previous_node: Option<&'a HtmlNode>,
    after_node: Option<&'a HtmlNode>,

    /// the last node in the list is the current node. everything before that is up in the hierarchy.
    parent_nodes: Vec<&'a HtmlNode>,

    /// in tab count
    indentation: u16,
}

impl<'a> PrintingContext<'a> {
    /// clones the current context and returns a new one with the increased indentation.
    fn increase_indentation_by(&self, increase: u16) -> Self {
        let mut copy = self.clone();
        copy.indentation += increase;
        copy
    }

    fn get_parent(&self) -> Option<&'a HtmlNode> {
        self.parent_nodes
            .iter()
            .rev()
            .skip(1)
            .take(1)
            .map(|n| *n)
            .next()
    }
}

impl<'a> Default for PrintingContext<'a> {
    fn default() -> Self {
        PrintingContext {
            previous_node: None,
            after_node: None,
            parent_nodes: vec![],
            indentation: 0,
        }
    }
}

pub async fn write_tree(file_context: Arc<FileContext>) {
    let path = create_and_secure_output_path(&file_context).await;
    let file = File::create(path).await.expect("can't create file.");
    let mut writer = BufWriter::new(file);

    print_node(
        &mut writer,
        &file_context.tree,
        &mut PrintingContext::default(),
    )
    .await;

    writer.flush().await.unwrap();
}

async fn create_and_secure_output_path(file_context: &FileContext) -> PathBuf {
    let base_path = match &file_context.cli_context.output_path {
        None => Path::new(""),
        Some(p) => p,
    };
    let path = base_path.join(&*file_context.file_path);

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .expect("can't create directory for output");
    }

    path
}

fn print_node<'a, W: AsyncWrite + Unpin + Send + ?Sized>(
    writer: &'a mut W,
    node: &'a HtmlNode,
    context: &'a mut PrintingContext<'a>,
) -> Pin<Box<dyn Future<Output = ()> + 'a + Send>> {
    Box::pin(async move {
        context.parent_nodes.push(&node);

        match node {
            HtmlNode::Root(root) => {
                print_node_list(writer, &root, context).await;
            }
            HtmlNode::Tag(tag) => {
                print_tag(writer, &tag, context).await;
            }
            HtmlNode::Plain(plain) => {
                print_plain(writer, &plain, context).await;
            }
            HtmlNode::Comment(comment) => {
                print_html_comment(writer, comment, context).await;
            }
            HtmlNode::VueBlock(vue) => {
                print_vue_block(writer, &vue, context).await;
            }
            HtmlNode::TwigBlock(twig) => {
                print_twig_block(writer, &twig, context).await;
            }
            HtmlNode::TwigParentCall => {
                print_twig_parent_call(writer, context).await;
            }
            HtmlNode::TwigComment(comment) => {
                print_twig_comment(writer, comment, context).await;
            }
            HtmlNode::Whitespace => {
                print_whitespace(writer, context).await;
            }
        }
    })
}

async fn print_node_list<W: AsyncWrite + Unpin + Send + ?Sized>(
    writer: &mut W,
    nodes: &[HtmlNode],
    context: &PrintingContext<'_>,
) {
    for idx in 0..nodes.len() {
        let previous = if idx > 0 { nodes.get(idx - 1) } else { None };
        let current = &nodes[idx];
        let after = nodes.get(idx + 1);

        let mut context = PrintingContext {
            previous_node: previous,
            after_node: after,
            parent_nodes: context.parent_nodes.clone(),
            indentation: context.indentation,
        };

        print_node(writer, current, &mut context).await;
    }
}

async fn print_tag<W: AsyncWrite + Unpin + Send + ?Sized>(
    writer: &mut W,
    tag: &HtmlTag,
    context: &PrintingContext<'_>,
) {
    print_indentation_if_whitespace_exists_before(writer, context).await;

    writer.write_all(b"<").await.unwrap();
    writer.write_all(tag.name.as_bytes()).await.unwrap();

    let line_length_for_inlined_two_attributes = context.indentation as usize * 4
        + 1
        + tag.name.len()
        + tag
            .attributes
            .iter()
            .take(2)
            .map(|a| 1 + a.name.len() + a.value.as_ref().map(|v| v.len() + 3).unwrap_or(0))
            .sum::<usize>()
        + tag.self_closed as usize
        + 1
        + if tag
            .children
            .first()
            .map(|f| !matches!(f, HtmlNode::Whitespace))
            .unwrap_or(true)
        {
            1000 //add 1000 to line length if there is no whitespace between opening tag and children
        } else {
            0
        };

    let inline_mode = tag.attributes.len() <= 2 && line_length_for_inlined_two_attributes <= 120;
    let continuation_indent_mode = tag.name.len() > 8;

    // attributes
    for (index, attribute) in tag.attributes.iter().enumerate() {
        if inline_mode {
            writer.write_all(b" ").await.unwrap();
        } else if continuation_indent_mode {
            writer.write_all(b"\n").await.unwrap();
            print_indentation(writer, &context.increase_indentation_by(2)).await;
        } else {
            // write attribute on first line (same as tag)
            if index == 0 {
                writer.write_all(b" ").await.unwrap();
            } else {
                writer.write_all(b"\n").await.unwrap();

                print_indentation(writer, context).await;
                for _ in 0..(tag.name.len() + 2) {
                    writer.write_all(b" ").await.unwrap();
                }
            }
        }

        writer.write_all(attribute.name.as_bytes()).await.unwrap();

        if let Some(value) = &attribute.value {
            writer.write_all(b"=\"").await.unwrap();
            writer.write_all(value.as_bytes()).await.unwrap();
            writer.write_all(b"\"").await.unwrap();
        }
    }

    if tag.self_closed {
        writer.write_all(b"/>").await.unwrap();
    } else {
        writer.write_all(b">").await.unwrap();
        // only print children if tag is not self_closed!
        print_node_list(writer, &tag.children, &context.increase_indentation_by(1)).await;
    }

    if let Some(last) = tag.children.last() {
        if let HtmlNode::Whitespace = last {
            print_indentation(writer, context).await;
        }
    }

    if !tag.self_closed {
        writer.write_all(b"</").await.unwrap();
        writer.write_all(tag.name.as_bytes()).await.unwrap();
        writer.write_all(b">").await.unwrap();
    }
}

async fn print_plain<W: AsyncWrite + Unpin + Send + ?Sized>(
    writer: &mut W,
    plain: &HtmlPlain,
    context: &PrintingContext<'_>,
) {
    print_indentation_if_whitespace_exists_before(writer, context).await;
    writer.write_all(plain.plain.as_bytes()).await.unwrap();
}

async fn print_html_comment<W: AsyncWrite + Unpin + Send + ?Sized>(
    writer: &mut W,
    comment: &HtmlComment,
    context: &PrintingContext<'_>,
) {
    print_indentation_if_whitespace_exists_before(writer, context).await;
    writer.write_all(b"<!-- ").await.unwrap();
    writer.write_all(comment.content.as_bytes()).await.unwrap();
    writer.write_all(b" -->").await.unwrap();
}

async fn print_vue_block<W: AsyncWrite + Unpin + Send + ?Sized>(
    writer: &mut W,
    vue: &VueBlock,
    context: &PrintingContext<'_>,
) {
    print_indentation_if_whitespace_exists_before(writer, context).await;
    writer.write_all(b"{{ ").await.unwrap();
    writer.write_all(vue.content.as_bytes()).await.unwrap();
    writer.write_all(b" }}").await.unwrap();
}

async fn print_twig_block<W: AsyncWrite + Unpin + Send + ?Sized>(
    writer: &mut W,
    twig: &TwigBlock,
    context: &PrintingContext<'_>,
) {
    print_indentation_if_whitespace_exists_before(writer, context).await;

    writer.write_all(b"{% block ").await.unwrap();
    writer.write_all(twig.name.as_bytes()).await.unwrap();
    writer.write_all(b" %}").await.unwrap();

    print_node_list(writer, &twig.children, &context.increase_indentation_by(1)).await;

    if let Some(last) = twig.children.last() {
        if let HtmlNode::Whitespace = last {
            print_indentation(writer, context).await;
        }
    }

    writer.write_all(b"{% endblock %}").await.unwrap();
}

async fn print_twig_parent_call<W: AsyncWrite + Unpin + Send + ?Sized>(
    writer: &mut W,
    context: &PrintingContext<'_>,
) {
    print_indentation_if_whitespace_exists_before(writer, context).await;
    writer.write_all(b"{% parent %}").await.unwrap();
}

async fn print_twig_comment<W: AsyncWrite + Unpin + Send + ?Sized>(
    writer: &mut W,
    comment: &TwigComment,
    context: &PrintingContext<'_>,
) {
    print_indentation_if_whitespace_exists_before(writer, context).await;
    writer.write_all(b"{# ").await.unwrap();
    writer.write_all(comment.content.as_bytes()).await.unwrap();
    writer.write_all(b" #}").await.unwrap();
}

async fn print_whitespace<W: AsyncWrite + Unpin + Send + ?Sized>(
    writer: &mut W,
    context: &PrintingContext<'_>,
) {
    match (
        context.previous_node,
        context.after_node,
        context.get_parent(),
    ) {
        (Some(prev), Some(aft), Some(par)) => {
            if let HtmlNode::TwigBlock(_) = par {
                // don't print another whitespace if the parent is also a block.
            } else {
                if let HtmlNode::TwigBlock(_) = prev {
                    // print another whitespace.
                    writer.write_all(b"\r\n").await.unwrap();
                } else if let HtmlNode::TwigBlock(_) = aft {
                    // print another whitespace.
                    writer.write_all(b"\r\n").await.unwrap();
                }
            }
        }

        (None, Some(aft), Some(par)) => {
            if let HtmlNode::TwigBlock(_) = par {
                // don't print another whitespace if the parent is also a block.
            } else {
                if let HtmlNode::TwigBlock(_) = aft {
                    // print another whitespace.
                    writer.write_all(b"\r\n").await.unwrap();
                }
            }
        }

        (Some(prev), None, Some(par)) => {
            if let HtmlNode::TwigBlock(_) = par {
                // don't print another whitespace if the parent is also a block.
            } else {
                if let HtmlNode::TwigBlock(_) = prev {
                    // print another whitespace.
                    writer.write_all(b"\r\n").await.unwrap();
                }
            }
        }

        (_, _, _) => {}
    }

    writer.write_all(b"\r\n").await.unwrap();
}

async fn print_indentation<W: AsyncWrite + Unpin + Send + ?Sized>(
    writer: &mut W,
    context: &PrintingContext<'_>,
) {
    for _ in 0..context.indentation {
        writer.write_all(b"    ").await.unwrap();
    }
}

async fn print_indentation_if_whitespace_exists_before<W: AsyncWrite + Unpin + Send + ?Sized>(
    writer: &mut W,
    context: &PrintingContext<'_>,
) {
    if let Some(prev) = context.previous_node {
        if let HtmlNode::Whitespace = prev {
            print_indentation(writer, context).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use twig::ast::HtmlAttribute;

    async fn convert_tree_into_written_string(tree: HtmlNode) -> String {
        let mut writer_raw: Cursor<Vec<u8>> = Cursor::new(Vec::new());

        print_node(&mut writer_raw, &tree, &mut PrintingContext::default()).await;

        String::from_utf8(writer_raw.into_inner()).unwrap()
    }

    #[tokio::test]
    async fn test_write_empty_html_tag() {
        let tree = HtmlNode::Tag(HtmlTag {
            name: "this_is_a_test_one".to_string(),
            children: vec![
                HtmlNode::Whitespace,
                HtmlNode::Tag(HtmlTag {
                    name: "this_is_a_test_two".to_string(),
                    children: vec![],
                    ..Default::default()
                }),
                HtmlNode::Whitespace,
            ],
            ..Default::default()
        });

        let res = convert_tree_into_written_string(tree).await;

        assert_eq!(
            res,
            "<this_is_a_test_one>\r\n    <this_is_a_test_two></this_is_a_test_two>\r\n</this_is_a_test_one>".to_string()
        );
    }

    #[tokio::test]
    async fn test_write_simple_twig_block() {
        let tree = HtmlNode::TwigBlock(TwigBlock {
            name: "some_twig_block".to_string(),
            children: vec![
                HtmlNode::Whitespace,
                HtmlNode::Plain(HtmlPlain {
                    plain: "Hello world".to_string(),
                }),
                HtmlNode::Whitespace,
            ],
        });

        let res = convert_tree_into_written_string(tree).await;

        assert_eq!(
            res,
            "{% block some_twig_block %}\r\n    Hello world\r\n{% endblock %}".to_string()
        );
    }

    #[tokio::test]
    async fn test_write_nested_twig_block() {
        let tree = HtmlNode::TwigBlock(TwigBlock {
            name: "this_is_a_test_one".to_string(),
            children: vec![
                HtmlNode::Whitespace,
                HtmlNode::TwigBlock(TwigBlock {
                    name: "this_is_a_test_two".to_string(),
                    children: vec![
                        HtmlNode::Whitespace,
                        HtmlNode::TwigBlock(TwigBlock {
                            name: "this_is_a_test_three".to_string(),
                            children: vec![HtmlNode::Whitespace],
                        }),
                        HtmlNode::Whitespace,
                    ],
                }),
                HtmlNode::Whitespace,
            ],
        });

        let res = convert_tree_into_written_string(tree).await;

        assert_eq!(
            res,
            "{% block this_is_a_test_one %}\r\n    {% block this_is_a_test_two %}\r\n        {% block this_is_a_test_three %}\r\n        {% endblock %}\r\n    {% endblock %}\r\n{% endblock %}".to_string()
        );
    }

    #[tokio::test]
    async fn test_write_nested_twig_block_separation() {
        let tree = HtmlNode::Tag(HtmlTag {
            name: "this_is_a_test_one".to_string(),
            children: vec![
                HtmlNode::Whitespace,
                HtmlNode::TwigBlock(TwigBlock {
                    name: "this_is_a_test_two".to_string(),
                    children: vec![
                        HtmlNode::Whitespace,
                        HtmlNode::Plain(HtmlPlain {
                            plain: "Some content".to_string(),
                        }),
                        HtmlNode::Whitespace,
                    ],
                }),
                HtmlNode::Whitespace,
                HtmlNode::TwigBlock(TwigBlock {
                    name: "this_is_a_test_three".to_string(),
                    children: vec![
                        HtmlNode::Whitespace,
                        HtmlNode::Plain(HtmlPlain {
                            plain: "Some content".to_string(),
                        }),
                        HtmlNode::Whitespace,
                    ],
                }),
                HtmlNode::Whitespace,
            ],
            ..Default::default()
        });

        let res = convert_tree_into_written_string(tree).await;

        assert_eq!(
            res,
            "<this_is_a_test_one>\r\n\r\n    {% block this_is_a_test_two %}\r\n        Some content\r\n    {% endblock %}\r\n\r\n    {% block this_is_a_test_three %}\r\n        Some content\r\n    {% endblock %}\r\n\r\n</this_is_a_test_one>".to_string()
        );
    }

    #[tokio::test]
    async fn test_write_empty_twig_block() {
        let tree = HtmlNode::Tag(HtmlTag {
            name: "this_is_a_test_one".to_string(),
            children: vec![
                HtmlNode::Whitespace,
                HtmlNode::TwigBlock(TwigBlock {
                    name: "this_is_a_test_two".to_string(),
                    children: vec![],
                }),
                HtmlNode::Whitespace,
            ],
            ..Default::default()
        });

        let res = convert_tree_into_written_string(tree).await;

        assert_eq!(
            res,
            "<this_is_a_test_one>\r\n\r\n    {% block this_is_a_test_two %}{% endblock %}\r\n\r\n</this_is_a_test_one>".to_string()
        );
    }

    #[tokio::test]
    async fn test_write_tag_and_twig_block_without_whitespace() {
        let tree = HtmlNode::Tag(HtmlTag {
            name: "slot".to_string(),
            children: vec![HtmlNode::TwigBlock(TwigBlock {
                name: "sw_grid_slot_pagination".to_string(),
                children: vec![],
            })],
            attributes: vec![HtmlAttribute {
                name: "name".to_string(),
                value: Some("pagination".to_string()),
            }],
            ..Default::default()
        });

        let res = convert_tree_into_written_string(tree).await;

        assert_eq!(
            res,
            "<slot name=\"pagination\">{% block sw_grid_slot_pagination %}{% endblock %}</slot>"
                .to_string()
        );
    }

    #[tokio::test]
    async fn test_write_tag_and_twig_block_content_without_whitespace() {
        let tree = HtmlNode::Tag(HtmlTag {
            name: "slot".to_string(),
            children: vec![HtmlNode::TwigBlock(TwigBlock {
                name: "sw_grid_slot_pagination".to_string(),
                children: vec![HtmlNode::Plain(HtmlPlain {
                    plain: "Hello world".to_string(),
                })],
            })],
            attributes: vec![HtmlAttribute {
                name: "name".to_string(),
                value: Some("pagination".to_string()),
            }],
            ..Default::default()
        });

        let res = convert_tree_into_written_string(tree).await;

        assert_eq!(
            res,
            "<slot name=\"pagination\">{% block sw_grid_slot_pagination %}Hello world{% endblock %}</slot>"
                .to_string()
        );
    }
}
