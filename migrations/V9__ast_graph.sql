-- AST Graph Tables

CREATE TABLE memory_ast_nodes (
    id UUID PRIMARY KEY,
    document_id UUID NOT NULL REFERENCES memory_documents(id) ON DELETE CASCADE,
    node_type TEXT NOT NULL,
    name TEXT NOT NULL,
    content_preview TEXT NOT NULL,
    start_byte INTEGER NOT NULL,
    end_byte INTEGER NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (document_id, name)
);

CREATE INDEX idx_memory_ast_nodes_doc on memory_ast_nodes(document_id);

CREATE TABLE memory_ast_edges (
    id UUID PRIMARY KEY,
    source_node_id UUID NOT NULL REFERENCES memory_ast_nodes(id) ON DELETE CASCADE,
    target_node_id UUID NOT NULL REFERENCES memory_ast_nodes(id) ON DELETE CASCADE,
    edge_type TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_memory_ast_edges_source on memory_ast_edges(source_node_id);
CREATE INDEX idx_memory_ast_edges_target on memory_ast_edges(target_node_id);
