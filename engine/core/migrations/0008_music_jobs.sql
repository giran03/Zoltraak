-- One row per track a music download produces.
--
-- A music job is a third shape of work, next to a queued download and a video
-- job: the source is usually a *catalogue entry* (a Spotify track, an Apple
-- Music song) rather than a file, so the job has to find an audio stream for
-- it first, fetch that, and then re-encode and tag the result. None of that
-- fits the byte-range queue, and the video table's columns (a chosen
-- `VideoFormat`, a merge) describe a different pipeline. So it gets its own
-- table and its own manager, the same way video did.
--
-- A playlist or album is a set of rows sharing a `collection_id`; there is no
-- collection table because a collection has no state of its own — it is
-- whatever its tracks are.
CREATE TABLE music_jobs (
    id               TEXT    PRIMARY KEY,

    collection_id    TEXT,
    collection_title TEXT,
    collection_url   TEXT,
    -- Order within the collection (or enqueue order for a single). Drives
    -- track numbering and the order the worker takes jobs in.
    position         INTEGER NOT NULL DEFAULT 0,

    -- JSON `MusicTrack`: everything the catalogue said about the track,
    -- including how its audio is to be obtained. Stored whole because a resume
    -- or a retry must reproduce the original request exactly, and analysis is
    -- stateless.
    track            TEXT    NOT NULL,
    -- JSON `OutputOptions` as they were when the job was queued. Changing the
    -- defaults later never rewrites queued work.
    options          TEXT    NOT NULL,

    directory        TEXT    NOT NULL,
    -- The file stem, resolved once at enqueue and reused verbatim; the
    -- extension is decided by the conversion step.
    stem             TEXT    NOT NULL,

    -- queued | resolving | downloading | converting | finished | failed | skipped | paused
    state            TEXT    NOT NULL,
    -- Human-readable detail of the current step, for the UI's one-line readout.
    stage            TEXT,
    -- The stream the audio was actually taken from (a YouTube watch URL, a
    -- SoundCloud transcoding), kept so a wrong match can be seen and judged.
    matched_url      TEXT,
    matched_title    TEXT,

    downloaded_bytes INTEGER NOT NULL DEFAULT 0,
    total_bytes      INTEGER,
    error            TEXT,
    path             TEXT,

    created_at       TEXT    NOT NULL,
    updated_at       TEXT    NOT NULL,
    finished_at      TEXT
);

-- The worker's claim query, and the UI's newest-first listing.
CREATE INDEX idx_music_jobs_state ON music_jobs (state, created_at ASC, position ASC);
CREATE INDEX idx_music_jobs_collection ON music_jobs (collection_id, position ASC);
