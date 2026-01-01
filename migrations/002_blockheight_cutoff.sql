-- Add blockheight cutoff and input size validation for UTXO eligibility
-- Only UTXOs created before the cutoff block AND with small inputs are eligible for payout
-- UTXOs at or after the cutoff, or with large inputs, are treated as donations

-- The block height where the deposit transaction was confirmed
ALTER TABLE recycles ADD COLUMN deposit_block_height INTEGER;

-- Whether this deposit is eligible for payout (created before cutoff, inputs are dust)
-- 1 = eligible for 101% payout, 0 = donation (no payout)
ALTER TABLE recycles ADD COLUMN is_eligible INTEGER DEFAULT 1;

-- The reason for donation status (NULL if eligible)
-- Values: 'block_height' (after cutoff), 'input_too_large' (input > 1000 sats)
ALTER TABLE recycles ADD COLUMN donation_reason TEXT;

-- The maximum input UTXO value in the deposit transaction (for auditing)
ALTER TABLE recycles ADD COLUMN max_input_sats INTEGER;

-- Add index for querying donations
CREATE INDEX IF NOT EXISTS idx_recycles_is_eligible ON recycles(is_eligible);
