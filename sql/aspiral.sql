CREATE SCHEMA IF NOT EXISTS aspiral;

CREATE TABLE IF NOT EXISTS aspiral.metadata (
    view_name TEXT PRIMARY KEY,
    parent_view TEXT NOT NULL,
    frame_seconds INTEGER NOT NULL,
    base_view TEXT NOT NULL,
    scope_columns TEXT[] NOT NULL DEFAULT '{}',
    columns_metadata JSONB NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS aspiral.sources (
    view_name TEXT NOT NULL,
    base_view TEXT NOT NULL,
    frame_seconds INTEGER NOT NULL,
    base_column TEXT NOT NULL,
    formula TEXT NOT NULL,
    mat_column TEXT NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}',
    PRIMARY KEY (view_name, base_column, formula)
);

CREATE TABLE IF NOT EXISTS aspiral.changelog (
    base_view TEXT NOT NULL,
    scope_values JSONB NOT NULL DEFAULT '{}',
    t_start BIGINT NOT NULL,
    t_end BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_aspiral_changelog_base ON aspiral.changelog (base_view);

CREATE OR REPLACE FUNCTION aspiral.track_changes_stmt() RETURNS TRIGGER AS $$
DECLARE
    v_base_view TEXT := TG_ARGV[0];
    v_ts BIGINT;
    v_te BIGINT;
BEGIN
    IF (TG_OP = 'INSERT' OR TG_OP = 'UPDATE') THEN
        SELECT MIN(aspiral(t)), MAX(aspiral(t)) INTO v_ts, v_te FROM new_table;
        IF v_ts IS NOT NULL THEN
            INSERT INTO aspiral.changelog (base_view, t_start, t_end) VALUES (v_base_view, v_ts, v_te);
        END IF;
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;
