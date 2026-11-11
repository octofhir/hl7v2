-- Initial local PostgreSQL custody schema. Apply once with the host's migration tool.
CREATE SCHEMA hl7v2;

CREATE TABLE hl7v2.inbox (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    scope text COLLATE "C" NOT NULL CHECK (octet_length(scope) BETWEEN 1 AND 512),
    processing_id bytea NOT NULL CHECK (octet_length(processing_id) = 1),
    control_id bytea NOT NULL CHECK (octet_length(control_id) BETWEEN 1 AND 256),
    plan bytea NOT NULL CHECK (octet_length(plan) = 32),
    payload bytea NOT NULL CHECK (octet_length(payload) BETWEEN 1 AND 8388608),
    sha256 bytea NOT NULL CHECK (octet_length(sha256) = 32),
    acknowledgement bytea NOT NULL CHECK (octet_length(acknowledgement) BETWEEN 1 AND 65536),
    state text NOT NULL DEFAULT 'ready' CHECK (state = 'ready'),
    received_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (scope, processing_id, control_id)
);

CREATE TABLE hl7v2.deliveries (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    message_id bigint NOT NULL REFERENCES hl7v2.inbox(id),
    channel text NOT NULL CHECK (octet_length(channel) BETWEEN 1 AND 1024),
    requested_plan bytea NOT NULL CHECK (octet_length(requested_plan) = 32),
    payload bytea NOT NULL CHECK (octet_length(payload) BETWEEN 1 AND 8388608),
    sha256 bytea NOT NULL CHECK (octet_length(sha256) = 32),
    disposition text NOT NULL CHECK (disposition IN ('new', 'duplicate', 'conflict')),
    acknowledgement bytea NOT NULL CHECK (octet_length(acknowledgement) BETWEEN 1 AND 65536),
    commit_mode text NOT NULL CHECK (commit_mode IN ('on', 'local', 'remote_write', 'remote_apply')),
    received_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE INDEX deliveries_message_id ON hl7v2.deliveries(message_id, id);
