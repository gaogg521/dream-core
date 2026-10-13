-- How a scene turns a rough request into a real one. A scene was a prompt plus a
-- package of skills and tools, but "make me a website" is an entry point, not a
-- specification: the conversation should first ask what it needs (who it is for,
-- how it is done today, what is in and out of the first version) and only build
-- once that is confirmed. This holds that as JSON: {discovery, starters,
-- questions, constraints, deliverables}. Optional; NULL keeps a scene behaving as
-- it did, prompt and resources only.
ALTER TABLE one_scenes ADD COLUMN intent TEXT;
