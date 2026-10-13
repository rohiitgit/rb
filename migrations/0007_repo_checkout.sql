-- The checkout a repository was last indexed from (contract M7h S1): the canonical path of its
-- git root, so a question asked inside that checkout finds the repository whatever it was named.
-- NULL for repositories indexed before this migration (they keep the directory-name rule).
ALTER TABLE rb.repo ADD COLUMN checkout text;
CREATE INDEX repo_checkout_idx ON rb.repo (checkout);
