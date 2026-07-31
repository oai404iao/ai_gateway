-- Add the standalone OpenAI Images API family while preserving the
-- one-format-per-rule/group/channel routing invariant.
--
-- PostgreSQL requires a newly added enum value to commit before a later
-- migration may safely reference it in table constraints.
ALTER TYPE api_format ADD VALUE 'open_ai_images';
