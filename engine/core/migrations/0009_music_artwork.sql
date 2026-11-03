-- The cover of the album or playlist a track was queued from.
--
-- It belongs to the collection rather than to any one track, and a job row is
-- the only thing that outlives the paste that created it: `MusicSource` is
-- handed back by a stateless analysis and is gone by the time the worker runs.
-- Without this, saving "the album cover" beside the files could only ever save
-- whatever the *first track* happened to carry, which on a compilation is the
-- wrong picture and on a matched track is a video still.
ALTER TABLE music_jobs ADD COLUMN collection_cover TEXT;
