-- The startup writer performs the six-operation table rebuild using the frozen
-- operation_split planner and storage applicator in this same transaction.
-- It retains exact financial rows, indexes, views, triggers and storage checks;
-- foreign_keys must be disabled before BEGIN and checked before commit.
SELECT 1;
