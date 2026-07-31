-- Record the concrete public operation independently from its routing format.
-- Existing rows predate Images support and can be inferred without ambiguity.
ALTER TABLE request_logs
    ADD COLUMN api_operation text
        CHECK (
            (api_format = 'open_ai_chat_completions' AND api_operation = 'chat_completions')
            OR (api_format = 'open_ai_responses' AND api_operation = 'responses')
            OR (
                api_format = 'open_ai_images'
                AND api_operation IN ('images_generation', 'images_edit')
            )
        );

-- Keep rolling upgrades compatible with an older Gateway process that still
-- inserts only api_format. New binaries always send the explicit operation;
-- Images rows without one can only mean generation in this release.
CREATE FUNCTION infer_request_log_api_operation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.api_operation IS NULL THEN
        NEW.api_operation := CASE NEW.api_format
            WHEN 'open_ai_chat_completions' THEN 'chat_completions'
            WHEN 'open_ai_responses' THEN 'responses'
            WHEN 'open_ai_images' THEN 'images_generation'
        END;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER request_logs_infer_api_operation
BEFORE INSERT ON request_logs
FOR EACH ROW EXECUTE FUNCTION infer_request_log_api_operation();

ALTER TABLE request_logs DISABLE TRIGGER request_logs_prevent_mutation;

UPDATE request_logs
SET api_operation = CASE api_format
    WHEN 'open_ai_chat_completions' THEN 'chat_completions'
    WHEN 'open_ai_responses' THEN 'responses'
END;

ALTER TABLE request_logs ENABLE TRIGGER request_logs_prevent_mutation;

ALTER TABLE request_logs
    ALTER COLUMN api_operation SET NOT NULL;
