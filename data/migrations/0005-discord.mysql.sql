ALTER TABLE `character_transaction`
    ADD COLUMN `sent_to_discord` TINYINT(1) NOT NULL DEFAULT 0,
    ADD COLUMN `resolved_by_name` TEXT;

CREATE INDEX IF NOT EXISTS `idx_character_transaction_discord`
    ON `character_transaction` (`status_id`, `sent_to_discord`);
