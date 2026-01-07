-- Track payment attempts to enforce retry limits
-- After MAX_PAYMENT_ATTEMPTS, recycle is marked as failed

ALTER TABLE recycles ADD COLUMN payment_attempts INTEGER DEFAULT 0;
