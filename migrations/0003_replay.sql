-- Replay grants a new bounded budget without resetting the fencing generation.
ALTER TABLE hl7v2.inbox DROP CONSTRAINT inbox_attempt_limit_check;
ALTER TABLE hl7v2.inbox ALTER COLUMN attempt_limit TYPE bigint;
ALTER TABLE hl7v2.inbox ADD CHECK (attempt_limit >= 1 AND attempt_limit >= generation);

CREATE TABLE hl7v2.replays (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    scope text COLLATE "C" NOT NULL CHECK (octet_length(scope) BETWEEN 1 AND 512),
    request_id text COLLATE "C" NOT NULL CHECK (octet_length(request_id) BETWEEN 1 AND 128),
    message_id bigint NOT NULL REFERENCES hl7v2.inbox(id),
    generation bigint NOT NULL CHECK (generation > 0),
    plan bytea NOT NULL CHECK (octet_length(plan) = 32),
    additional_attempts integer NOT NULL CHECK (additional_attempts BETWEEN 1 AND 1000),
    previous_limit bigint NOT NULL CHECK (previous_limit >= generation),
    previous_reason text NOT NULL CHECK (previous_reason IN ('requested', 'attempt_limit')),
    operator text NOT NULL CHECK (octet_length(operator) BETWEEN 1 AND 256),
    reason text NOT NULL CHECK (octet_length(reason) BETWEEN 1 AND 2048),
    commit_mode text NOT NULL CHECK (commit_mode IN ('on', 'local', 'remote_write', 'remote_apply')),
    requested_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (scope, request_id)
);

ALTER TABLE hl7v2.inbox ADD COLUMN replay_id bigint REFERENCES hl7v2.replays(id);
ALTER TABLE hl7v2.attempts ADD COLUMN replay_id bigint REFERENCES hl7v2.replays(id);
CREATE INDEX attempts_replay_id ON hl7v2.attempts(replay_id) WHERE replay_id IS NOT NULL;
