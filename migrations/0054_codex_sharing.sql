CREATE TABLE codex_sharing_groups (
    id uuid PRIMARY KEY,
    user_group_id uuid NOT NULL UNIQUE REFERENCES user_groups(id) ON DELETE RESTRICT,
    credential_id uuid NOT NULL UNIQUE
        REFERENCES codex_oauth_credentials(channel_id) ON DELETE RESTRICT,
    provider_account_id text NOT NULL,
    provider_user_id text NOT NULL CHECK (provider_user_id <> ''),
    name text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 120),
    enabled boolean NOT NULL,
    seats jsonb NOT NULL CHECK (
        jsonb_typeof(seats) = 'array' AND jsonb_array_length(seats) BETWEEN 1 AND 100
    ),
    primary_limit_amount numeric(20,8) NOT NULL CHECK (primary_limit_amount > 0),
    secondary_limit_amount numeric(20,8) NOT NULL CHECK (secondary_limit_amount > 0),
    request_reservation_amount numeric(20,8) NOT NULL CHECK (request_reservation_amount > 0),
    user_requests_per_minute integer NOT NULL CHECK (user_requests_per_minute > 0),
    group_requests_per_minute integer NOT NULL CHECK (group_requests_per_minute > 0),
    user_max_concurrent_requests integer NOT NULL CHECK (user_max_concurrent_requests > 0),
    group_max_concurrent_requests integer NOT NULL CHECK (group_max_concurrent_requests > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (provider_account_id, provider_user_id)
);

CREATE TRIGGER codex_sharing_groups_set_updated_at
BEFORE UPDATE ON codex_sharing_groups
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE FUNCTION protect_codex_sharing_binding() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.id <> OLD.id OR NEW.credential_id <> OLD.credential_id
       OR NEW.user_group_id <> OLD.user_group_id
       OR NEW.provider_account_id <> OLD.provider_account_id
       OR NEW.provider_user_id <> OLD.provider_user_id
       OR jsonb_array_length(NEW.seats) < jsonb_array_length(OLD.seats)
    THEN
        RAISE EXCEPTION 'sharing bindings and existing seat numbers are immutable';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER codex_sharing_binding_guard
BEFORE UPDATE ON codex_sharing_groups
FOR EACH ROW EXECUTE FUNCTION protect_codex_sharing_binding();

-- The local ledger must accompany the database during recovery. A second
-- process or an empty replacement volume must not silently issue fresh money.
CREATE TABLE codex_sharing_ledger (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    ledger_id uuid NOT NULL UNIQUE
);

CREATE FUNCTION protect_codex_sharing_identity() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.deleted_at IS NOT NULL AND EXISTS (
        SELECT 1 FROM codex_sharing_groups WHERE credential_id = OLD.channel_id
    ) THEN
        RAISE EXCEPTION 'a sharing credential cannot be deleted';
    END IF;
    IF EXISTS (
        SELECT 1 FROM codex_sharing_groups
        WHERE credential_id = OLD.channel_id
          AND (provider_account_id <> COALESCE(NEW.account_id, '')
               OR provider_user_id IS DISTINCT FROM NEW.user_id)
    ) THEN
        RAISE EXCEPTION 'a sharing credential cannot change provider identity';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER codex_sharing_identity_guard
BEFORE UPDATE OF account_id, user_id, deleted_at ON codex_oauth_credentials
FOR EACH ROW EXECUTE FUNCTION protect_codex_sharing_identity();
