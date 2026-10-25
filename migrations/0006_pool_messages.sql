-- Optional per-pool messages. `popup_message` is shown before generating sets from, or
-- studying, the pool; `print_message` is printed on generated sets. Empty means none.
ALTER TABLE question_pools
    ADD COLUMN IF NOT EXISTS popup_message TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS print_message TEXT NOT NULL DEFAULT '';
