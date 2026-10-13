-- A scene's own prompt: what an administrator wants a conversation opened from
-- the scene to know — the role, the house style, what to do first. Until now a
-- scene was a name, a description, descriptive job-function tags and a grant
-- package, so opening one could carry the resources but nothing to say how to
-- use them. Free text, optional; the desktop falls back to the description and
-- job functions when it is empty.
ALTER TABLE one_scenes ADD COLUMN prompt TEXT;
