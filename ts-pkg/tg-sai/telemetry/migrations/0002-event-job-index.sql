-- Lets `tg-sai events show <id>` find rows by job_id without a table scan.
CREATE INDEX event_job_id ON event (job_id);
