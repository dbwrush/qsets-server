ALTER TABLE users
    ADD COLUMN IF NOT EXISTS can_upload_pools BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE question_pools
    ADD COLUMN IF NOT EXISTS created_by INTEGER REFERENCES users(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_question_pools_created_by ON question_pools (created_by);
