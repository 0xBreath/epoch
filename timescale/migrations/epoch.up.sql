CREATE TABLE accounts
(
    id         BIGINT       NOT NULL,
    key        TEXT         NOT NULL,
    slot       BIGINT       NOT NULL,
    lamports   BIGINT       NOT NULL,
    owner      TEXT         NOT NULL,
    executable BOOLEAN      NOT NULL,
    rent_epoch BIGINT       NOT NULL,
    data       TEXT         NOT NULL
);


SELECT create_hypertable('accounts', by_range('slot'), if_not_exists => TRUE);

CREATE UNIQUE INDEX ON accounts (id, slot);

ALTER TABLE accounts SET (
    timescaledb.compress,
    timescaledb.compress_orderby = 'slot ASC',
    timescaledb.compress_segmentby = 'id'
);

-- compress after ~5 hours
SELECT add_compression_policy('accounts',  BIGINT '20000');

-- tier after ~5 hours
SELECT add_tiering_policy('accounts', BIGINT '20000');

alter database tsdb set timescaledb.enable_tiered_reads to true;