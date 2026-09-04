-- one-devops 016 (MySQL port): per-model protocol overrides on a company model
-- channel. See the SQLite copy for rationale. JSON object string, so TEXT (no
-- DEFAULT possible on MySQL TEXT, and NULL is the intended "no overrides"
-- state anyway).
ALTER TABLE one_provider_registry ADD COLUMN model_protocols TEXT NULL;
