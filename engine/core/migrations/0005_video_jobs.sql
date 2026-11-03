CREATE TABLE video_jobs (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    webpage_url TEXT NOT NULL,
    state TEXT NOT NULL,
    downloaded_bytes INTEGER NOT NULL DEFAULT 0,
    error TEXT,
    path TEXT,
    created_at TEXT NOT NULL
);
