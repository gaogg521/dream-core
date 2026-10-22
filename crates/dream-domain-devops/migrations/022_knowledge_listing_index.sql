-- Knowledge listing, visibility checks and search joins read only short metadata columns, but `visibility`,
-- `library_id` and `content_revision` were added after `content` (ALTER TABLE appends), so reading any of them
-- from the table walks the document's content overflow pages. On a large library every listing and every member
-- visibility check was reading all document text. These covering indexes answer those reads without the table.

-- Administrators' listing: ORDER BY created_at, every listed column.
CREATE INDEX IF NOT EXISTS idx_one_rag_documents_listing ON one_rag_documents
    (created_at, id, title, file_path, file_size, mime_type, status, last_error, chunk_count,
     scope, team_id, visibility, created_by, library_id);

-- Members' visibility predicate (scope / team / visibility / owner), with the listed columns.
-- Supersedes the (scope, team_id) index from 009, which forced a table lookup per row.
CREATE INDEX IF NOT EXISTS idx_one_rag_documents_scope_cover ON one_rag_documents
    (scope, team_id, visibility, created_by, created_at, id, title, file_path, file_size, mime_type, status,
     last_error, chunk_count, library_id);
DROP INDEX IF EXISTS idx_one_rag_documents_scope;

-- Search joins chunks to their document by id and then checks visibility.
CREATE INDEX IF NOT EXISTS idx_one_rag_documents_acl ON one_rag_documents
    (id, scope, team_id, visibility, created_by, chunk_count, title);
