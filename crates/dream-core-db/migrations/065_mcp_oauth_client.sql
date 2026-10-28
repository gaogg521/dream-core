-- Public OAuth client metadata is stored atomically with its issued token.
-- Contains client_id/issuer/resource only; client secrets are not accepted.
ALTER TABLE oauth_tokens ADD COLUMN client_info TEXT;
