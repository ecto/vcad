-- Track whether we've already sent the 80% usage alert for this period,
-- so the chat handler doesn't re-send on every subsequent message.
alter table usage_periods
  add column if not exists usage_alert_sent_at timestamptz;
