-- Apply after 0001_inbox.sql. Custody bytes and pinned plans are unchanged.
ALTER TABLE hl7v2.inbox DROP CONSTRAINT inbox_state_check;
ALTER TABLE hl7v2.inbox
    ADD COLUMN generation bigint NOT NULL DEFAULT 0 CHECK (generation >= 0),
    ADD COLUMN attempt_limit integer CHECK (attempt_limit BETWEEN 1 AND 1000),
    ADD COLUMN available_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    ADD COLUMN quarantine_reason text CHECK (quarantine_reason IN ('requested', 'attempt_limit')),
    ADD CONSTRAINT inbox_state_check CHECK (state IN ('ready', 'processing', 'retry_scheduled', 'completed', 'quarantined')),
    ADD CHECK (state <> 'processing' OR (generation > 0 AND attempt_limit IS NOT NULL)),
    ADD CHECK ((state = 'quarantined') = (quarantine_reason IS NOT NULL));

CREATE INDEX inbox_due_work ON hl7v2.inbox(scope, plan, available_at, id)
    WHERE state IN ('ready', 'processing', 'retry_scheduled');

CREATE TABLE hl7v2.attempts (
    message_id bigint NOT NULL REFERENCES hl7v2.inbox(id),
    generation bigint NOT NULL CHECK (generation > 0),
    worker text NOT NULL CHECK (octet_length(worker) BETWEEN 1 AND 256),
    plan bytea NOT NULL CHECK (octet_length(plan) = 32),
    started_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    lease_until timestamptz NOT NULL,
    ended_at timestamptz,
    outcome text CHECK (outcome IN ('completed', 'retry_scheduled', 'quarantined', 'retry_limit', 'expired')),
    report bytea NOT NULL DEFAULT ''::bytea CHECK (octet_length(report) <= 65536),
    commit_mode text NOT NULL CHECK (commit_mode IN ('on', 'local', 'remote_write', 'remote_apply')),
    PRIMARY KEY (message_id, generation),
    CHECK ((outcome IS NULL) = (ended_at IS NULL))
);
