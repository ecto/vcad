-- Encrypt CLI auth tokens at rest.
--
-- Before this migration, cli_auth_codes stored the access_token and
-- refresh_token in plaintext keyed by the device code. A leaked backup
-- (or a compromised service role key) surrenders every in-flight CLI
-- session. After this migration the API writes:
--
--   code        = sha256(raw_code || CLI_AUTH_CODE_PEPPER)
--   enc_access_token  = AES-256-GCM(plaintext, key = HKDF(CLI_AUTH_ENC_KEY, salt = code))
--   enc_refresh_token = AES-256-GCM(plaintext, key = HKDF(CLI_AUTH_ENC_KEY, salt = code))
--   enc_nonce   = 12 random bytes used for both AES-GCM invocations
--
-- so that a database dump alone is worthless without the raw code the
-- browser originally generated.

alter table cli_auth_codes
    add column if not exists enc_access_token  bytea,
    add column if not exists enc_refresh_token bytea,
    add column if not exists enc_nonce         bytea;

-- The old plaintext columns stay in the schema for one release so a
-- rollback is possible, but they become optional and the application
-- no longer writes to them.
alter table cli_auth_codes
    alter column access_token  drop not null;
-- refresh_token was already nullable.

-- Drop any rows created under the old plaintext contract so nothing
-- decryptable-with-plaintext-tokens lingers after the API is redeployed.
delete from cli_auth_codes where enc_access_token is null;
