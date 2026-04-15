-- One-time device-code table for `vcad login` browser flow.
--
-- Rows are inserted by `POST /api/cli-auth` when the browser completes
-- OAuth, looked up once by `GET /api/cli-auth?code=X` from the TUI's
-- polling loop, and deleted on successful read. Expired rows (>10 min)
-- are cleaned up lazily on each GET and by a periodic cron below.

CREATE TABLE IF NOT EXISTS cli_auth_codes (
    code          TEXT        PRIMARY KEY,
    user_id       UUID        NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    access_token  TEXT        NOT NULL,
    refresh_token TEXT,
    expires_at    BIGINT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Fast expiry sweeps.
CREATE INDEX IF NOT EXISTS cli_auth_codes_created_at_idx
    ON cli_auth_codes (created_at);

-- Service role only — the code is the whole secret, no RLS escape hatch.
ALTER TABLE cli_auth_codes ENABLE ROW LEVEL SECURITY;

-- Cron: delete rows older than 15 minutes (5 min grace past the 10-min TTL).
-- Requires the `pg_cron` extension — if you don't have it, the lazy cleanup
-- in api/cli-auth.ts's GET handler is the backstop.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'pg_cron') THEN
        PERFORM cron.schedule(
            'cli_auth_codes_sweep',
            '*/5 * * * *',
            $sql$
                DELETE FROM cli_auth_codes
                WHERE created_at < now() - interval '15 minutes'
            $sql$
        );
    END IF;
END
$$;
