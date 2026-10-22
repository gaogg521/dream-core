-- SQLite-only covering index (see migrations/022). InnoDB keeps large TEXT values off-page already, so a listing
-- never reads them; nothing to do here, kept so both migration trees stay in step.
UPDATE one_rag_documents SET status = status WHERE 1 = 0;
