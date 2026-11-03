-- Request headers forwarded by an external client (the browser extension),
-- stored as a JSON array of [name, value] pairs.
--
-- NULL for anything added from the CLI or the app's own URL field, which is
-- the majority of rows — a session cookie only matters for a link the user was
-- already authenticated to reach.
ALTER TABLE downloads ADD COLUMN headers TEXT;
