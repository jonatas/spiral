CREATE SCHEMA IF NOT EXISTS spiral;

CREATE TABLE IF NOT EXISTS spiral.metadata (
    view_name TEXT PRIMARY KEY,
    parent_view TEXT NOT NULL,
    frame_seconds INTEGER NOT NULL,
    base_view TEXT NOT NULL,
    scope_columns TEXT[] NOT NULL DEFAULT '{}',
    columns_metadata JSONB NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS spiral.sources (
    view_name TEXT NOT NULL,
    base_view TEXT NOT NULL,
    frame_seconds INTEGER NOT NULL,
    base_column TEXT NOT NULL,
    formula TEXT NOT NULL,
    mat_column TEXT NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}',
    PRIMARY KEY (view_name, base_column, formula)
);

CREATE TABLE IF NOT EXISTS spiral.changelog (
    base_view TEXT NOT NULL,
    scope_values JSONB NOT NULL DEFAULT '{}',
    t_start BIGINT NOT NULL,
    t_end BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_spiral_changelog_base ON spiral.changelog (base_view);

CREATE TYPE time_value_spiral AS (v double precision, t bigint);

CREATE OR REPLACE FUNCTION first_spiral_sfunc(state time_value_spiral, val double precision, ts bigint) RETURNS time_value_spiral AS $$
    BEGIN RETURN CASE WHEN state IS NULL OR ts < state.t THEN (val, ts)::time_value_spiral ELSE state END; END;
$$ LANGUAGE plpgsql IMMUTABLE;

CREATE OR REPLACE FUNCTION last_spiral_sfunc(state time_value_spiral, val double precision, ts bigint) RETURNS time_value_spiral AS $$
    BEGIN RETURN CASE WHEN state IS NULL OR ts >= state.t THEN (val, ts)::time_value_spiral ELSE state END; END;
$$ LANGUAGE plpgsql IMMUTABLE;

CREATE OR REPLACE FUNCTION first_spiral_final(state time_value_spiral) RETURNS double precision AS $$
    BEGIN RETURN CASE WHEN state IS NULL THEN NULL ELSE state.v END; END;
$$ LANGUAGE plpgsql IMMUTABLE;

CREATE AGGREGATE first(double precision, bigint) (
    sfunc = first_spiral_sfunc,
    stype = time_value_spiral,
    finalfunc = first_spiral_final
);

CREATE AGGREGATE last(double precision, bigint) (
    sfunc = last_spiral_sfunc,
    stype = time_value_spiral,
    finalfunc = first_spiral_final
);

-- PASSTHROUGHS & HELPERS
CREATE OR REPLACE FUNCTION to_timestamptz(BIGINT) RETURNS TIMESTAMPTZ AS $$
    SELECT spiral_from_epoch($1);
$$ LANGUAGE SQL IMMUTABLE PARALLEL SAFE;

CREATE OR REPLACE FUNCTION spiral.track_changes_stmt() RETURNS TRIGGER AS $$
DECLARE
    v_base_view TEXT := TG_ARGV[0];
    v_ts BIGINT;
    v_te BIGINT;
BEGIN
    IF (TG_OP = 'INSERT' OR TG_OP = 'UPDATE') THEN
        -- Explicitly cast to timestamptz to resolve any ambiguity.
        SELECT MIN(spiral(t::timestamptz)), MAX(spiral(t::timestamptz)) INTO v_ts, v_te FROM new_table;
        IF v_ts IS NOT NULL THEN
            INSERT INTO spiral.changelog (base_view, t_start, t_end) VALUES (v_base_view, v_ts, v_te);
        END IF;
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql
SET search_path = public, spiral, "$user";

-- STORAGE ALIASES
CREATE OR REPLACE FUNCTION spiral_pack_delta_zero(text, int) RETURNS void AS 'SELECT spiral_pack_delta($1, $2)' LANGUAGE SQL;
CREATE OR REPLACE FUNCTION spiral_read_main_zero(int, bigint, bigint) RETURNS double precision AS 'SELECT spiral_read_main($1, $2, $3)' LANGUAGE SQL;

-- LEGACY SUPPORT
CREATE OR REPLACE FUNCTION first_pg_sfunc(state time_value_spiral, val double precision, ts bigint) RETURNS time_value_spiral AS 'SELECT first_spiral_sfunc($1, $2, $3)' LANGUAGE SQL;
CREATE OR REPLACE FUNCTION last_pg_sfunc(state time_value_spiral, val double precision, ts bigint) RETURNS time_value_spiral AS 'SELECT last_spiral_sfunc($1, $2, $3)' LANGUAGE SQL;

-- ROBUST REGISTER WRAPPER
CREATE OR REPLACE FUNCTION spiral_register_view(
    view_name text, 
    parent_view text, 
    frame_seconds integer, 
    base_view text, 
    scope_columns text[]
) RETURNS void AS $$
BEGIN
    -- This calls the Rust function
    PERFORM spiral_register_view_rust(view_name, parent_view, frame_seconds, base_view, scope_columns);
END;
$$ LANGUAGE plpgsql
SET search_path = public, spiral, "$user";

-- Missing functions from demo.sql
CREATE OR REPLACE FUNCTION to_spiraling_number(t BIGINT, cycle INTEGER, lane INTEGER) RETURNS BIGINT AS $$
    SELECT spiral_zorder($1, ARRAY[$2::text, $3::text]);
$$ LANGUAGE SQL IMMUTABLE PARALLEL SAFE;

CREATE OR REPLACE FUNCTION to_spiral(t BIGINT, cycle INTEGER) RETURNS BOX AS $$
    SELECT box(point(($1 % $2)::double precision, ($1 / $2)::double precision), 
               point(($1 % $2 + 10)::double precision, ($1 / $2 + 10)::double precision));
$$ LANGUAGE SQL IMMUTABLE PARALLEL SAFE;

CREATE OR REPLACE FUNCTION spiral_create_partition(table_name TEXT, range_size BIGINT, partition_id INTEGER) RETURNS VOID AS $$
DECLARE
    v_start BIGINT := range_size * partition_id;
    v_end BIGINT := range_size * (partition_id + 1);
    v_part_name TEXT := table_name || '_' || partition_id;
BEGIN
    EXECUTE format('CREATE TABLE IF NOT EXISTS %I PARTITION OF %I FOR VALUES FROM (%L) TO (%L)', 
                   v_part_name, table_name, v_start, v_end);
END;
$$ LANGUAGE plpgsql;

-- DOCS SUPPORT
CREATE OR REPLACE VIEW spiral.available_sources AS
SELECT 
    m.base_view,
    m.view_name,
    m.frame_seconds,
    s.base_column,
    s.formula,
    s.mat_column
FROM spiral.metadata m
JOIN spiral.sources s ON m.view_name = s.view_name
ORDER BY m.base_view, m.frame_seconds;
