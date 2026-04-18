-- High-performance aggregates for the Aspiral ecosystem
CREATE AGGREGATE aspiral_sketch(double precision) (
    sfunc = aspiral_sketch_sfunc,
    stype = bytea
);
CREATE AGGREGATE aspiral_sketch_merge(bytea) (
    sfunc = aspiral_sketch_merge_sfunc,
    stype = bytea
);
CREATE AGGREGATE first(double precision, bigint) (
    sfunc = first_sfunc,
    stype = TimeValue,
    finalfunc = time_value_final
);
CREATE AGGREGATE last(double precision, bigint) (
    sfunc = last_sfunc,
    stype = TimeValue,
    finalfunc = time_value_final
);
