-- How many questions of each type a pool holds, kept up to date whenever its questions change so
-- the generator can tell which question types a pool offers without reading the questions.
-- An empty object means "not computed yet"; the server fills those in at startup.
ALTER TABLE question_pools
    ADD COLUMN IF NOT EXISTS type_counts JSONB NOT NULL DEFAULT '{}'::jsonb;
