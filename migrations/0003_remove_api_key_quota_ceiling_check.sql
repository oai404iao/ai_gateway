-- Stage 5 uses a soft precheck: an already-over-limit settled amount must be
-- representable because the preceding request may have settled above the cap.
-- The original unnamed table CHECK must therefore be found by its definition,
-- rather than assuming PostgreSQL assigned a particular constraint name.
DO $$
DECLARE
    quota_ceiling_constraints text[];
    quota_ceiling_count bigint;
BEGIN
    SELECT array_agg(conname ORDER BY conname), count(*)
      INTO quota_ceiling_constraints, quota_ceiling_count
      FROM pg_constraint
     WHERE conrelid = 'api_keys'::regclass
       AND contype = 'c'
       AND lower(regexp_replace(
             pg_get_constraintdef(oid),
             '[[:space:]()]',
             '',
             'g'
           )) LIKE '%quota_used_amount<=quota_limit_amount%';

    IF quota_ceiling_count <> 1 THEN
        RAISE EXCEPTION
            'expected exactly one api_keys quota-used ceiling CHECK constraint, found %',
            quota_ceiling_count;
    END IF;

    EXECUTE format(
        'ALTER TABLE api_keys DROP CONSTRAINT %I',
        quota_ceiling_constraints[1]
    );
END;
$$;
