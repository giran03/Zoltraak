-- Rule-based destination routing. Evaluated once, at enqueue time; editing
-- or deleting a category never moves files a prior match already placed.
CREATE TABLE categories (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL,
    kind        TEXT    NOT NULL,
    pattern     TEXT    NOT NULL,
    directory   TEXT    NOT NULL,

    -- Higher runs first when more than one rule could match the same URL.
    priority    INTEGER NOT NULL DEFAULT 0,
    position    INTEGER NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 1,

    created_at  TEXT    NOT NULL
) STRICT;

CREATE INDEX idx_categories_matching ON categories (enabled, priority DESC, position ASC);

-- Whether the queue applied auto-categorization when a download was added,
-- kept for display even after the category itself is deleted.
ALTER TABLE downloads ADD COLUMN category_name TEXT;
