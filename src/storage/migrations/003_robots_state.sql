-- M5: add `state` column to robots_cache so we can distinguish a parsed entry
-- from a sentinel (allow_all on 4xx, disallow_all on 5xx/timeout fail-closed).
--
-- Pre-existing rows (none in shipped releases — M2 created the table but M5
-- is the first milestone to write to it) are interpreted as parsed entries
-- since their `body` column carries the robots.txt text.

ALTER TABLE robots_cache ADD COLUMN state TEXT NOT NULL DEFAULT 'parsed';
