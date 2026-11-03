-- What a paused or interrupted video job needs in order to continue.
--
-- Analysis is stateless by design: the client hands back the exact format an
-- earlier `analyze` returned, and nothing is cached server-side in between.
-- That is fine while a job runs inside one call, but a job that is paused --
-- or that outlives a restart -- has nobody left to hand it that format again.
-- So the chosen format, the session it was resolved with, and where it was
-- headed are stored on the row itself.
ALTER TABLE video_jobs ADD COLUMN format TEXT;

-- The request headers the format was resolved with, as a JSON array of
-- [name, value] pairs -- same shape and same reasoning as `downloads.headers`.
-- A login-gated stream needs the session that unlocked it on every attempt,
-- not just the first.
ALTER TABLE video_jobs ADD COLUMN headers TEXT;

ALTER TABLE video_jobs ADD COLUMN directory TEXT;

-- Resolved once, when the job is created, and reused verbatim on every resume.
-- Re-deriving it would re-run the collision numbering against a directory that
-- now contains this job's own partial file, so a resumed job would pick a new
-- name and orphan the bytes it already downloaded.
ALTER TABLE video_jobs ADD COLUMN filename TEXT;

-- JSON: per-stream segment/byte position plus the playlist fingerprint each
-- was measured against. See `video::stream::StreamProgress`.
ALTER TABLE video_jobs ADD COLUMN resume_state TEXT;
