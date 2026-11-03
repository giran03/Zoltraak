-- Every attempt to put a finished file into Discord, kept as a row.
--
-- A delivery is not a download and does not belong on the download's row: one
-- file can be sent more than once, a send can fail long after the download
-- succeeded, and a manual send has no download behind it at all. Keeping them
-- separate also means the Discord feature can be switched off, or fail
-- entirely, without touching the queue.
CREATE TABLE discord_deliveries (
    id           TEXT PRIMARY KEY,
    -- 'download' | 'video' | 'manual'
    source       TEXT NOT NULL,
    -- The queue item or video job behind it, when there is one.
    source_id    TEXT,
    path         TEXT NOT NULL,
    filename     TEXT NOT NULL,
    bytes        INTEGER NOT NULL,
    -- pending | preparing | uploading | sent | skipped | failed
    state        TEXT NOT NULL,
    -- Why it was skipped, or how it failed. Never holds a credential.
    reason       TEXT,
    message_url  TEXT,
    -- What was actually uploaded, which differs from `bytes` when the file was
    -- re-encoded to fit. Both are shown, so the transcode is never silent.
    sent_bytes   INTEGER,
    attempts     INTEGER NOT NULL DEFAULT 0,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

-- The worker's only query: the oldest thing still waiting.
CREATE INDEX discord_deliveries_state ON discord_deliveries (state, created_at);
