//! AST Graph Indexer.
//!
//! Reads content from workspace documents, parses them with tree-sitter,
//! and stores AST node/edge relationships into the database for GraphRAG queries.

use std::sync::Arc;

use uuid::Uuid;

use crate::db::AstGraphStore;
use crate::error::WorkspaceError;
use crate::workspace::ast::parse_rust_ast;

/// Indexes AST graph nodes and edges into the database.
pub struct AstGraphIndexer {
    db: Arc<dyn AstGraphStore>,
}

impl AstGraphIndexer {
    /// Create a new AST graph indexer.
    pub fn new(db: Arc<dyn AstGraphStore>) -> Self {
        Self { db }
    }

    /// Index a document's AST graph.
    ///
    /// Parses the content using tree-sitter, inserts AST nodes,
    /// and creates edges between callers and callees.
    pub async fn index_document(
        &self,
        document_id: Uuid,
        content: &str,
        path: &str,
    ) -> Result<(), WorkspaceError> {
        // Only parse Rust files for now
        if !path.ends_with(".rs") {
            return Ok(());
        }

        let ast_nodes = match parse_rust_ast(content) {
            Ok(nodes) => nodes,
            Err(e) => {
                tracing::warn!("AST parse failed for {}: {}", path, e);
                return Ok(());
            }
        };

        if ast_nodes.is_empty() {
            return Ok(());
        }

        // Delete existing AST nodes for this document (edges cascade)
        self.db.delete_ast_nodes(document_id).await?;

        // Insert nodes and collect their IDs keyed by name
        let mut node_ids: std::collections::HashMap<String, Uuid> =
            std::collections::HashMap::new();

        for node in &ast_nodes {
            let node_id = self
                .db
                .insert_ast_node(
                    document_id,
                    &node.node_type,
                    &node.name,
                    &node.content_preview,
                    node.start_byte as i64,
                    node.end_byte as i64,
                )
                .await?;
            node_ids.insert(node.name.clone(), node_id);
        }

        // Create edges: if function A calls function B (and B exists in this file),
        // create an edge A -> B with type "calls"
        for node in &ast_nodes {
            if let Some(&source_id) = node_ids.get(&node.name) {
                for callee_name in &node.outgoing_calls {
                    if let Some(&target_id) = node_ids.get(callee_name) {
                        self.db
                            .insert_ast_edge(source_id, target_id, "calls")
                            .await?;
                    }
                }
            }
        }

        tracing::debug!(
            "Indexed {} AST nodes for {}",
            ast_nodes.len(),
            path
        );

        Ok(())
    }
}
