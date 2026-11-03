-- Engine-wide configuration: bandwidth limit, connection/folder defaults,
-- and the scheduler's window and completion action. One row, one JSON blob —
-- edited as a whole from a single settings screen, never partially.
CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;
