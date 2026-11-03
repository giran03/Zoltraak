-- Queue and history. One row per download, whatever its state.
CREATE TABLE downloads (
    id               TEXT    PRIMARY KEY,
    url              TEXT    NOT NULL,
    filename         TEXT    NOT NULL,
    directory        TEXT    NOT NULL,
    state            TEXT    NOT NULL,

    -- NULL when the server sent no Content-Length.
    total_bytes      INTEGER,
    downloaded_bytes INTEGER NOT NULL DEFAULT 0,
    connections      INTEGER,
    -- ETag or Last-Modified, mirrored from the sidecar for display.
    validator        TEXT,

    -- Higher priority runs first; position breaks ties in insertion order.
    priority         INTEGER NOT NULL DEFAULT 0,
    position         INTEGER NOT NULL,

    -- Last failure message, kept after a retry succeeds for diagnostics.
    error            TEXT,

    created_at       TEXT    NOT NULL,
    updated_at       TEXT    NOT NULL,
    completed_at     TEXT
) STRICT;

-- The scheduler's hot path: find the next runnable item.
CREATE INDEX idx_downloads_runnable
    ON downloads (state, priority DESC, position ASC);

CREATE INDEX idx_downloads_created ON downloads (created_at DESC);

-- The same URL landing in the same directory twice is almost always a
-- double-submit rather than an intentional second copy.
CREATE UNIQUE INDEX idx_downloads_target ON downloads (url, directory, filename);
