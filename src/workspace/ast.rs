use std::collections::HashSet;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Parser, Query, QueryCursor};

/// An AST node extracted from a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AstNode {
    /// E.g. "function", "struct", "impl"
    pub node_type: String,
    /// Name of the entity
    pub name: String,
    /// Snippet preview of the node
    pub content_preview: String,
    /// Byte range
    pub start_byte: usize,
    pub end_byte: usize,
    /// Potential outgoing calls/references (names of other nodes)
    pub outgoing_calls: HashSet<String>,
}

/// Parse a Rust file and extract its primary AST nodes and their outgoing function calls.
pub fn parse_rust_ast(content: &str) -> Result<Vec<AstNode>, String> {
    let mut parser = Parser::new();
    let language = tree_sitter_rust::LANGUAGE.into();
    parser.set_language(&language).map_err(|e| e.to_string())?;

    let tree = parser.parse(content, None).ok_or("Failed to parse code")?;
    let root_node = tree.root_node();

    // Query for function declarations and impl blocks
    let query_source = r#"
        (function_item name: (identifier) @name) @function
        (impl_item type: (type_identifier) @impl_name) @impl
    "#;
    
    let query = Query::new(&language, query_source).map_err(|e| e.to_string())?;
    let mut query_cursor = QueryCursor::new();
    
    let mut results = Vec::new();
    let text_bytes = content.as_bytes();

    let mut matches = query_cursor.matches(&query, root_node, text_bytes);
    
    while let Some(m) = matches.next() {
        let mut node_type = "";
        let mut name = "";
        let mut main_node: Option<Node> = None;

        for capture in m.captures {
            let capture_name = query.capture_names()[capture.index as usize];
            match capture_name {
                "function" => {
                    node_type = "function";
                    main_node = Some(capture.node);
                }
                "impl" => {
                    node_type = "impl";
                    main_node = Some(capture.node);
                }
                "name" | "impl_name" => {
                    if let Ok(text) = capture.node.utf8_text(text_bytes) {
                        let text_str: &str = text;
                        name = text_str;
                    }
                }
                _ => {}
            }
        }

        if let Some(node) = main_node {
            if !name.is_empty() {
                // Extract brief preview (first line)
                let full_text = node.utf8_text(text_bytes).unwrap_or("");
                let preview = full_text.lines().next().unwrap_or("").trim().to_string();

                // Find incoming/outgoing call queries within this block
                let outgoing = find_outgoing_calls(node, text_bytes, &language);

                results.push(AstNode {
                    node_type: node_type.to_string(),
                    name: name.to_string(),
                    content_preview: preview,
                    start_byte: node.start_byte(),
                    end_byte: node.end_byte(),
                    outgoing_calls: outgoing,
                });
            }
        }
    }

    Ok(results)
}

fn find_outgoing_calls(
    root: Node,
    source: &[u8],
    language: &tree_sitter::Language,
) -> HashSet<String> {
    let mut calls = HashSet::new();
    
    let call_query_src = r#"
        (call_expression function: (identifier) @call_target)
        (call_expression function: (field_expression field: (field_identifier) @call_target))
        (call_expression function: (scoped_identifier name: (identifier) @call_target))
    "#;
    
    if let Ok(query) = Query::new(language, call_query_src) {
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, root, source);
        while let Some(m) = matches.next() {
            for capture in m.captures {
                if let Ok(text) = capture.node.utf8_text(source) {
                    let text_str: &str = text;
                    calls.insert(text_str.to_string());
                }
            }
        }
    }
    
    calls
}
