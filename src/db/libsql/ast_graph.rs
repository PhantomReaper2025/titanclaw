//! AstGraphStore implementation for LibSqlBackend.

use async_trait::async_trait;
use libsql::params;
use uuid::Uuid;

use super::{LibSqlBackend, get_i64, get_text};
use crate::db::{AstGraphStore, StoredAstEdge, StoredAstNode};
use crate::error::WorkspaceError;

#[async_trait]
impl AstGraphStore for LibSqlBackend {
    async fn delete_ast_nodes(&self, document_id: Uuid) -> Result<(), WorkspaceError> {
        let conn = self
            .connect()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: e.to_string(),
            })?;
        conn.execute(
            "DELETE FROM memory_ast_nodes WHERE document_id = ?1",
            params![document_id.to_string()],
        )
        .await
        .map_err(|e| WorkspaceError::SearchFailed {
            reason: format!("Delete AST nodes failed: {}", e),
        })?;
        Ok(())
    }

    async fn insert_ast_node(
        &self,
        document_id: Uuid,
        node_type: &str,
        name: &str,
        content_preview: &str,
        start_byte: i64,
        end_byte: i64,
    ) -> Result<Uuid, WorkspaceError> {
        let conn = self
            .connect()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: e.to_string(),
            })?;
        let id = Uuid::new_v4();
        conn.execute(
            r#"INSERT OR REPLACE INTO memory_ast_nodes
               (id, document_id, node_type, name, content_preview, start_byte, end_byte)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
            params![
                id.to_string(),
                document_id.to_string(),
                node_type,
                name,
                content_preview,
                start_byte,
                end_byte,
            ],
        )
        .await
        .map_err(|e| WorkspaceError::SearchFailed {
            reason: format!("Insert AST node failed: {}", e),
        })?;
        Ok(id)
    }

    async fn insert_ast_edge(
        &self,
        source_node_id: Uuid,
        target_node_id: Uuid,
        edge_type: &str,
    ) -> Result<Uuid, WorkspaceError> {
        let conn = self
            .connect()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: e.to_string(),
            })?;
        let id = Uuid::new_v4();
        conn.execute(
            r#"INSERT INTO memory_ast_edges
               (id, source_node_id, target_node_id, edge_type)
               VALUES (?1, ?2, ?3, ?4)"#,
            params![
                id.to_string(),
                source_node_id.to_string(),
                target_node_id.to_string(),
                edge_type,
            ],
        )
        .await
        .map_err(|e| WorkspaceError::SearchFailed {
            reason: format!("Insert AST edge failed: {}", e),
        })?;
        Ok(id)
    }

    async fn get_ast_nodes(&self, document_id: Uuid) -> Result<Vec<StoredAstNode>, WorkspaceError> {
        let conn = self
            .connect()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: e.to_string(),
            })?;
        let mut rows = conn
            .query(
                r#"SELECT id, document_id, node_type, name, content_preview, start_byte, end_byte
                   FROM memory_ast_nodes WHERE document_id = ?1"#,
                params![document_id.to_string()],
            )
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Query AST nodes failed: {}", e),
            })?;

        let mut nodes = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Row fetch failed: {}", e),
            })?
        {
            nodes.push(StoredAstNode {
                id: get_text(&row, 0).parse().unwrap_or_default(),
                document_id: get_text(&row, 1).parse().unwrap_or_default(),
                node_type: get_text(&row, 2),
                name: get_text(&row, 3),
                content_preview: get_text(&row, 4),
                start_byte: get_i64(&row, 5),
                end_byte: get_i64(&row, 6),
            });
        }
        Ok(nodes)
    }

    async fn get_ast_edges_from(
        &self,
        source_node_id: Uuid,
    ) -> Result<Vec<StoredAstEdge>, WorkspaceError> {
        let conn = self
            .connect()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: e.to_string(),
            })?;
        let mut rows = conn
            .query(
                "SELECT id, source_node_id, target_node_id, edge_type FROM memory_ast_edges WHERE source_node_id = ?1",
                params![source_node_id.to_string()],
            )
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Query AST edges failed: {}", e),
            })?;

        let mut edges = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Row fetch failed: {}", e),
            })?
        {
            edges.push(StoredAstEdge {
                id: get_text(&row, 0).parse().unwrap_or_default(),
                source_node_id: get_text(&row, 1).parse().unwrap_or_default(),
                target_node_id: get_text(&row, 2).parse().unwrap_or_default(),
                edge_type: get_text(&row, 3),
            });
        }
        Ok(edges)
    }

    async fn find_ast_nodes_by_name(
        &self,
        name: &str,
    ) -> Result<Vec<StoredAstNode>, WorkspaceError> {
        let conn = self
            .connect()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: e.to_string(),
            })?;
        let mut rows = conn
            .query(
                r#"SELECT id, document_id, node_type, name, content_preview, start_byte, end_byte
                   FROM memory_ast_nodes WHERE name = ?1"#,
                params![name],
            )
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Query AST nodes by name failed: {}", e),
            })?;

        let mut nodes = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Row fetch failed: {}", e),
            })?
        {
            nodes.push(StoredAstNode {
                id: get_text(&row, 0).parse().unwrap_or_default(),
                document_id: get_text(&row, 1).parse().unwrap_or_default(),
                node_type: get_text(&row, 2),
                name: get_text(&row, 3),
                content_preview: get_text(&row, 4),
                start_byte: get_i64(&row, 5),
                end_byte: get_i64(&row, 6),
            });
        }
        Ok(nodes)
    }
}
